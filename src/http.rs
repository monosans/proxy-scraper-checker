use std::{
    collections::HashMap,
    fmt::Display,
    time::{Duration, SystemTime},
};

use color_eyre::Result;

use crate::config::Config;

const DEFAULT_MAX_RETRIES: u32 = 2;
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

#[derive(Clone, serde::Deserialize)]
pub struct BasicAuth {
    pub username: String,
    pub password: Option<String>,
}

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
    attempt: u32,
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
        .saturating_mul(2_u32.pow(attempt))
        .min(MAX_RETRY_DELAY);
    let jitter = 0.25_f64.mul_add(-rand::random::<f64>(), 1.0);
    Some(base.mul_f64(jitter))
}

pub async fn fetch_text<U: reqwest::IntoUrl + Clone + Display>(
    http_client: reqwest::Client,
    url: U,
    basic_auth: Option<&BasicAuth>,
    headers: Option<&HashMap<String, String>>,
) -> Result<String> {
    let mut attempt: u32 = 0;
    loop {
        let mut request = http_client.get(url.clone());
        if let Some(auth) = basic_auth {
            request =
                request.basic_auth(&auth.username, auth.password.as_ref());
        }
        if let Some(headers) = headers {
            for (k, v) in headers {
                request = request.header(k, v);
            }
        }
        match request.send().await {
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
                            url,
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
                return Ok(resp.text().await?);
            }
            Err(err) => {
                if attempt < DEFAULT_MAX_RETRIES
                    && err.is_connect()
                    && let Some(delay) = calculate_retry_timeout(None, attempt)
                {
                    tracing::info!(
                        "Connection error while requesting {}: {}. Retrying \
                         attempt {}/{} after {:?}",
                        url,
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

pub fn create_reqwest_client(
    config: &Config,
) -> reqwest::Result<reqwest::Client> {
    let mut builder = reqwest::ClientBuilder::new()
        .user_agent(&config.scraping.user_agent)
        .timeout(config.scraping.timeout)
        .connect_timeout(config.scraping.connect_timeout)
        .use_rustls_tls();
    if let Some(proxy) = config.scraping.proxy.clone() {
        builder = builder.proxy(reqwest::Proxy::all(proxy)?);
    }
    builder.build()
}
