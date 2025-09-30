use std::{
    cmp::Ordering,
    io,
    net::{IpAddr, Ipv4Addr},
    sync::Arc,
    time::Duration,
};

use color_eyre::eyre::WrapErr as _;
use itertools::Itertools as _;

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

#[derive(serde::Serialize)]
struct ProxyJson<'a> {
    protocol: ProxyType,
    username: Option<&'a str>,
    password: Option<&'a str>,
    host: &'a str,
    port: u16,
    timeout: Option<f64>,
    exit_ip: Option<&'a str>,
    asn: Option<maxminddb::geoip2::Asn<'a>>,
    geolocation: Option<maxminddb::geoip2::City<'a>>,
}

fn group_proxies<'a>(
    config: &Config,
    proxies: &'a [Proxy],
) -> HashMap<ProxyType, Vec<&'a Proxy>> {
    let mut groups: HashMap<_, _> =
        config.enabled_protocols().copied().map(|p| (p, Vec::new())).collect();
    for proxy in proxies {
        if let Some(proxies) = groups.get_mut(&proxy.protocol) {
            proxies.push(proxy);
        }
    }
    groups
}

#[expect(clippy::too_many_lines)]
pub async fn save_proxies(
    config: Arc<Config>,
    mut proxies: Vec<Proxy>,
) -> crate::Result<()> {
    if config.output.sort_by_speed {
        proxies.sort_unstable_by(compare_timeout);
    } else {
        proxies.sort_unstable_by(compare_natural);
    }

    if config.output.json.enabled {
        let (maybe_asn_db, maybe_geo_db) = tokio::try_join!(
            async {
                if config.output.json.include_asn {
                    ipdb::DbType::Asn.open_mmap().await.map(Some)
                } else {
                    Ok(None)
                }
            },
            async {
                if config.output.json.include_geolocation {
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
                    if let Some(exit_ip) = proxy.exit_ip.as_ref() {
                        let exit_ip_addr: IpAddr = exit_ip.parse()?;
                        asn_db.lookup::<maxminddb::geoip2::Asn<'_>>(
                            exit_ip_addr,
                        )?
                    } else {
                        None
                    }
                } else {
                    None
                },
                geolocation: if let Some(geo_db) = &maybe_geo_db {
                    if let Some(exit_ip) = proxy.exit_ip.as_ref() {
                        let exit_ip_addr: IpAddr = exit_ip.parse()?;
                        geo_db.lookup::<maxminddb::geoip2::City<'_>>(
                            exit_ip_addr,
                        )?
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
            match tokio::fs::remove_file(&path).await {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e).wrap_err_with(|| {
                    format!("failed to remove file: {}", path.display())
                }),
            }?;
            let json_data = if pretty {
                serde_json::to_vec_pretty(&proxy_dicts)?
            } else {
                serde_json::to_vec(&proxy_dicts)?
            };
            tokio::fs::write(&path, json_data).await.wrap_err_with(
                move || format!("failed to write to file: {}", path.display()),
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
                format!(
                    "failed to remove directory: {}",
                    directory_path.display()
                )
            }),
        }?;
        tokio::fs::create_dir_all(&directory_path).await.wrap_err_with(
            || {
                format!(
                    "failed to create directory: {}",
                    directory_path.display()
                )
            },
        )?;

        let text = create_proxy_list_str(proxies.iter(), true);
        tokio::fs::write(directory_path.join("all.txt"), text)
            .await
            .wrap_err_with(|| {
                format!(
                    "failed to write to file: {}",
                    directory_path.join("all.txt").display()
                )
            })?;

        for (proto, proxies) in grouped_proxies {
            let text = create_proxy_list_str(proxies, false);
            let mut file_path = directory_path.join(proto.as_str());
            file_path.set_extension("txt");
            tokio::fs::write(&file_path, text).await.wrap_err_with(
                move || {
                    format!("failed to write to file: {}", file_path.display())
                },
            )?;
        }
    }

    let path = config
        .output
        .path
        .canonicalize()
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

fn create_proxy_list_str<'a, I>(proxies: I, include_protocol: bool) -> String
where
    I: IntoIterator<Item = &'a Proxy>,
{
    proxies
        .into_iter()
        .map(move |proxy| proxy.to_string(include_protocol))
        .join("\n")
}
