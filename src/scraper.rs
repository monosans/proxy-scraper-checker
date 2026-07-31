use std::sync::Arc;

use color_eyre::eyre::{OptionExt as _, WrapErr as _};
use compact_str::ToCompactString as _;
use rapidhash::HashSetExt as _;
#[cfg(feature = "tui")]
use strum::EnumCount as _;

#[cfg(feature = "tui")]
use crate::event::{AppEvent, Event};
use crate::{
    HashSet,
    config::{Config, Source},
    parsers::proxy_matches,
    proxy::{Proxy, ProxyAuth, ProxyType},
    utils::pretty_error,
};

#[derive(Default)]
struct ScrapedProxies {
    proxies: HashSet<Proxy>,
    #[cfg(feature = "tui")]
    counts: crate::HashMap<ProxyType, usize>,
}

pub async fn scrape_all(
    config: Arc<Config>,
    http_client: reqwest_middleware::ClientWithMiddleware,
    token: tokio_util::sync::CancellationToken,
    #[cfg(feature = "tui")] tx: tokio::sync::mpsc::UnboundedSender<Event>,
) -> crate::Result<Vec<Proxy>> {
    let proxies = Arc::new(parking_lot::Mutex::new(ScrapedProxies::default()));

    let mut join_set = tokio::task::JoinSet::new();
    for (&proto, sources) in &config.scraping.sources {
        #[cfg(feature = "tui")]
        drop(tx.send(Event::App(AppEvent::SourcesTotal(proto, sources.len()))));

        for source in sources {
            let config = Arc::clone(&config);
            let http_client = http_client.clone();
            let proxies = Arc::clone(&proxies);
            let token = token.clone();
            let source = Arc::clone(source);
            #[cfg(feature = "tui")]
            let tx = tx.clone();
            join_set.spawn(async move {
                let scrape = scrape_one(
                    config,
                    http_client,
                    proto,
                    proxies,
                    source,
                    #[cfg(feature = "tui")]
                    tx,
                );

                tokio::select! {
                    biased;
                    res = scrape => res,
                    () = token.cancelled() => Ok(()),
                }
            });
        }
    }

    drop(config);
    drop(http_client);
    drop(token);
    #[cfg(feature = "tui")]
    drop(tx);

    while let Some(res) = join_set.join_next().await {
        res??;
    }

    Ok(Arc::into_inner(proxies)
        .ok_or_eyre("failed to unwrap Arc")?
        .into_inner()
        .proxies
        .into_iter()
        .collect())
}

async fn read_source_file(path: std::path::PathBuf) -> crate::Result<String> {
    tokio::fs::read_to_string(&path).await.wrap_err_with(move || {
        compact_str::format_compact!(
            "failed to read file to string: {}",
            path.display()
        )
    })
}

async fn scrape_one(
    config: Arc<Config>,
    http_client: reqwest_middleware::ClientWithMiddleware,
    proto: ProxyType,
    proxies: Arc<parking_lot::Mutex<ScrapedProxies>>,
    source: Arc<Source>,
    #[cfg(feature = "tui")] tx: tokio::sync::mpsc::UnboundedSender<Event>,
) -> crate::Result<()> {
    let text_result = if let Ok(u) = url::Url::parse(&source.url) {
        match u.scheme() {
            "http" | "https" => {
                let mut request = http_client.get(u);
                drop(http_client);

                if let Some(auth) = &source.basic_auth {
                    request = request
                        .basic_auth(&auth.username, auth.password.as_ref());
                }

                if let Some(headers) = &source.headers {
                    for (k, v) in headers {
                        request = request.header(k.as_bytes(), v.as_bytes());
                    }
                }

                match request.send().await {
                    Ok(resp) => resp.text().await.map_err(Into::into),
                    Err(e) => Err(e.into()),
                }
            }
            _ => {
                drop(http_client);
                let path = u
                    .to_file_path()
                    .unwrap_or_else(|()| source.url.as_str().into());
                read_source_file(path).await
            }
        }
    } else {
        drop(http_client);
        read_source_file(source.url.as_str().into()).await
    };

    #[cfg(feature = "tui")]
    drop(tx.send(Event::App(AppEvent::SourceScraped(proto))));

    let text = match text_result {
        Ok(text) => text,
        Err(e) => {
            tracing::warn!("{}: {}", source.url, pretty_error(&e));
            return Ok(());
        }
    };

    #[cfg(feature = "tui")]
    let mut seen_protocols = crate::proxy::ProxyTypeSet::default();

    let mut new_proxies = HashSet::new();

    for proxy_match in proxy_matches(&text) {
        if config.scraping.max_proxies_per_source != 0
            && new_proxies.len() >= config.scraping.max_proxies_per_source
        {
            tracing::warn!(
                "{}: too many proxies (> {}) - skipped",
                source.url,
                config.scraping.max_proxies_per_source
            );
            return Ok(());
        }

        let protocol = match proxy_match.protocol {
            Some(s) => s.parse()?,
            None => proto,
        };

        if config.protocol_is_enabled(protocol) {
            #[cfg(feature = "tui")]
            seen_protocols.insert(protocol);

            let port = proxy_match.port.parse()?;
            let auth = match (proxy_match.username, proxy_match.password) {
                (Some(username), Some(password)) => Some(Box::new(ProxyAuth {
                    username: username.into(),
                    password: password.into(),
                })),
                _ => None,
            };

            if proxy_match.host_cidr.as_bytes().contains(&b'/')
                && let Ok(ipv4_net) =
                    proxy_match.host_cidr.parse::<ipnet::Ipv4Net>()
            {
                new_proxies.extend(ipv4_net.hosts().map(move |host_ip| {
                    Proxy {
                        protocol,
                        host: host_ip.to_compact_string(),
                        port,
                        auth: auth.clone(),
                        timeout: None,
                        exit_ip: None,
                    }
                }));
            } else {
                new_proxies.insert(Proxy {
                    protocol,
                    host: proxy_match.host.into(),
                    port,
                    auth,
                    timeout: None,
                    exit_ip: None,
                });
            }
        }
    }

    drop(config);
    drop(text);

    if new_proxies.is_empty() {
        tracing::warn!("{}: no proxies found", source.url);
        return Ok(());
    }

    drop(source);

    let mut proxies = proxies.lock();

    if proxies.proxies.is_empty() {
        #[cfg(feature = "tui")]
        for proxy in &new_proxies {
            proxies
                .counts
                .entry(proxy.protocol)
                .and_modify(|c| *c = c.saturating_add(1))
                .or_insert(1);
        }

        proxies.proxies = new_proxies;
    } else {
        #[cfg(feature = "tui")]
        for proxy in new_proxies {
            let protocol = proxy.protocol;
            if proxies.proxies.insert(proxy) {
                proxies
                    .counts
                    .entry(protocol)
                    .and_modify(|c| *c = c.saturating_add(1))
                    .or_insert(1);
            }
        }

        #[cfg(not(feature = "tui"))]
        proxies.proxies.extend(new_proxies);
    }

    #[cfg(feature = "tui")]
    let totals: smallvec::SmallVec<[_; ProxyType::COUNT]> = seen_protocols
        .iter()
        .map(|proto| {
            (proto, proxies.counts.get(&proto).copied().unwrap_or_default())
        })
        .collect();

    drop(proxies);

    #[cfg(feature = "tui")]
    for (proto, count) in totals {
        drop(tx.send(Event::App(AppEvent::TotalProxies(proto, count))));
    }

    Ok(())
}
