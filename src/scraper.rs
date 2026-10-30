use std::sync::Arc;

use color_eyre::eyre::{OptionExt as _, WrapErr as _};
use compact_str::ToCompactString as _;
use rapidhash::HashSetExt as _;

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
    counts: crate::proxy::PerProxyType<usize>,
}

pub async fn scrape_all(
    config: Arc<Config>,
    http_client: reqwest_middleware::ClientWithMiddleware,
    token: tokio_util::sync::CancellationToken,
    #[cfg(feature = "tui")] tx: tokio::sync::mpsc::UnboundedSender<Event>,
) -> crate::Result<Vec<Proxy>> {
    let proxies = Arc::new(parking_lot::Mutex::new(ScrapedProxies::default()));

    let mut join_set = tokio::task::JoinSet::new();
    for proto in config.scraping.enabled_protocols.iter() {
        let sources = config.scraping.sources.get(proto);

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

async fn fetch_source(
    http_client: reqwest_middleware::ClientWithMiddleware,
    source: &Source,
) -> crate::Result<String> {
    let Ok(url) = url::Url::parse(&source.url) else {
        drop(http_client);
        return read_source_file(source.url.as_str().into()).await;
    };

    if !matches!(url.scheme(), "http" | "https") {
        drop(http_client);
        let path =
            url.to_file_path().unwrap_or_else(|()| source.url.as_str().into());
        return read_source_file(path).await;
    }

    let mut request = http_client.get(url);
    drop(http_client);

    if let Some(auth) = &source.basic_auth {
        request = request.basic_auth(&auth.username, auth.password.as_ref());
    }
    if let Some(headers) = &source.headers {
        for (k, v) in headers {
            request = request.header(k.as_bytes(), v.as_bytes());
        }
    }

    request.send().await?.text().await.map_err(Into::into)
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
    let text_result = fetch_source(http_client, &source).await;

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

            // Checked after inserting, so duplicates and CIDR ranges count as
            // the unique proxies they add.
            if config.scraping.max_proxies_per_source != 0
                && new_proxies.len() > config.scraping.max_proxies_per_source
            {
                tracing::warn!(
                    "{}: too many proxies (> {}) - skipped",
                    source.url,
                    config.scraping.max_proxies_per_source
                );
                return Ok(());
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
            let count = proxies.counts.get_mut(proxy.protocol);
            *count = count.saturating_add(1);
        }

        proxies.proxies = new_proxies;
    } else {
        // extend() reserves from the iterator's size hint; inserting one at a
        // time does not, and this runs while the shared lock is held.
        #[cfg(feature = "tui")]
        proxies.proxies.reserve(new_proxies.len());

        #[cfg(feature = "tui")]
        for proxy in new_proxies {
            let protocol = proxy.protocol;
            if proxies.proxies.insert(proxy) {
                let count = proxies.counts.get_mut(protocol);
                *count = count.saturating_add(1);
            }
        }

        #[cfg(not(feature = "tui"))]
        proxies.proxies.extend(new_proxies);
    }

    #[cfg(feature = "tui")]
    let counts = proxies.counts;

    drop(proxies);

    #[cfg(feature = "tui")]
    for proto in seen_protocols.iter() {
        drop(tx.send(Event::App(AppEvent::TotalProxies(
            proto,
            *counts.get(proto),
        ))));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use compact_str::CompactString;

    use super::{Proxy, scrape_all};
    use crate::{
        config::tests::{temp_directory, test_config},
        http::create_reqwest_client,
        raw_config::SourceConfig,
    };

    async fn scrape(name: &str, text: &str, limit: usize) -> Vec<Proxy> {
        let directory = temp_directory(name);
        let path = directory.join("source.txt");
        std::fs::write(&path, text).unwrap();

        let config = std::sync::Arc::new(
            test_config(|raw| {
                raw.scraping.max_proxies_per_source = limit;
                raw.scraping.http.enabled = true;
                raw.scraping.http.urls =
                    vec![SourceConfig::Simple(path.to_str().unwrap().into())];
                raw.scraping.socks4.enabled = false;
                raw.scraping.socks5.enabled = true;
                raw.scraping.socks5.urls = Vec::new();
            })
            .await,
        );
        let client = create_reqwest_client(&config).await.unwrap();

        let proxies = scrape_all(
            config,
            client,
            tokio_util::sync::CancellationToken::new(),
            #[cfg(feature = "tui")]
            tokio::sync::mpsc::unbounded_channel().0,
        )
        .await
        .unwrap();

        std::fs::remove_dir_all(directory).unwrap();
        proxies
    }

    fn sorted(proxies: &[Proxy]) -> Vec<CompactString> {
        let mut lines: Vec<_> =
            proxies.iter().map(|p| p.to_compact_string(true)).collect();
        lines.sort_unstable();
        lines
    }

    #[tokio::test]
    async fn the_scheme_in_a_source_overrides_its_section() {
        let proxies = scrape(
            "scheme",
            "1.1.1.1:80 socks5://2.2.2.2:80 socks4://3.3.3.3:80",
            0,
        )
        .await;
        assert_eq!(
            sorted(&proxies),
            ["http://1.1.1.1:80", "socks5://2.2.2.2:80"]
        );
    }

    #[tokio::test]
    async fn cidr_expands_to_hosts_and_keeps_credentials() {
        let proxies = scrape("cidr", "user:pass@1.1.1.0/30:80", 0).await;
        assert_eq!(
            sorted(&proxies),
            ["http://user:pass@1.1.1.1:80", "http://user:pass@1.1.1.2:80"]
        );
    }

    #[tokio::test]
    async fn limit_counts_unique_proxies_and_cidr_hosts() {
        let text = "1.1.1.1:80 1.1.1.1:80 2.2.2.2:80";
        assert_eq!(scrape("limit-at", text, 2).await.len(), 2);
        assert_eq!(scrape("limit-over", text, 1).await.len(), 0);
        assert_eq!(scrape("limit-cidr", "1.1.1.0/30:80", 1).await.len(), 0);
    }
}
