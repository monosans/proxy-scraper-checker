use std::{
    io,
    net::SocketAddr,
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
    pub username: String,
    pub password: Option<String>,
}

pub struct HickoryDnsResolver(Arc<hickory_resolver::TokioResolver>);

impl HickoryDnsResolver {
    pub async fn new() -> Result<Self, tokio::task::JoinError> {
        let mut builder = tokio::task::spawn_blocking(
            hickory_resolver::TokioResolver::builder_tokio,
        )
        .await?
        .unwrap_or_else(|_| {
            hickory_resolver::TokioResolver::builder_with_config(
                hickory_resolver::config::ResolverConfig::cloudflare(),
                hickory_resolver::name_server::TokioConnectionProvider::default(
                ),
            )
        });

        builder.options_mut().ip_strategy =
            hickory_resolver::config::LookupIpStrategy::Ipv4AndIpv6;
        Ok(Self(Arc::new(builder.build())))
    }
}

impl reqwest::dns::Resolve for HickoryDnsResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let resolver = Arc::clone(&self.0);
        Box::pin(async move {
            let lookup = resolver.lookup_ip(name.as_str()).await?;
            drop(resolver);
            let addrs: reqwest::dns::Addrs = Box::new(
                lookup.into_iter().map(|ip_addr| SocketAddr::new(ip_addr, 0)),
            );
            Ok(addrs)
        })
    }
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

pub struct RetryMiddleware;

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

pub fn create_reqwest_client<R: reqwest::dns::Resolve + 'static>(
    config: &Config,
    dns_resolver: Arc<R>,
) -> reqwest::Result<reqwest_middleware::ClientWithMiddleware> {
    let mut builder = reqwest::ClientBuilder::new()
        .user_agent(&config.scraping.user_agent)
        .timeout(config.scraping.timeout)
        .connect_timeout(config.scraping.connect_timeout)
        .dns_resolver(dns_resolver);

    if let Some(proxy) = &config.scraping.proxy {
        builder = builder.proxy(reqwest::Proxy::all(proxy.clone())?);
    }

    let client = builder.build()?;
    let client_with_middleware = reqwest_middleware::ClientBuilder::new(client)
        .with(RetryMiddleware)
        .build();

    Ok(client_with_middleware)
}
