use std::sync::Arc;

use color_eyre::eyre::{OptionExt as _, WrapErr as _};
use foldhash::HashSetExt as _;

#[cfg(feature = "tui")]
use crate::event::{AppEvent, Event};
use crate::{
    HashSet,
    config::{Config, Source},
    http,
    parsers::PROXY_REGEX,
    proxy::{Proxy, ProxyType},
    utils::pretty_error,
};

async fn scrape_one(
    config: Arc<Config>,
    http_client: reqwest::Client,
    proto: ProxyType,
    proxies: Arc<parking_lot::Mutex<HashSet<Proxy>>>,
    source: Arc<Source>,
    #[cfg(feature = "tui")] tx: tokio::sync::mpsc::UnboundedSender<Event>,
) -> crate::Result<()> {
    let text_result = if let Ok(u) = url::Url::parse(&source.url) {
        match u.scheme() {
            "http" | "https" => {
                http::fetch_text(
                    http_client,
                    u,
                    source.basic_auth.as_ref(),
                    source.headers.as_ref(),
                )
                .await
            }
            _ => match u.to_file_path() {
                Ok(path) => tokio::fs::read_to_string(path).await,
                Err(()) => tokio::fs::read_to_string(&source.url).await,
            }
            .map_err(Into::into),
        }
    } else {
        tokio::fs::read_to_string(&source.url).await.map_err(Into::into)
    };

    #[cfg(feature = "tui")]
    drop(tx.send(Event::App(AppEvent::SourceScraped(proto))));

    let text = match text_result {
        Ok(text) => text,
        Err(e) => {
            tracing::warn!("{} | {}", source.url, pretty_error(&e));
            return Ok(());
        }
    };

    let mut matches = Vec::new();
    for (i, maybe_capture) in PROXY_REGEX.captures_iter(&text).enumerate() {
        if config.scraping.max_proxies_per_source != 0
            && i >= config.scraping.max_proxies_per_source
        {
            tracing::warn!(
                "{} | Too many proxies (> {}) - skipped",
                source.url,
                config.scraping.max_proxies_per_source
            );
            return Ok(());
        }
        matches.push(maybe_capture?);
    }

    if matches.is_empty() {
        tracing::warn!("{} | No proxies found", source.url);
        return Ok(());
    }

    #[cfg(feature = "tui")]
    let mut seen_protocols = HashSet::new();
    let mut proxies = proxies.lock();
    for capture in matches {
        let protocol = match capture.name("protocol") {
            Some(m) => m.as_str().parse()?,
            None => proto,
        };
        if config.protocol_is_enabled(protocol) {
            #[cfg(feature = "tui")]
            seen_protocols.insert(protocol);
            proxies.insert(Proxy {
                protocol,
                host: capture
                    .name("host")
                    .ok_or_eyre("failed to match \"host\" regex capture group")?
                    .as_str()
                    .to_owned(),
                port: capture
                    .name("port")
                    .ok_or_eyre("failed to match \"port\" regex capture group")?
                    .as_str()
                    .parse()?,
                username: capture
                    .name("username")
                    .map(|m| m.as_str().to_owned()),
                password: capture
                    .name("password")
                    .map(|m| m.as_str().to_owned()),
                timeout: None,
                exit_ip: None,
            });
        }
    }
    #[cfg(feature = "tui")]
    for proto in seen_protocols {
        let count = proxies.iter().filter(move |p| p.protocol == proto).count();
        drop(tx.send(Event::App(AppEvent::TotalProxies(proto, count))));
    }
    drop(proxies);
    Ok(())
}

pub async fn scrape_all(
    config: Arc<Config>,
    http_client: reqwest::Client,
    token: tokio_util::sync::CancellationToken,
    #[cfg(feature = "tui")] tx: tokio::sync::mpsc::UnboundedSender<Event>,
) -> crate::Result<Vec<Proxy>> {
    let proxies = Arc::new(parking_lot::Mutex::new(HashSet::new()));

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
                tokio::select! {
                    biased;
                    res = scrape_one(
                        config,
                        http_client,
                        proto,
                        proxies,
                        source,
                        #[cfg(feature = "tui")]
                        tx,
                    ) => res,
                    () = token.cancelled() => Ok(()),
                }
            });
        }
    }

    drop(config);
    drop(http_client);
    drop(token);
    drop(tx);

    while let Some(res) = join_set.join_next().await {
        res.wrap_err("proxy scraping task panicked or was cancelled")?
            .wrap_err("proxy scraping task failed")?;
    }

    drop(join_set);

    Ok(Arc::into_inner(proxies)
        .ok_or_eyre("failed to unwrap Arc")?
        .into_inner()
        .into_iter()
        .collect())
}
