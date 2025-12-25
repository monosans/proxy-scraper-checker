use std::sync::Arc;

use color_eyre::eyre::{OptionExt as _, WrapErr as _};
use foldhash::HashSetExt as _;

#[cfg(feature = "tui")]
use crate::event::{AppEvent, Event};
use crate::{
    HashSet,
    config::{Config, Source},
    parsers::PROXY_REGEX,
    proxy::{Proxy, ProxyType},
    utils::pretty_error,
};

async fn scrape_one(
    config: Arc<Config>,
    http_client: reqwest_middleware::ClientWithMiddleware,
    proto: ProxyType,
    proxies: Arc<parking_lot::Mutex<HashSet<Proxy>>>,
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
                match u.to_file_path() {
                    Ok(path) => tokio::fs::read_to_string(path)
                        .await
                        .wrap_err_with(move || {
                            compact_str::format_compact!(
                                "failed to read file to string: {u}"
                            )
                        }),
                    Err(()) => tokio::fs::read_to_string(&source.url)
                        .await
                        .wrap_err_with(move || {
                            compact_str::format_compact!(
                                "failed to read file to string: {u}"
                            )
                        }),
                }
            }
        }
    } else {
        drop(http_client);
        tokio::fs::read_to_string(&source.url).await.wrap_err_with(|| {
            compact_str::format_compact!(
                "failed to read file to string: {}",
                source.url
            )
        })
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
    let mut seen_protocols = HashSet::new();

    let mut new_proxies = HashSet::new();

    for maybe_capture in PROXY_REGEX.captures_iter(&text) {
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

        let capture = maybe_capture?;

        let protocol = match capture.name("protocol") {
            Some(m) => m.as_str().parse()?,
            None => proto,
        };

        if config.protocol_is_enabled(protocol) {
            #[cfg(feature = "tui")]
            seen_protocols.insert(protocol);

            new_proxies.insert(Proxy {
                protocol,
                host: capture
                    .name("host")
                    .ok_or_eyre("failed to match \"host\" regex capture group")?
                    .as_str()
                    .into(),
                port: capture
                    .name("port")
                    .ok_or_eyre("failed to match \"port\" regex capture group")?
                    .as_str()
                    .parse()?,
                username: capture.name("username").map(|m| m.as_str().into()),
                password: capture.name("password").map(|m| m.as_str().into()),
                timeout: None,
                exit_ip: None,
            });
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
    proxies.extend(new_proxies);

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
    http_client: reqwest_middleware::ClientWithMiddleware,
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
    #[cfg(feature = "tui")]
    drop(tx);

    while let Some(res) = join_set.join_next().await {
        res??;
    }

    drop(join_set);

    Ok(Arc::into_inner(proxies)
        .ok_or_eyre("failed to unwrap Arc")?
        .into_inner()
        .into_iter()
        .collect())
}
