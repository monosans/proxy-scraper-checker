use std::{collections::HashSet, sync::Arc};

use color_eyre::eyre::{OptionExt as _, WrapErr as _};

#[cfg(feature = "tui")]
use crate::event::{AppEvent, Event};
use crate::{
    config::Config,
    parsers::PROXY_REGEX,
    proxy::{Proxy, ProxyType},
    storage::ProxyStorage,
    utils::{is_http_url, pretty_error},
};

async fn fetch_text(
    config: Arc<Config>,
    http_client: reqwest::Client,
    source: &str,
) -> color_eyre::Result<String> {
    if is_http_url(source) {
        http_client
            .get(source)
            .timeout(config.source_timeout)
            .send()
            .await
            .wrap_err_with(move || {
                format!("failed to send request to {source}")
            })?
            .error_for_status()
            .wrap_err_with(move || {
                format!("got error status code from {source}")
            })?
            .text()
            .await
            .wrap_err_with(move || {
                format!("failed to decode {source} response as text")
            })
    } else {
        tokio::fs::read_to_string(
            source.strip_prefix("file://").unwrap_or(source),
        )
        .await
        .wrap_err("failed to read file to string")
    }
}

async fn scrape_one(
    config: Arc<Config>,
    http_client: reqwest::Client,
    proto: ProxyType,
    source: &str,
    #[cfg(feature = "tui")] tx: tokio::sync::mpsc::UnboundedSender<Event>,
) -> color_eyre::Result<HashSet<Proxy>> {
    let text_result =
        fetch_text(Arc::clone(&config), http_client.clone(), source).await;

    #[cfg(feature = "tui")]
    tx.send(Event::App(AppEvent::SourceScraped(proto.clone())))?;

    let text = match text_result {
        Ok(text) => text,
        Err(e) => {
            log::warn!("{} | {}", source, pretty_error(&e));
            return Ok(HashSet::new());
        }
    };

    let matches: Vec<_> = PROXY_REGEX.captures_iter(&text).collect();

    if matches.is_empty() {
        log::warn!("{source} | No proxies found");
        return Ok(HashSet::new());
    }

    if config.proxies_per_source_limit != 0
        && matches.len() > config.proxies_per_source_limit
    {
        log::warn!(
            "{} | Too many proxies ({}) - skipped",
            source,
            matches.len(),
        );
        return Ok(HashSet::new());
    }

    let mut proxies = HashSet::with_capacity(matches.len());
    for capture in matches {
        let capture =
            capture.wrap_err("failed to match regex captures groups")?;
        let proxy = Proxy {
            protocol: match capture.name("protocol") {
                Some(m) => m.as_str().try_into()?,
                None => proto.clone(),
            },
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
            username: capture.name("username").map(|m| m.as_str().to_owned()),
            password: capture.name("password").map(|m| m.as_str().to_owned()),
            timeout: None,
            exit_ip: None,
        };
        proxies.insert(proxy);
    }
    Ok(proxies)
}

pub async fn scrape_all(
    config: Arc<Config>,
    http_client: reqwest::Client,
    #[cfg(feature = "tui")] tx: tokio::sync::mpsc::UnboundedSender<Event>,
) -> color_eyre::Result<ProxyStorage> {
    let mut join_set = tokio::task::JoinSet::new();
    for (proto, sources) in config.sources.clone() {
        #[cfg(feature = "tui")]
        tx.send(Event::App(AppEvent::SourcesTotal(
            proto.clone(),
            sources.len(),
        )))?;
        for source in sources {
            let config = Arc::clone(&config);
            let http_client = http_client.clone();
            let proto = proto.clone();
            #[cfg(feature = "tui")]
            let tx = tx.clone();
            join_set.spawn(async move {
                scrape_one(
                    config,
                    http_client,
                    proto,
                    &source,
                    #[cfg(feature = "tui")]
                    tx,
                )
                .await
            });
        }
    }

    let mut storage =
        ProxyStorage::new(config.sources.keys().cloned().collect());
    while let Some(res) = join_set.join_next().await {
        #[cfg(feature = "tui")]
        let mut seen_protocols = HashSet::new();
        for proxy in res
            .wrap_err("failed to join proxy scrape task")?
            .wrap_err("proxy scrape task failed")?
        {
            #[cfg(feature = "tui")]
            seen_protocols.insert(proxy.protocol.clone());
            storage.insert(proxy);
        }
        #[cfg(feature = "tui")]
        for proto in seen_protocols {
            let count = storage.iter().filter(|p| p.protocol == proto).count();
            tx.send(Event::App(AppEvent::TotalProxies(proto, count)))?;
        }
    }
    Ok(storage)
}
