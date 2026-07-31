use std::{
    cmp::Ordering,
    io,
    io::Write as _,
    net::{IpAddr, Ipv4Addr},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use color_eyre::eyre::WrapErr as _;
use serde::ser::SerializeSeq as _;

use crate::{
    config::Config,
    ipdb,
    proxy::{Proxy, ProxyType},
    utils::is_container,
};

type IpDb = maxminddb::Reader<maxminddb::Mmap>;

const WRITE_BUFFER_BYTES: usize = 128 * 1024;

fn compare_timeout(a: &Proxy, b: &Proxy) -> Ordering {
    a.timeout.unwrap_or(Duration::MAX).cmp(&b.timeout.unwrap_or(Duration::MAX))
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum HostSortKey {
    Ipv4([u8; 4]),
    Name(compact_str::CompactString),
}

fn natural_sort_key(proxy: &Proxy) -> (ProxyType, HostSortKey, u16) {
    let host = proxy.host.parse::<Ipv4Addr>().map_or_else(
        move |_| HostSortKey::Name(proxy.host.clone()),
        |ip| HostSortKey::Ipv4(ip.octets()),
    );
    (proxy.protocol, host, proxy.port)
}

fn keep_english_name(names: &mut maxminddb::geoip2::Names<'_>) {
    *names = maxminddb::geoip2::Names {
        english: names.english,
        ..Default::default()
    };
}

fn strip_non_english_names(city: &mut maxminddb::geoip2::City<'_>) {
    keep_english_name(&mut city.city.names);
    keep_english_name(&mut city.continent.names);
    keep_english_name(&mut city.country.names);
    keep_english_name(&mut city.registered_country.names);
    keep_english_name(&mut city.represented_country.names);
    for subdivision in &mut city.subdivisions {
        keep_english_name(&mut subdivision.names);
    }
}

#[derive(serde::Serialize)]
struct ProxyJson<'a> {
    protocol: ProxyType,
    username: Option<&'a str>,
    password: Option<&'a str>,
    host: &'a str,
    port: u16,
    timeout: Option<f64>,
    exit_ip: Option<Ipv4Addr>,
    asn: Option<maxminddb::geoip2::Asn<'a>>,
    geolocation: Option<maxminddb::geoip2::City<'a>>,
}

struct ProxyListJson<'a> {
    proxies: &'a [Proxy],
    asn_db: Option<&'a IpDb>,
    geo_db: Option<&'a IpDb>,
}

impl serde::Serialize for ProxyListJson<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut seq = serializer.serialize_seq(Some(self.proxies.len()))?;
        for proxy in self.proxies {
            let exit_ip_addr = proxy.exit_ip.map(IpAddr::V4);

            let asn = match (self.asn_db, exit_ip_addr) {
                (Some(asn_db), Some(addr)) => asn_db
                    .lookup(addr)
                    .and_then(|entry| entry.decode())
                    .map_err(serde::ser::Error::custom)?,
                _ => None,
            };

            let geolocation = match (self.geo_db, exit_ip_addr) {
                (Some(geo_db), Some(addr)) => geo_db
                    .lookup(addr)
                    .and_then(|entry| {
                        entry.decode::<maxminddb::geoip2::City<'_>>()
                    })
                    .map_err(serde::ser::Error::custom)?
                    .map(|mut city| {
                        strip_non_english_names(&mut city);
                        city
                    }),
                _ => None,
            };

            seq.serialize_element(&ProxyJson {
                protocol: proxy.protocol,
                username: proxy.auth.as_ref().map(|a| a.username.as_str()),
                password: proxy.auth.as_ref().map(|a| a.password.as_str()),
                host: &proxy.host,
                port: proxy.port,
                timeout: proxy
                    .timeout
                    .map(|d| (d.as_secs_f64() * 100.0).round() / 100.0_f64),
                exit_ip: proxy.exit_ip,
                asn,
                geolocation,
            })?;
        }
        seq.end()
    }
}

fn write_json_file(
    path: &Path,
    pretty: bool,
    list: &ProxyListJson<'_>,
) -> crate::Result<()> {
    let file = std::fs::File::create(path).wrap_err_with(move || {
        compact_str::format_compact!(
            "failed to create file: {}",
            path.display()
        )
    })?;
    let mut writer =
        std::io::BufWriter::with_capacity(WRITE_BUFFER_BYTES, file);

    let failed = move || {
        compact_str::format_compact!(
            "failed to write to file: {}",
            path.display()
        )
    };

    if pretty {
        serde_json::to_writer_pretty(&mut writer, list)
    } else {
        serde_json::to_writer(&mut writer, list)
    }
    .wrap_err_with(failed)?;

    writer.flush().wrap_err_with(failed)
}

fn write_proxy_list_to_file<'a, I>(
    path: &Path,
    proxies: I,
    include_protocol: bool,
    buf: &mut Vec<u8>,
) -> crate::Result<()>
where
    I: IntoIterator<Item = &'a Proxy>,
{
    let mut file = std::fs::File::create(path).wrap_err_with(move || {
        compact_str::format_compact!(
            "failed to create file: {}",
            path.display()
        )
    })?;

    let write = move |file: &mut std::fs::File, buf: &[u8]| {
        file.write_all(buf).wrap_err_with(move || {
            compact_str::format_compact!(
                "failed to write to file: {}",
                path.display()
            )
        })
    };

    buf.clear();
    let mut first = true;
    for proxy in proxies {
        if first {
            first = false;
        } else {
            buf.push(b'\n');
        }
        proxy.write_to(buf, include_protocol);

        if buf.len() >= WRITE_BUFFER_BYTES {
            write(&mut file, buf)?;
            buf.clear();
        }
    }

    if !buf.is_empty() {
        write(&mut file, buf)?;
        buf.clear();
    }

    Ok(())
}

pub struct UseIpDb {
    pub asn: bool,
    pub geo: bool,
}

pub async fn save_proxies(
    config: Arc<Config>,
    mut proxies: Vec<Proxy>,
    use_ipdb: UseIpDb,
) -> crate::Result<()> {
    if config.output.sort_by_speed {
        proxies.sort_unstable_by(compare_timeout);
    } else {
        proxies.sort_by_cached_key(natural_sort_key);
    }

    if config.output.json.enabled {
        let (maybe_asn_db, maybe_geo_db) = tokio::try_join!(
            async move {
                if use_ipdb.asn {
                    ipdb::DbType::Asn.open_mmap().await.map(Some)
                } else {
                    Ok(None)
                }
            },
            async move {
                if use_ipdb.geo {
                    ipdb::DbType::Geo.open_mmap().await.map(Some)
                } else {
                    Ok(None)
                }
            }
        )?;

        let json_paths: [(PathBuf, bool); 2] = [
            (config.output.path.join("proxies.json"), false),
            (config.output.path.join("proxies_pretty.json"), true),
        ];

        proxies = tokio::task::spawn_blocking(move || {
            let list = ProxyListJson {
                proxies: &proxies,
                asn_db: maybe_asn_db.as_ref(),
                geo_db: maybe_geo_db.as_ref(),
            };
            for (path, pretty) in &json_paths {
                write_json_file(path, *pretty, &list)?;
            }
            crate::Result::Ok(proxies)
        })
        .await??;
    }

    if config.output.txt.enabled {
        let directory_path = config.output.path.join("proxies");
        match tokio::fs::remove_dir_all(&directory_path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).wrap_err_with(|| {
                compact_str::format_compact!(
                    "failed to remove directory: {}",
                    directory_path.display()
                )
            }),
        }?;
        tokio::fs::create_dir_all(&directory_path).await.wrap_err_with(
            || {
                compact_str::format_compact!(
                    "failed to create directory: {}",
                    directory_path.display()
                )
            },
        )?;

        let enabled_protocols = config.scraping.enabled_protocols;
        tokio::task::spawn_blocking(move || {
            let mut buf = Vec::with_capacity(WRITE_BUFFER_BYTES);

            write_proxy_list_to_file(
                &directory_path.join("all.txt"),
                proxies.iter(),
                true,
                &mut buf,
            )?;

            for proto in enabled_protocols.iter() {
                let mut file_path =
                    directory_path.join(proto.as_str_lowercase());
                file_path.set_extension("txt");
                write_proxy_list_to_file(
                    &file_path,
                    proxies.iter().filter(move |p| p.protocol == proto),
                    false,
                    &mut buf,
                )?;
            }

            crate::Result::Ok(())
        })
        .await??;
    }

    let path = std::path::absolute(&config.output.path)
        .unwrap_or_else(move |_| config.output.path.clone());
    if is_container().await {
        tracing::info!(
            "Proxies have been saved to ./out ({} in container)",
            path.display()
        );
    } else {
        tracing::info!("Proxies have been saved to {}", path.display());
    }

    Ok(())
}
