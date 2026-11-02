use std::sync::Arc;

use color_eyre::eyre::{OptionExt as _, WrapErr as _, eyre};
use compact_str::ToCompactString as _;
use rapidhash::HashSetExt as _;

#[cfg(feature = "tui")]
use crate::event::{AppEvent, Event};
use crate::{
    HashSet,
    config::{Config, Source},
    parsers::proxy_matches,
    proxy::{Proxy, ProxyAuth, ProxyType},
    utils::{plural, pretty_error},
};

// Far more than a real list needs. It has to hold while reading: a compressed
// reply can unpack to more than fits in memory, long before anything can count
// its proxies.
const MAX_BYTES_PER_PROXY: usize = 1024;

#[derive(Default)]
struct ScrapedProxies {
    proxies: HashSet<Proxy>,
    #[cfg(feature = "tui")]
    counts: crate::proxy::PerProxyType<usize>,
}

impl ScrapedProxies {
    // Runs while the shared lock is held.
    #[cfg(feature = "tui")]
    fn add(&mut self, new_proxies: HashSet<Proxy>) {
        if self.proxies.is_empty() {
            for proxy in &new_proxies {
                let count = self.counts.get_mut(proxy.protocol);
                *count = count.saturating_add(1);
            }
            self.proxies = new_proxies;
            return;
        }

        // extend() reserves from the iterator's size hint; inserting one at a
        // time does not.
        self.proxies.reserve(new_proxies.len());
        for proxy in new_proxies {
            let protocol = proxy.protocol;
            if self.proxies.insert(proxy) {
                let count = self.counts.get_mut(protocol);
                *count = count.saturating_add(1);
            }
        }
    }

    #[cfg(not(feature = "tui"))]
    fn add(&mut self, new_proxies: HashSet<Proxy>) {
        if self.proxies.is_empty() {
            self.proxies = new_proxies;
        } else {
            self.proxies.extend(new_proxies);
        }
    }
}

pub async fn scrape_all(
    config: Arc<Config>,
    http_client: reqwest_middleware::ClientWithMiddleware,
    token: tokio_util::sync::CancellationToken,
    #[cfg(feature = "tui")] tx: tokio::sync::mpsc::UnboundedSender<Event>,
) -> crate::Result<Vec<Proxy>> {
    let sources_count: usize = config
        .scraping
        .enabled_protocols
        .iter()
        .map(|proto| config.scraping.sources.get(proto).len())
        .sum();
    tracing::info!(
        "Scraping {sources_count} {}",
        plural(sources_count, "source", "sources")
    );

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
    drop(join_set);

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
    max_proxies: usize,
) -> crate::Result<String> {
    let Ok(url) = url::Url::parse(&source.url) else {
        drop(http_client);
        return read_source_file(source.url.as_str().into()).await;
    };

    if !matches!(url.scheme(), "http" | "https") {
        drop(http_client);
        let path = url
            .to_file_path()
            .unwrap_or_else(move |()| source.url.as_str().into());
        drop(url);
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

    let max_bytes = if max_proxies == 0 {
        usize::MAX
    } else {
        max_proxies.saturating_mul(MAX_BYTES_PER_PROXY)
    };

    let mut response = request.send().await?;
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > max_bytes.saturating_sub(body.len()) {
            return Err(eyre!("response is over {max_bytes} bytes - skipped"));
        }
        body.extend_from_slice(&chunk);
    }
    drop(response);

    Ok(String::from_utf8(body)
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()))
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
    let text_result = fetch_source(
        http_client,
        &source,
        config.scraping.max_proxies_per_source,
    )
    .await;

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

    let mut scraped = proxies.lock();
    scraped.add(new_proxies);

    #[cfg(feature = "tui")]
    let counts = scraped.counts;

    drop(scraped);
    #[cfg(feature = "tui")]
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
    use std::sync::Arc;

    use compact_str::CompactString;
    use tokio_util::sync::CancellationToken;

    use super::{Proxy, scrape_all};
    use crate::{
        config::{
            Source,
            tests::{temp_directory, test_config},
        },
        http::{create_reqwest_client, tests::serve},
        raw_config::SourceConfig,
        utils::tests::capture_logs,
    };

    async fn scrape(name: &str, text: &str, limit: usize) -> Vec<Proxy> {
        let directory = temp_directory(name);
        let path = directory.join("source.txt");
        std::fs::write(&path, text).unwrap();
        let proxies = scrape_source(path.to_str().unwrap(), limit).await;
        drop(path);
        std::fs::remove_dir_all(directory).unwrap();
        proxies
    }

    async fn scrape_source(source: &str, limit: usize) -> Vec<Proxy> {
        scrape_sources(vec![SourceConfig::Simple(source.into())], limit).await
    }

    async fn scrape_sources(
        sources: Vec<SourceConfig>,
        limit: usize,
    ) -> Vec<Proxy> {
        let config = Arc::new(
            test_config(move |raw| {
                raw.scraping.max_proxies_per_source = limit;
                raw.scraping.http.enabled = true;
                raw.scraping.http.urls = sources;
                raw.scraping.socks4.enabled = false;
                raw.scraping.socks5.enabled = true;
                raw.scraping.socks5.urls = Vec::new();
            })
            .await,
        );
        let client = create_reqwest_client(&config).await.unwrap();
        #[cfg(feature = "tui")]
        let tx = tokio::sync::mpsc::unbounded_channel().0;

        scrape_all(
            config,
            client,
            CancellationToken::new(),
            #[cfg(feature = "tui")]
            tx,
        )
        .await
        .unwrap()
    }

    fn reply(body: &[u8]) -> Vec<u8> {
        let mut reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: \
             close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        reply.extend_from_slice(body);
        reply
    }

    #[tokio::test]
    async fn a_source_can_ask_for_credentials_and_headers() {
        let (address, requests) = serve(vec![reply(b"1.1.1.1:80")]).await;
        let source: Source = toml::from_str(&format!(
            r#"
            url = "http://{address}/list"
            basic_auth = {{ username = "user", password = "pass" }}
            headers = {{ X-Api-Key = "secret" }}
            "#
        ))
        .unwrap();

        let proxies =
            scrape_sources(vec![SourceConfig::Detailed(Box::new(source))], 0)
                .await;

        assert_eq!(sorted(&proxies), ["http://1.1.1.1:80"]);
        let request = requests.lock().pop().unwrap().to_ascii_lowercase();
        assert!(request.starts_with("get /list http/1.1\r\n"), "{request}");
        // user:pass
        assert!(
            request.contains("\r\nauthorization: basic dxnlcjpwyxnz\r\n"),
            "{request}"
        );
        assert!(request.contains("\r\nx-api-key: secret\r\n"), "{request}");
    }

    #[tokio::test]
    async fn a_source_that_is_not_utf8_still_gives_its_proxies() {
        let (address, _) =
            serve(vec![reply(b"\xff 1.1.1.1:80 \xfe 2.2.2.2:80")]).await;
        let proxies = scrape_source(&format!("http://{address}/"), 0).await;
        assert_eq!(
            sorted(&proxies),
            ["http://1.1.1.1:80", "http://2.2.2.2:80"]
        );
    }

    #[tokio::test]
    async fn a_source_can_be_a_file_url() {
        let directory = temp_directory("file-url");
        let path = directory.join("source.txt");
        std::fs::write(&path, "1.1.1.1:80").unwrap();
        let url = url::Url::from_file_path(&path).unwrap();
        drop(path);

        let proxies = scrape_source(url.as_str(), 0).await;

        assert_eq!(sorted(&proxies), ["http://1.1.1.1:80"]);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn sources_that_fail_or_have_nothing_only_get_a_warning() {
        let logs = capture_logs();
        let directory = temp_directory("failing-sources");
        let empty = directory.join("empty.txt");
        std::fs::write(&empty, "no proxies here").unwrap();
        let missing = directory.join("missing.txt");
        let crowded = directory.join("crowded.txt");
        std::fs::write(&crowded, "1.1.1.1:80 2.2.2.2:80").unwrap();
        let working = directory.join("working.txt");
        std::fs::write(&working, "3.3.3.3:80").unwrap();

        let sources = [&empty, &missing, &crowded, &working]
            .map(|path| SourceConfig::Simple(path.to_str().unwrap().into()));
        let proxies = scrape_sources(sources.into(), 1).await;

        assert_eq!(sorted(&proxies), ["http://3.3.3.3:80"]);
        let text = logs.text();
        assert!(text.contains("Scraping 4 sources"), "{text}");
        for (path, warning) in [
            (&empty, "no proxies found"),
            (&missing, "failed to read file to string: "),
            (&crowded, "too many proxies (> 1) - skipped"),
        ] {
            let path = path.to_str().unwrap();
            assert!(text.contains(&format!("{path}: {warning}")), "{text}");
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn an_unknown_url_scheme_is_tried_as_a_path() {
        let logs = capture_logs();
        let proxies = scrape_source("unknown://source", 0).await;
        assert!(proxies.is_empty());
        let text = logs.text();
        assert!(
            text.contains(
                "unknown://source: failed to read file to string: \
                 unknown://source"
            ),
            "{text}"
        );
    }

    #[tokio::test]
    async fn every_source_adds_to_one_list_without_duplicates() {
        let directory = temp_directory("several-sources");
        let mut sources = Vec::new();
        for (name, text) in [
            ("first.txt", "1.1.1.1:80 socks5://2.2.2.2:80"),
            ("second.txt", "1.1.1.1:80 3.3.3.3:80 socks5://2.2.2.2:80"),
            ("third.txt", "socks5://4.4.4.4:80"),
        ] {
            let path = directory.join(name);
            std::fs::write(&path, text).unwrap();
            sources.push(SourceConfig::Simple(path.to_str().unwrap().into()));
        }

        let config = Arc::new(
            test_config(move |raw| {
                raw.scraping.http.enabled = true;
                raw.scraping.http.urls = sources;
                raw.scraping.socks4.enabled = false;
                raw.scraping.socks5.enabled = true;
                raw.scraping.socks5.urls = Vec::new();
            })
            .await,
        );
        let client = create_reqwest_client(&config).await.unwrap();
        #[cfg(feature = "tui")]
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        let proxies = scrape_all(
            config,
            client,
            CancellationToken::new(),
            #[cfg(feature = "tui")]
            tx,
        )
        .await
        .unwrap();

        assert_eq!(
            sorted(&proxies),
            [
                "http://1.1.1.1:80",
                "http://3.3.3.3:80",
                "socks5://2.2.2.2:80",
                "socks5://4.4.4.4:80"
            ]
        );
        std::fs::remove_dir_all(directory).unwrap();

        // The totals only grow, and end at what was found.
        #[cfg(feature = "tui")]
        {
            use crate::{
                event::{AppEvent, Event},
                proxy::ProxyType,
            };

            let events: Vec<_> =
                std::iter::from_fn(|| rx.try_recv().ok()).collect();
            let of = |protocol: ProxyType| {
                let mut sources = None;
                let mut scraped = 0_usize;
                let mut totals = Vec::new();
                for event in &events {
                    if let Event::App(AppEvent::SourcesTotal(p, count)) = event
                        && *p == protocol
                    {
                        sources = Some(*count);
                    }
                    if let Event::App(AppEvent::SourceScraped(p)) = event
                        && *p == protocol
                    {
                        scraped = scraped.saturating_add(1);
                    }
                    if let Event::App(AppEvent::TotalProxies(p, count)) = event
                        && *p == protocol
                    {
                        totals.push(*count);
                    }
                }
                (sources, scraped, totals)
            };
            let (sources, scraped, totals) = of(ProxyType::Http);
            assert_eq!((sources, scraped), (Some(3), 3));
            assert!(totals.is_sorted(), "{totals:?}");
            assert_eq!(totals.last(), Some(&2));

            let (sources, scraped, totals) = of(ProxyType::Socks5);
            assert_eq!((sources, scraped), (Some(0), 0));
            assert!(totals.is_sorted(), "{totals:?}");
            assert_eq!(totals.last(), Some(&2));
        }
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
    async fn a_garbled_scheme_does_not_end_the_run() {
        let proxies =
            scrape("garbled", "\u{17f}ocks5://1.1.1.1:80 2.2.2.2:80", 0).await;
        assert_eq!(
            sorted(&proxies),
            ["http://1.1.1.1:80", "http://2.2.2.2:80"]
        );
    }

    #[tokio::test]
    async fn a_reply_too_big_for_the_limit_is_dropped_while_reading() {
        let body = "1.1.1.1:80\n".repeat(200);
        let reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: \
             close\r\n\r\n{body}",
            body.len()
        );
        drop(body);
        let reply: &'static str = reply.leak();

        let (address, _) = serve(vec![reply]).await;
        let url = format!("http://{address}/");
        assert_eq!(scrape_source(&url, 1).await.len(), 0);
        drop(url);

        let (address, _) = serve(vec![reply]).await;
        let url = format!("http://{address}/");
        assert_eq!(scrape_source(&url, 3).await.len(), 1);
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
