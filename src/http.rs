use std::{
    io,
    sync::Arc,
    time::{Duration, SystemTime},
};

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
    pub username: compact_str::CompactString,
    pub password: Option<compact_str::CompactString>,
}

pub enum TlsProfile {
    Scraping,
    Checking,
}

#[cfg_attr(target_os = "android", expect(clippy::unused_async))]
pub async fn build_rustls_config(
    profile: TlsProfile,
) -> crate::Result<rustls::ClientConfig> {
    let mut provider = rustls::crypto::aws_lc_rs::default_provider();
    if matches!(profile, TlsProfile::Checking) {
        provider.kx_groups = vec![
            rustls::crypto::aws_lc_rs::kx_group::X25519,
            rustls::crypto::aws_lc_rs::kx_group::SECP256R1,
            rustls::crypto::aws_lc_rs::kx_group::SECP384R1,
        ];
    }

    let builder =
        rustls::ClientConfig::builder_with_provider(Arc::new(provider))
            .with_safe_default_protocol_versions()?;

    #[cfg(not(target_os = "android"))]
    let builder = tokio::task::spawn_blocking(move || {
        rustls_platform_verifier::BuilderVerifierExt::with_platform_verifier(
            builder,
        )
    })
    .await??;

    #[cfg(target_os = "android")]
    let builder = builder.with_root_certificates(rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    });

    let mut config = builder.with_no_client_auth();

    match profile {
        TlsProfile::Scraping => {
            config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        }
        TlsProfile::Checking => {
            config.alpn_protocols = vec![b"http/1.1".to_vec()];
            config.resumption = rustls::client::Resumption::disabled();
        }
    }

    Ok(config)
}

pub struct RetryMiddleware;

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

fn backoff(attempt: u32) -> Duration {
    INITIAL_RETRY_DELAY
        .saturating_mul(2_u32.saturating_pow(attempt))
        .min(MAX_RETRY_DELAY)
}

fn total_backoff() -> Duration {
    (0..DEFAULT_MAX_RETRIES)
        .map(backoff)
        .fold(Duration::ZERO, Duration::saturating_add)
}

fn calculate_retry_timeout(
    headers: Option<&reqwest::header::HeaderMap>,
    attempt: u32,
) -> Option<Duration> {
    if let Some(h) = headers
        && let Some(after) = parse_retry_after(h)
    {
        if after > total_backoff() {
            return None;
        }
        return Some(after);
    }

    let jitter = 0.25_f64.mul_add(-fastrand::f64(), 1.0);
    Some(backoff(attempt).mul_f64(jitter))
}

#[async_trait::async_trait]
impl reqwest_middleware::Middleware for RetryMiddleware {
    async fn handle(
        &self,
        req: reqwest::Request,
        extensions: &mut http::Extensions,
        next: reqwest_middleware::Next<'_>,
    ) -> reqwest_middleware::Result<reqwest::Response> {
        let mut attempt: u32 = 0;
        loop {
            let req = req.try_clone().ok_or_else(|| {
                reqwest_middleware::Error::middleware(io::Error::other(
                    "Request object is not cloneable",
                ))
            })?;

            match next.clone().run(req, extensions).await {
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
                            drop(resp);
                            tokio::time::sleep(delay).await;
                            attempt = attempt.saturating_add(1);
                            continue;
                        }
                        resp.error_for_status_ref()?;
                    }
                    return Ok(resp);
                }
                Err(err) => {
                    if attempt < DEFAULT_MAX_RETRIES
                        && err.is_connect()
                        && let Some(delay) =
                            calculate_retry_timeout(None, attempt)
                    {
                        drop(err);
                        tokio::time::sleep(delay).await;
                        attempt = attempt.saturating_add(1);
                        continue;
                    }
                    return Err(err);
                }
            }
        }
    }
}

pub async fn create_reqwest_client(
    config: &Config,
) -> crate::Result<reqwest_middleware::ClientWithMiddleware> {
    let tls_backend = build_rustls_config(TlsProfile::Scraping).await?;
    let mut builder = reqwest::ClientBuilder::new()
        .user_agent(config.scraping.user_agent.as_bytes())
        .timeout(config.scraping.timeout)
        .connect_timeout(config.scraping.connect_timeout)
        .tls_backend_preconfigured(tls_backend);

    if let Some(proxy) = config.scraping.proxy.clone() {
        builder = builder.proxy(reqwest::Proxy::all(proxy)?);
    }

    let client = builder.build()?;
    let client_with_middleware = reqwest_middleware::ClientBuilder::new(client)
        .with(RetryMiddleware)
        .build();

    Ok(client_with_middleware)
}
