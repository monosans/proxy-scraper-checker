use std::{
    cmp::Ordering,
    io,
    net::{IpAddr, Ipv4Addr},
    path::Path,
    sync::Arc,
    time::Duration,
};

use color_eyre::eyre::WrapErr as _;
use serde::Serialize as _;
use tokio::io::AsyncWriteExt as _;

use crate::{
    HashMap,
    config::Config,
    ipdb,
    proxy::{Proxy, ProxyType},
    utils::is_docker,
};

fn compare_timeout(a: &Proxy, b: &Proxy) -> Ordering {
    a.timeout.unwrap_or(Duration::MAX).cmp(&b.timeout.unwrap_or(Duration::MAX))
}

fn compare_natural(a: &Proxy, b: &Proxy) -> Ordering {
    a.protocol
        .cmp(&b.protocol)
        .then_with(move || {
            match (a.host.parse::<Ipv4Addr>(), b.host.parse::<Ipv4Addr>()) {
                (Ok(ai), Ok(bi)) => ai.octets().cmp(&bi.octets()),
                (Ok(_), Err(_)) => Ordering::Less,
                (Err(_), Ok(_)) => Ordering::Greater,
                (Err(_), Err(_)) => a.host.cmp(&b.host),
            }
        })
        .then_with(move || a.port.cmp(&b.port))
}

fn strip_non_english_names(v: &mut serde_json::Value) {
    if let serde_json::Value::Object(map) = v {
        if let Some(names_val) = map.get_mut("names") {
            if let serde_json::Value::Object(names_map) = names_val {
                if let Some(en_val) = names_map.get("en").cloned() {
                    names_map.clear();
                    names_map.insert("en".to_owned(), en_val);
                } else {
                    names_map.clear();
                }
            }
        } else {
            for (_, val) in map {
                strip_non_english_names(val);
            }
        }
    } else if let serde_json::Value::Array(arr) = v {
        for item in arr {
            strip_non_english_names(item);
        }
    }
}

#[expect(clippy::ref_option)]
fn serialize_opt_strip_names<T: serde::Serialize, S: serde::Serializer>(
    opt: &Option<T>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    if let Some(t) = opt {
        let mut v =
            serde_json::to_value(t).map_err(serde::ser::Error::custom)?;
        strip_non_english_names(&mut v);
        v.serialize(serializer)
    } else {
        serializer.serialize_none()
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
    exit_ip: Option<&'a str>,
    #[serde(serialize_with = "serialize_opt_strip_names")]
    asn: Option<maxminddb::geoip2::Asn<'a>>,
    #[serde(serialize_with = "serialize_opt_strip_names")]
    geolocation: Option<maxminddb::geoip2::City<'a>>,
}

fn group_proxies<'a>(
    config: &Config,
    proxies: &'a [Proxy],
) -> HashMap<ProxyType, Vec<&'a Proxy>> {
    let mut groups: HashMap<_, _> =
        config.enabled_protocols().copied().map(|p| (p, Vec::new())).collect();
    for proxy in proxies {
        if let Some(group) = groups.get_mut(&proxy.protocol) {
            group.push(proxy);
        }
    }
    groups
}

async fn write_proxy_list_to_file<'a, I>(
    path: &Path,
    proxies: I,
    include_protocol: bool,
) -> crate::Result<()>
where
    I: IntoIterator<Item = &'a Proxy>,
{
    let file =
        tokio::fs::File::create(path).await.wrap_err_with(move || {
            compact_str::format_compact!(
                "failed to create file: {}",
                path.display()
            )
        })?;
    let mut writer = tokio::io::BufWriter::new(file);

    let mut first = true;
    let mut tmp = Vec::new();
    for proxy in proxies {
        if first {
            first = false;
        } else {
            writer.write_all(b"\n").await.wrap_err_with(move || {
                compact_str::format_compact!(
                    "failed to write to file: {}",
                    path.display()
                )
            })?;
        }

        proxy.write_to_sink(&mut tmp, include_protocol);
        writer.write_all(&tmp).await.wrap_err_with(move || {
            compact_str::format_compact!(
                "failed to write to file: {}",
                path.display()
            )
        })?;
        tmp.clear();
    }
    drop(tmp);

    writer.flush().await.wrap_err_with(move || {
        compact_str::format_compact!(
            "failed to write to file: {}",
            path.display()
        )
    })?;

    Ok(())
}

pub struct UseIpDb {
    pub asn: bool,
    pub geo: bool,
}

#[expect(clippy::too_many_lines)]
pub async fn save_proxies(
    config: Arc<Config>,
    mut proxies: Vec<Proxy>,
    use_ipdb: UseIpDb,
) -> crate::Result<()> {
    if config.output.sort_by_speed {
        proxies.sort_unstable_by(compare_timeout);
    } else {
        proxies.sort_unstable_by(compare_natural);
    }

    if config.output.json.enabled {
        let (maybe_asn_db, maybe_geo_db) = tokio::try_join!(
            async {
                if use_ipdb.asn {
                    ipdb::DbType::Asn.open_mmap().await.map(Some)
                } else {
                    Ok(None)
                }
            },
            async {
                if use_ipdb.geo {
                    ipdb::DbType::Geo.open_mmap().await.map(Some)
                } else {
                    Ok(None)
                }
            }
        )?;

        let mut proxy_dicts = Vec::with_capacity(proxies.len());
        for proxy in &proxies {
            proxy_dicts.push(ProxyJson {
                protocol: proxy.protocol,
                username: proxy.username.as_deref(),
                password: proxy.password.as_deref(),
                host: &proxy.host,
                port: proxy.port,
                timeout: proxy
                    .timeout
                    .map(|d| (d.as_secs_f64() * 100.0).round() / 100.0_f64),
                exit_ip: proxy.exit_ip.as_deref(),
                asn: if let Some(asn_db) = &maybe_asn_db {
                    if let Some(exit_ip) = &proxy.exit_ip {
                        let exit_ip_addr: IpAddr = exit_ip.parse()?;
                        asn_db.lookup(exit_ip_addr)?.decode()?
                    } else {
                        None
                    }
                } else {
                    None
                },
                geolocation: if let Some(geo_db) = &maybe_geo_db {
                    if let Some(exit_ip) = &proxy.exit_ip {
                        let exit_ip_addr: IpAddr = exit_ip.parse()?;
                        geo_db.lookup(exit_ip_addr)?.decode()?
                    } else {
                        None
                    }
                } else {
                    None
                },
            });
        }

        for (path, pretty) in [
            (config.output.path.join("proxies.json"), false),
            (config.output.path.join("proxies_pretty.json"), true),
        ] {
            let json_data = if pretty {
                serde_json::to_vec_pretty(&proxy_dicts)?
            } else {
                serde_json::to_vec(&proxy_dicts)?
            };
            tokio::fs::write(&path, json_data).await.wrap_err_with(
                move || {
                    compact_str::format_compact!(
                        "failed to write to file: {}",
                        path.display()
                    )
                },
            )?;
        }
    }

    if config.output.txt.enabled {
        let grouped_proxies = group_proxies(&config, &proxies);
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

        write_proxy_list_to_file(
            &directory_path.join("all.txt"),
            proxies.iter(),
            true,
        )
        .await?;

        for (proto, proxies) in grouped_proxies {
            let mut file_path = directory_path.join(proto.as_str_lowercase());
            file_path.set_extension("txt");
            write_proxy_list_to_file(&file_path, proxies, false).await?;
        }
    }

    drop(proxies);

    let path = tokio::fs::canonicalize(&config.output.path)
        .await
        .unwrap_or_else(move |_| config.output.path.clone());
    if is_docker().await {
        tracing::info!(
            "Proxies have been saved to ./out ({} in container)",
            path.display()
        );
    } else {
        tracing::info!("Proxies have been saved to {}", path.display());
    }

    Ok(())
}
