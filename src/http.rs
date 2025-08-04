use std::time::{Duration, SystemTime};

use color_eyre::Result;

use crate::config::Config;

pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
                              AppleWebKit/537.36 (KHTML, like Gecko) \
                              Chrome/138.0.0.0 Safari/537.36";

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

pub async fn fetch_text(
    http_client: reqwest::Client,
    url: &str,
) -> Result<String> {
    let mut attempt: u32 = 0;
    loop {
        let resp = http_client.get(url).send().await;
        match resp {
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
                let text = resp.text().await?;
                return Ok(text);
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
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(config.scraping.timeout)
        .connect_timeout(config.scraping.connect_timeout)
        .use_rustls_tls()
        .build()
}
