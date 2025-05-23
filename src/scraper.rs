use std::{collections::HashSet, sync::Arc};

use color_eyre::eyre::{OptionExt as _, WrapErr as _};

#[cfg(feature = "tui")]
use crate::event::{AppEvent, Event};
use crate::{
    config::Config,
    parsers::PROXY_REGEX,
    proxy::{Proxy, ProxyType},
    utils::{is_http_url, pretty_error},
};

async fn fetch_text(
    config: &Config,
    http_client: reqwest::Client,
    source: &str,
) -> color_eyre::Result<String> {
    Ok(if is_http_url(source) {
        http_client
            .get(source)
            .timeout(config.scraping.timeout)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?
    } else {
        tokio::fs::read_to_string(
            source.strip_prefix("file://").unwrap_or(source),
        )
        .await?
    })
}

async fn scrape_one(
    config: Arc<Config>,
    http_client: reqwest::Client,
    proto: ProxyType,
    proxies: Arc<tokio::sync::Mutex<HashSet<Proxy>>>,
    source: &str,
    #[cfg(feature = "tui")] tx: tokio::sync::mpsc::UnboundedSender<Event>,
) -> color_eyre::Result<()> {
    let text_result = fetch_text(&config, http_client.clone(), source).await;

    #[cfg(feature = "tui")]
    tx.send(Event::App(AppEvent::SourceScraped(proto.clone())))?;

    let text = match text_result {
        Ok(text) => text,
        Err(e) => {
            log::warn!("{} | {}", source, pretty_error(&e));
            return Ok(());
        }
    };

    let matches =
        PROXY_REGEX.captures_iter(&text).collect::<Result<Vec<_>, _>>()?;

    if matches.is_empty() {
        log::warn!("{source} | No proxies found");
        return Ok(());
    }

    if config.scraping.max_proxies_per_source != 0
        && matches.len() > config.scraping.max_proxies_per_source
    {
        log::warn!(
            "{} | Too many proxies ({}) - skipped",
            source,
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
                    .map(move |m| m.as_str().to_owned()),
                password: capture
                    .name("password")
                    .map(move |m| m.as_str().to_owned()),
                timeout: None,
                exit_ip: None,
            });
        }
    }
    #[cfg(feature = "tui")]
    for proto in seen_protocols {
        let count = proxies.iter().filter(|p| p.protocol == proto).count();
        tx.send(Event::App(AppEvent::TotalProxies(proto, count)))?;
    }
    drop(proxies);
    Ok(())
}

pub async fn scrape_all(
    config: Arc<Config>,
    http_client: reqwest::Client,
    #[cfg(feature = "tui")] tx: tokio::sync::mpsc::UnboundedSender<Event>,
) -> color_eyre::Result<HashSet<Proxy>> {
    let proxies = Arc::new(tokio::sync::Mutex::new(HashSet::new()));

    let mut join_set = tokio::task::JoinSet::new();
    for (proto, sources) in config.scraping.sources.clone() {
        #[cfg(feature = "tui")]
        tx.send(Event::App(AppEvent::SourcesTotal(
            proto.clone(),
            sources.len(),
        )))?;
        for source in sources {
            let config = Arc::clone(&config);
            let http_client = http_client.clone();
            let proto = proto.clone();
            let proxies = Arc::clone(&proxies);
            #[cfg(feature = "tui")]
            let tx = tx.clone();
            join_set.spawn(async move {
                scrape_one(
                    config,
                    http_client,
                    proto,
                    proxies,
                    &source,
                    #[cfg(feature = "tui")]
                    tx,
                )
                .await
            });
        }
    }

    while let Some(res) = join_set.join_next().await {
        res.wrap_err("failed to join proxy scrape task")?
            .wrap_err("proxy scrape task failed")?;
    }

    Ok(Arc::into_inner(proxies)
        .ok_or_eyre("failed to unwrap Arc")?
        .into_inner())
}
