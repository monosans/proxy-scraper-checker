use std::{
    collections::HashMap,
    io, iter,
    net::{IpAddr, Ipv4Addr},
    sync::Arc,
};

use color_eyre::eyre::WrapErr as _;

use crate::{
    config::Config,
    ipdb,
    proxy::{Proxy, ProxyType},
    utils::is_docker,
};

fn sort_by_timeout(proxy: &Proxy) -> tokio::time::Duration {
    proxy.timeout.unwrap_or(tokio::time::Duration::MAX)
}

fn sort_naturally(proxy: &Proxy) -> (ProxyType, Vec<u8>, u16) {
    let host_key = proxy.host.parse::<Ipv4Addr>().map_or_else(
        move |_| iter::repeat_n(u8::MAX, 4).chain(proxy.host.bytes()).collect(),
        |ip| ip.octets().to_vec(),
    );
    (proxy.protocol.clone(), host_key, proxy.port)
}

#[derive(serde::Serialize)]
struct ProxyJson<'a> {
    protocol: ProxyType,
    username: Option<String>,
    password: Option<String>,
    host: String,
    port: u16,
    timeout: Option<f64>,
    exit_ip: Option<String>,
    asn: Option<maxminddb::geoip2::Asn<'a>>,
    geolocation: Option<maxminddb::geoip2::City<'a>>,
}

fn group_proxies<'a>(
    config: &Config,
    proxies: &'a Vec<Proxy>,
) -> HashMap<ProxyType, Vec<&'a Proxy>> {
    let mut groups: HashMap<_, _> =
        config.enabled_protocols().map(|p| (p.clone(), Vec::new())).collect();
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
) -> color_eyre::Result<()> {
    if config.output.sort_by_speed {
        proxies.sort_by_key(sort_by_timeout);
    } else {
        proxies.sort_by_key(sort_naturally);
    }

    if config.output.json.enabled {
        let maybe_asn_db = if config.output.json.include_asn {
            let path = ipdb::DbType::Asn.db_path().await?;
            Some(
                tokio::task::spawn_blocking(move || {
                    maxminddb::Reader::open_mmap(path)
                })
                .await
                .wrap_err("failed to spawn tokio blocking task")?
                .wrap_err("failed to open ASN database")?,
            )
        } else {
            None
        };

        let maybe_geo_db = if config.output.json.include_geolocation {
            let path = ipdb::DbType::Geo.db_path().await?;
            Some(
                tokio::task::spawn_blocking(move || {
                    maxminddb::Reader::open_mmap(path)
                })
                .await
                .wrap_err("failed to spawn tokio blocking task")?
                .wrap_err("failed to open geolocation database")?,
            )
        } else {
            None
        };

        let mut proxy_dicts = Vec::with_capacity(proxies.len());
        for proxy in &proxies {
            proxy_dicts.push(ProxyJson {
                protocol: proxy.protocol.clone(),
                username: proxy.username.clone(),
                password: proxy.password.clone(),
                host: proxy.host.clone(),
                port: proxy.port,
                timeout: proxy
                    .timeout
                    .map(|d| (d.as_secs_f64() * 100.0).round() / 100.0_f64),
                exit_ip: proxy.exit_ip.clone(),
                asn: if let Some(asn_db) = &maybe_asn_db {
                    if let Some(exit_ip) = proxy.exit_ip.clone() {
                        let exit_ip_addr: IpAddr = exit_ip.parse().wrap_err(
                            "failed to parse proxy's exit ip as IpAddr",
                        )?;
                        asn_db
                            .lookup::<maxminddb::geoip2::Asn>(exit_ip_addr)
                            .wrap_err_with(move || {
                                format!(
                                    "failed to lookup {exit_ip_addr} in ASN \
                                     database"
                                )
                            })?
                    } else {
                        None
                    }
                } else {
                    None
                },
                geolocation: if let Some(geo_db) = &maybe_geo_db {
                    if let Some(exit_ip) = proxy.exit_ip.clone() {
                        let exit_ip_addr: IpAddr = exit_ip.parse().wrap_err(
                            "failed to parse proxy's exit ip as IpAddr",
                        )?;
                        geo_db
                            .lookup::<maxminddb::geoip2::City>(exit_ip_addr)
                            .wrap_err_with(move || {
                            format!(
                                "failed to lookup {exit_ip_addr} in \
                                 geolocation database"
                            )
                        })?
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
                    format!("failed to remove file {}", path.display())
                }),
            }?;
            let json_data = if pretty {
                serde_json::to_vec_pretty(&proxy_dicts)
                    .wrap_err("failed to serialize proxies to pretty json")?
            } else {
                serde_json::to_vec(&proxy_dicts)
                    .wrap_err("failed to serialize proxies to json")?
            };
            tokio::fs::write(&path, json_data).await.wrap_err_with(
                move || {
                    format!("failed to write proxies to {}", path.display())
                },
            )?;
        }
    }

    if config.output.txt.enabled {
        let grouped_proxies = group_proxies(&config, &proxies);

        for (anonymous_only, directory) in
            [(false, "proxies"), (true, "proxies_anonymous")]
        {
            let directory_path = config.output.path.join(directory);
            match tokio::fs::remove_dir_all(&directory_path).await {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e).wrap_err_with(|| {
                    format!(
                        "failed to remove directory {}",
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

            let text = create_proxy_list_str(
                &proxies.iter().collect(),
                anonymous_only,
                true,
            );
            tokio::fs::write(directory_path.join("all.txt"), text)
                .await
                .wrap_err_with(|| {
                    format!(
                        "failed to write proxies to {}",
                        directory_path.join("all.txt").display()
                    )
                })?;

            for (proto, proxies) in &grouped_proxies {
                let text =
                    create_proxy_list_str(proxies, anonymous_only, false);
                tokio::fs::write(
                    directory_path.join(format!("{proto}.txt")),
                    text,
                )
                .await
                .wrap_err_with(|| {
                    format!(
                        "failed to write proxies to {}",
                        directory_path.join(format!("{proto}.txt")).display()
                    )
                })?;
            }
        }
    }

    let path = config
        .output
        .path
        .canonicalize()
        .unwrap_or_else(move |_| config.output.path.clone());
    if is_docker().await {
        log::info!(
            "Proxies have been saved to ./out ({} in container)",
            path.display()
        );
    } else {
        log::info!("Proxies have been saved to {}", path.display());
    }

    Ok(())
}

fn create_proxy_list_str(
    proxies: &Vec<&Proxy>,
    anonymous_only: bool,
    include_protocol: bool,
) -> String {
    proxies
        .iter()
        .filter(move |proxy| {
            !anonymous_only
                || proxy
                    .exit_ip
                    .as_ref()
                    .is_some_and(move |ip| *ip != proxy.host)
        })
        .map(move |proxy| proxy.as_str(include_protocol))
        .collect::<Vec<_>>()
        .join("\n")
}
