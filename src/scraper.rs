use std::{collections::HashSet, sync::Arc};

use color_eyre::eyre::{OptionExt as _, WrapErr as _};

#[cfg(feature = "tui")]
use crate::event::{AppEvent, Event};
use crate::{
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
    proxies: Arc<tokio::sync::Mutex<HashSet<Proxy>>>,
    source: Source,
    #[cfg(feature = "tui")] tx: tokio::sync::mpsc::UnboundedSender<Event>,
) -> color_eyre::Result<()> {
    let text_result = if let Ok(u) = url::Url::parse(&source.url) {
        match u.scheme() {
            "http" | "https" => {
                http::fetch_text(
                    http_client,
                    u,
                    source.basic_auth,
                    source.headers,
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
    drop(tx.send(Event::App(AppEvent::SourceScraped(proto.clone()))));

    let text = match text_result {
        Ok(text) => text,
        Err(e) => {
            tracing::warn!("{} | {}", source.url, pretty_error(&e));
            return Ok(());
        }
    };

    let matches =
        PROXY_REGEX.captures_iter(&text).collect::<Result<Vec<_>, _>>()?;

    if matches.is_empty() {
        tracing::warn!("{} | No proxies found", source.url);
        return Ok(());
    }

    if config.scraping.max_proxies_per_source != 0
        && matches.len() > config.scraping.max_proxies_per_source
    {
        tracing::warn!(
            "{} | Too many proxies ({}) - skipped",
            source.url,
            matches.len(),
        );
        return Ok(());
    }

    #[cfg(feature = "tui")]
    let mut seen_protocols = HashSet::new();
    let mut proxies = proxies.lock().await;
    for capture in matches {
        let protocol = match capture.name("protocol") {
            Some(m) => m.as_str().parse()?,
            None => proto.clone(),
        };
        if config.protocol_is_enabled(&protocol) {
            #[cfg(feature = "tui")]
            seen_protocols.insert(protocol.clone());
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
        let count = proxies.iter().filter(|p| p.protocol == proto).count();
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
) -> color_eyre::Result<Vec<Proxy>> {
    let proxies = Arc::new(tokio::sync::Mutex::new(HashSet::new()));

    let mut join_set = tokio::task::JoinSet::new();
    for (proto, sources) in &config.scraping.sources {
        #[cfg(feature = "tui")]
        drop(tx.send(Event::App(AppEvent::SourcesTotal(
            proto.clone(),
            sources.len(),
        ))));
        for source in sources.iter().cloned() {
            let config = Arc::clone(&config);
            let http_client = http_client.clone();
            let proto = proto.clone();
            let proxies = Arc::clone(&proxies);
            let token = token.clone();
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
                    () = token.cancelled() => Ok(())
                }
            });
        }
    }

    while let Some(res) = join_set.join_next().await {
        res.wrap_err("proxy scraping task panicked or was cancelled")?
            .wrap_err("proxy scraping task failed")?;
    }

    Ok(Arc::into_inner(proxies)
        .ok_or_eyre("failed to unwrap Arc")?
        .into_inner()
        .into_iter()
        .collect())
}
