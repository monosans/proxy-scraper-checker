use std::{
    collections::HashSet,
    sync::Arc,
    time::{Duration, SystemTime},
};

use color_eyre::eyre::{OptionExt as _, WrapErr as _};

#[cfg(feature = "tui")]
use crate::event::{AppEvent, Event};
use crate::{
    config::Config,
    parsers::PROXY_REGEX,
    proxy::{Proxy, ProxyType},
    utils::{is_http_url, pretty_error},
};

const DEFAULT_MAX_RETRIES: usize = 2;
const INITIAL_RETRY_DELAY: Duration = Duration::from_millis(500);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(8);

static RETRY_STATUSES: &[reqwest::StatusCode] = &[
    reqwest::StatusCode::REQUEST_TIMEOUT,
    reqwest::StatusCode::TOO_MANY_REQUESTS,
    reqwest::StatusCode::INTERNAL_SERVER_ERROR,
    reqwest::StatusCode::BAD_GATEWAY,
    reqwest::StatusCode::SERVICE_UNAVAILABLE,
    reqwest::StatusCode::GATEWAY_TIMEOUT,
];

fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    if let Some(val) = headers.get("retry-after-ms")
        && let Ok(s) = val.to_str()
        && let Ok(ms) = s.parse()
    {
        return Some(Duration::from_millis(ms));
    }

    if let Some(val) = headers.get(reqwest::header::RETRY_AFTER)
        && let Ok(s) = val.to_str()
    {
        if let Ok(sec) = s.parse() {
            return Some(Duration::from_secs(sec));
        }

        if let Ok(parsed) = httpdate::parse_http_date(s)
            && let Ok(dur) = parsed.duration_since(SystemTime::now())
        {
            return Some(dur);
        }
    }
    None
}

fn calculate_retry_timeout(
    headers: Option<&reqwest::header::HeaderMap>,
    attempt: usize,
) -> Option<Duration> {
    if let Some(h) = headers
        && let Some(after) = parse_retry_after(h)
    {
        if after > Duration::from_secs(60) {
            return None;
        }
        return Some(after);
    }

    let base = INITIAL_RETRY_DELAY
        .saturating_mul((2_u32).pow(u32::try_from(attempt).unwrap_or(u32::MAX)))
        .min(MAX_RETRY_DELAY);
    let jitter = 0.25_f64.mul_add(-rand::random::<f64>(), 1.0);
    Some(base.mul_f64(jitter))
}

async fn fetch_text(
    config: &Config,
    http_client: reqwest::Client,
    source: &str,
) -> color_eyre::Result<String> {
    if is_http_url(source) {
        let mut attempt = 0;
        loop {
            let req = http_client
                .get(source)
                .timeout(config.scraping.timeout)
                .send()
                .await;

            match req {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_client_error() || status.is_server_error() {
                        if attempt < DEFAULT_MAX_RETRIES
                            && RETRY_STATUSES.contains(&status)
                            && let Some(delay) = calculate_retry_timeout(
                                Some(resp.headers()),
                                attempt,
                            )
                        {
                            tracing::info!(
                                "Request to {} returned status {}. Retrying \
                                 attempt {}/{} after {:?}",
                                source,
                                status,
                                attempt.saturating_add(1),
                                DEFAULT_MAX_RETRIES,
                                delay
                            );
                            tokio::time::sleep(delay).await;
                            attempt = attempt.saturating_add(1);
                            continue;
                        }
                        resp.error_for_status_ref()?;
                    }
                    let text = resp.text().await?;
                    return Ok(text);
                }
                Err(err) => {
                    if attempt < DEFAULT_MAX_RETRIES
                        && err.is_connect()
                        && let Some(delay) =
                            calculate_retry_timeout(None, attempt)
                    {
                        tracing::info!(
                            "Connection error while requesting {}: {}. \
                             Retrying attempt {}/{} after {:?}",
                            source,
                            err,
                            attempt.saturating_add(1),
                            DEFAULT_MAX_RETRIES,
                            delay
                        );
                        tokio::time::sleep(delay).await;
                        attempt = attempt.saturating_add(1);
                        continue;
                    }
                    return Err(err.into());
                }
            }
        }
    }
    Ok(tokio::fs::read_to_string(
        source.strip_prefix("file://").unwrap_or(source),
    )
    .await?)
}

async fn scrape_one(
    config: Arc<Config>,
    http_client: reqwest::Client,
    proto: ProxyType,
    proxies: Arc<tokio::sync::Mutex<HashSet<Proxy>>>,
    source: &str,
    #[cfg(feature = "tui")] tx: tokio::sync::mpsc::UnboundedSender<Event>,
) -> color_eyre::Result<()> {
    let text_result = fetch_text(&config, http_client, source).await;

    #[cfg(feature = "tui")]
    drop(tx.send(Event::App(AppEvent::SourceScraped(proto.clone()))));

    let text = match text_result {
        Ok(text) => text,
        Err(e) => {
            tracing::warn!("{} | {}", source, pretty_error(&e));
            return Ok(());
        }
    };

    let matches =
        PROXY_REGEX.captures_iter(&text).collect::<Result<Vec<_>, _>>()?;

    if matches.is_empty() {
        tracing::warn!("{source} | No proxies found");
        return Ok(());
    }

    if config.scraping.max_proxies_per_source != 0
        && matches.len() > config.scraping.max_proxies_per_source
    {
        tracing::warn!(
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
    for (proto, sources) in config.scraping.sources.clone() {
        #[cfg(feature = "tui")]
        drop(tx.send(Event::App(AppEvent::SourcesTotal(
            proto.clone(),
            sources.len(),
        ))));
        for source in sources {
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
                        &source,
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
