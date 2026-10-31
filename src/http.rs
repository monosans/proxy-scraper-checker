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
#[serde(deny_unknown_fields)]
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

    let jitter = 1.0_f64.algebraic_sub(0.25_f64.algebraic_mul(fastrand::f64()));
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

#[cfg(test)]
pub mod tests {
    use std::{
        future,
        net::SocketAddr,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::{Duration, SystemTime},
    };

    use reqwest::header::{HeaderMap, HeaderValue};
    use tokio::{
        io::{AsyncReadExt as _, AsyncWriteExt as _},
        net::{TcpListener, TcpStream},
    };

    use super::{backoff, calculate_retry_timeout, parse_retry_after};
    use crate::config::tests::test_config;

    const OK: &str =
        "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
    const UNAVAILABLE: &str =
        "HTTP/1.1 503 Service Unavailable\r\nRetry-After-Ms: \
         1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    const NOT_FOUND: &str = "HTTP/1.1 404 Not Found\r\nContent-Length: \
                             0\r\nConnection: close\r\n\r\n";

    async fn accept(listener: &TcpListener) -> TcpStream {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 1024];
        assert_ne!(socket.read(&mut request).await.unwrap(), 0);
        socket
    }

    // Answers one connection per reply, in order.
    pub async fn serve(
        replies: Vec<&'static str>,
    ) -> (SocketAddr, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&requests);
        tokio::spawn(async move {
            for reply in replies {
                let mut socket = accept(&listener).await;
                counter.fetch_add(1, Ordering::Relaxed);
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        (address, requests)
    }

    // Sends `reply`, then holds the connection open and silent.
    pub async fn serve_then_stall(reply: &'static str) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut socket = accept(&listener).await;
            socket.write_all(reply.as_bytes()).await.unwrap();
            future::pending::<()>().await;
            drop(socket);
        });
        address
    }

    async fn get(
        address: SocketAddr,
    ) -> reqwest_middleware::Result<reqwest::Response> {
        let config = test_config(|_| {}).await;
        super::create_reqwest_client(&config)
            .await
            .unwrap()
            .get(format!("http://{address}/"))
            .send()
            .await
    }

    fn header(name: &'static str, value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(name, HeaderValue::from_str(value).unwrap());
        headers
    }

    #[tokio::test]
    async fn a_temporary_failure_is_retried() {
        let (address, requests) = serve(vec![UNAVAILABLE, OK]).await;
        let response = get(address).await.unwrap();
        assert_eq!(response.text().await.unwrap(), "ok");
        assert_eq!(requests.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn retries_run_out() {
        let (address, requests) =
            serve(vec![UNAVAILABLE, UNAVAILABLE, UNAVAILABLE, OK]).await;
        drop(get(address).await.unwrap_err());
        assert_eq!(requests.load(Ordering::Relaxed), 3);
    }

    #[tokio::test]
    async fn a_permanent_failure_is_not_retried() {
        let (address, requests) = serve(vec![NOT_FOUND, OK]).await;
        drop(get(address).await.unwrap_err());
        assert_eq!(requests.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn retry_after_comes_in_three_forms() {
        assert_eq!(
            parse_retry_after(&header("retry-after-ms", "1500")),
            Some(Duration::from_millis(1500))
        );
        assert_eq!(
            parse_retry_after(&header("retry-after", "3")),
            Some(Duration::from_secs(3))
        );

        let date = httpdate::fmt_http_date(
            SystemTime::now() + Duration::from_secs(60),
        );
        let delay = parse_retry_after(&header("retry-after", &date)).unwrap();
        assert!(delay > Duration::from_secs(50));

        assert_eq!(parse_retry_after(&header("retry-after", "?")), None);
        assert_eq!(parse_retry_after(&HeaderMap::new()), None);
    }

    #[test]
    fn a_server_asking_for_too_long_a_wait_is_not_waited_for() {
        assert_eq!(
            calculate_retry_timeout(Some(&header("retry-after", "1")), 0),
            Some(Duration::from_secs(1))
        );
        assert_eq!(
            calculate_retry_timeout(Some(&header("retry-after", "3600")), 0),
            None
        );
    }

    #[test]
    fn backoff_doubles_up_to_a_limit() {
        assert_eq!(backoff(0), Duration::from_millis(500));
        assert_eq!(backoff(1), Duration::from_secs(1));
        assert_eq!(backoff(10), Duration::from_secs(8));
    }

    #[test]
    fn jitter_only_shortens_the_wait_slightly() {
        for _ in 0_u8..100 {
            let delay = calculate_retry_timeout(None, 1).unwrap();
            assert!(delay <= Duration::from_secs(1));
            assert!(delay >= Duration::from_millis(750));
        }
    }
}
