use std::{
    net::{SocketAddr, ToSocketAddrs as _},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use color_eyre::eyre::OptionExt as _;
use compact_str::{CompactString, format_compact};

#[cfg(feature = "tui")]
use crate::event::{AppEvent, Event, Stage};
use crate::{
    config::Config,
    http,
    proxy::Proxy,
    utils::{clock, grouped, log_panic, plural, pretty_error},
};

// Checking can take many minutes, and a log that says nothing for that long
// looks exactly like a hang.
const PROGRESS_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Default)]
struct Progress {
    checked: AtomicUsize,
    working: AtomicUsize,
}

fn progress_line(checked: usize, total: usize, working: usize, elapsed: Duration) -> CompactString {
    let mut line = format_compact!(
        "Checked {} of {} {}, {} working",
        grouped(checked),
        grouped(total),
        plural(total, "proxy", "proxies"),
        grouped(working)
    );
    let left = (total.saturating_sub(checked) as u64)
        .saturating_mul(elapsed.as_secs())
        .checked_div(checked as u64);
    if let Some(left) = left {
        line.push_str(", about ");
        line.push_str(&clock(left));
        line.push_str(" left");
    }
    line
}

// The TUI shows all of this already, and without it nobody reads the events,
// so that is what tells the two apart.
#[cfg(feature = "tui")]
fn nobody_watches(tx: &tokio::sync::mpsc::UnboundedSender<Event>) -> bool {
    tx.is_closed()
}

#[cfg(not(feature = "tui"))]
const fn nobody_watches() -> bool {
    true
}

// Popping alone keeps the memory of every proxy already taken until the last
// check. Releasing it only when half is unused copies about one proxy per pop.
fn next_proxy(queue: &parking_lot::Mutex<Vec<Proxy>>) -> Option<Proxy> {
    let mut queue = queue.lock();
    let proxy = queue.pop();
    if queue.len() < queue.capacity().div_euclid(2) {
        queue.shrink_to_fit();
    }
    proxy
}

async fn resolve_check_url(config: &Config) -> Vec<SocketAddr> {
    let Some((host, port)) =
        config.checking.check_url.as_ref().and_then(|url| {
            Some((CompactString::new(url.host_str()?), url.port_or_known_default()?))
        })
    else {
        return Vec::new();
    };

    // Only SOCKS4 uses these, as it can't name a host, and it can't carry IPv6
    // either. Every other protocol leaves resolving to the proxy.
    let lookup = tokio::task::spawn_blocking(move || {
        (host.as_str(), port)
            .to_socket_addrs()
            .map(|addresses| addresses.filter(SocketAddr::is_ipv4).collect())
    });

    if let Ok(Ok(addrs)) = lookup.await { addrs } else { Vec::new() }
}

pub async fn check_all(
    config: Arc<Config>,
    proxies: Vec<Proxy>,
    token: tokio_util::sync::CancellationToken,
    #[cfg(feature = "tui")] tx: tokio::sync::mpsc::UnboundedSender<Event>,
) -> crate::Result<Vec<Proxy>> {
    if config.checking.check_url.is_none() {
        return Ok(proxies);
    }

    let workers_count = config.checking.max_concurrent_checks.min(proxies.len());
    if workers_count == 0 {
        return Ok(Vec::new());
    }

    let prepare = async {
        tokio::try_join!(
            async { Ok(Arc::new(resolve_check_url(&config).await)) },
            http::build_rustls_config(http::TlsProfile::Checking),
        )
    };

    let (check_url_addrs, tls_backend) = tokio::select! {
        biased;
        () = token.cancelled() => return Ok(Vec::new()),
        res = prepare => res?,
    };

    #[cfg(feature = "tui")]
    drop(tx.send(Event::App(AppEvent::Stage(Stage::Checking))));
    tracing::info!(
        "Started checking {} {}",
        grouped(proxies.len()),
        plural(proxies.len(), "proxy", "proxies")
    );

    let log_the_progress = nobody_watches(
        #[cfg(feature = "tui")]
        &tx,
    );

    let total = proxies.len();
    let progress = Arc::new(Progress::default());

    let queue = Arc::new(parking_lot::Mutex::new(proxies));
    let checked_proxies = Arc::new(parking_lot::Mutex::new(Vec::new()));

    let mut join_set = tokio::task::JoinSet::<()>::new();
    for _ in 0..workers_count {
        let progress = Arc::clone(&progress);
        let check_url_addrs = Arc::clone(&check_url_addrs);
        let checked_proxies = Arc::clone(&checked_proxies);
        let config = Arc::clone(&config);
        let queue = Arc::clone(&queue);
        let tls_backend = tls_backend.clone();
        let token = token.clone();
        #[cfg(feature = "tui")]
        let tx = tx.clone();
        join_set.spawn(async move {
            let work = async {
                loop {
                    let Some(mut proxy) = next_proxy(&queue) else {
                        break;
                    };
                    let check_result =
                        proxy.check(&config, &check_url_addrs, tls_backend.clone()).await;
                    progress.checked.fetch_add(1, Ordering::Relaxed);
                    #[cfg(feature = "tui")]
                    drop(tx.send(Event::App(AppEvent::ProxyChecked(proxy.protocol))));
                    match check_result {
                        Ok(()) => {
                            progress.working.fetch_add(1, Ordering::Relaxed);
                            #[cfg(feature = "tui")]
                            drop(tx.send(Event::App(AppEvent::ProxyWorking {
                                protocol: proxy.protocol,
                                address: format_compact!("{}:{}", proxy.host, proxy.port),
                                latency: proxy.timeout.unwrap_or_default(),
                            })));
                            checked_proxies.lock().push(proxy);
                        }
                        Err(e) if tracing::event_enabled!(tracing::Level::DEBUG) => {
                            tracing::debug!(
                                "{}: {}",
                                proxy.to_compact_string(true),
                                pretty_error(&e.into())
                            );
                        }
                        Err(_) => {}
                    }
                }
            };

            tokio::select! {
                biased;
                res = work => res,
                () = token.cancelled() => (),
            }
        });
    }

    drop(check_url_addrs);
    drop(config);
    drop(queue);
    drop(tls_backend);
    drop(token);
    #[cfg(feature = "tui")]
    drop(tx);

    let started = tokio::time::Instant::now();
    let mut report = tokio::time::interval_at(
        started.checked_add(PROGRESS_INTERVAL).unwrap_or(started),
        PROGRESS_INTERVAL,
    );

    loop {
        tokio::select! {
            biased;
            res = join_set.join_next() => {
                let Some(res) = res else {
                    break;
                };
                log_panic("Proxy checking task", res);
            }
            _ = report.tick(), if log_the_progress => {
                tracing::info!(
                    "{}",
                    progress_line(
                        progress.checked.load(Ordering::Relaxed),
                        total,
                        progress.working.load(Ordering::Relaxed),
                        started.elapsed(),
                    )
                );
            }
        }
    }

    Ok(Arc::into_inner(checked_proxies).ok_or_eyre("failed to unwrap Arc")?.into_inner())
}

#[cfg(test)]
mod tests {
    use std::{
        net::{Ipv4Addr, SocketAddr},
        sync::Arc,
        time::Duration,
    };

    use compact_str::{CompactString, ToCompactString as _};
    use tokio_util::sync::CancellationToken;

    use super::{check_all, next_proxy, progress_line, resolve_check_url};
    use crate::{
        config::{Config, tests::test_config},
        http::tests::{serve, serve_then_stall},
        proxy::{Proxy, ProxyType},
        raw_config::RawConfig,
        utils::tests::capture_logs,
    };

    const EXIT_IP_REPLY: &str =
        "HTTP/1.1 200 OK\r\nContent-Length: 9\r\nConnection: close\r\n\r\n192.0.2.1";

    fn http_proxy(address: SocketAddr) -> Proxy {
        Proxy {
            protocol: ProxyType::Http,
            host: address.ip().to_compact_string(),
            port: address.port(),
            auth: None,
            timeout: None,
            exit_ip: None,
        }
    }

    // Nothing listens there any more, so connecting is refused at once.
    async fn refusing_proxy() -> Proxy {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        http_proxy(address)
    }

    async fn checking_config(edit: impl FnOnce(&mut RawConfig)) -> Config {
        test_config(move |raw| {
            // An address, so that nothing waits for DNS.
            raw.checking.check_url = Some("http://127.0.0.1:9/".parse().unwrap());
            edit(raw);
        })
        .await
    }

    // As when nobody reads the events.
    async fn check(config: Config, proxies: Vec<Proxy>, token: CancellationToken) -> Vec<Proxy> {
        #[cfg(feature = "tui")]
        let tx = tokio::sync::mpsc::unbounded_channel().0;
        check_all(
            Arc::new(config),
            proxies,
            token,
            #[cfg(feature = "tui")]
            tx,
        )
        .await
        .unwrap()
    }

    fn endpoints(proxies: &[Proxy]) -> Vec<CompactString> {
        proxies.iter().map(|proxy| proxy.to_compact_string(false)).collect()
    }

    #[tokio::test]
    async fn without_a_check_url_every_proxy_is_kept_unchecked() {
        let config = test_config(|raw| raw.checking.check_url = None).await;
        let proxy = refusing_proxy().await;
        let expected = endpoints(std::slice::from_ref(&proxy));

        let proxies = check(config, vec![proxy], CancellationToken::new()).await;

        assert_eq!(endpoints(&proxies), expected);
        assert!(proxies.iter().all(|proxy| proxy.timeout.is_none()));
    }

    #[tokio::test]
    async fn no_proxies_leave_nothing_to_check() {
        let config = checking_config(|_| {}).await;
        let proxies = check(config, Vec::new(), CancellationToken::new()).await;
        assert!(proxies.is_empty());
    }

    #[tokio::test]
    async fn a_stop_before_checking_keeps_nothing() {
        let (address, requests) = serve(vec![EXIT_IP_REPLY]).await;
        let config = checking_config(|_| {}).await;
        let token = CancellationToken::new();
        token.cancel();

        let proxies = check(config, vec![http_proxy(address)], token).await;

        assert!(proxies.is_empty());
        assert_eq!(requests.lock().len(), 0);
    }

    #[tokio::test]
    async fn only_the_working_proxies_are_kept_and_why_not_is_logged() {
        let logs = capture_logs();
        let (address, _) = serve(vec![EXIT_IP_REPLY]).await;
        let working = http_proxy(address);
        let refusing = refusing_proxy().await;
        let expected = endpoints(std::slice::from_ref(&working));
        let failed = refusing.to_compact_string(true);
        let config = checking_config(|_| {}).await;
        #[cfg(feature = "tui")]
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        let proxies = check_all(
            Arc::new(config),
            vec![working, refusing],
            CancellationToken::new(),
            #[cfg(feature = "tui")]
            tx,
        )
        .await
        .unwrap();

        assert_eq!(endpoints(&proxies), expected);
        let proxy = proxies.first().unwrap();
        assert!(proxy.timeout.is_some());
        assert_eq!(proxy.exit_ip, Some(Ipv4Addr::new(192, 0, 2, 1)));
        let text = logs.text();
        assert!(text.contains("Started checking 2 proxies"), "{text}");
        assert!(text.contains(&format!(": {failed}: ")), "{text}");

        #[cfg(feature = "tui")]
        {
            use crate::event::{AppEvent, Event, Stage};

            let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
            assert!(matches!(events.first(), Some(Event::App(AppEvent::Stage(Stage::Checking)))));
            let checked = events
                .iter()
                .filter(|event| {
                    matches!(event, Event::App(AppEvent::ProxyChecked(ProxyType::Http)))
                })
                .count();
            assert_eq!(checked, 2);
            let working: Vec<_> = events
                .iter()
                .filter_map(|event| {
                    if let Event::App(AppEvent::ProxyWorking { protocol, address, latency }) = event
                    {
                        Some((*protocol, address.as_str(), Some(*latency)))
                    } else {
                        None
                    }
                })
                .collect();
            let first = expected.first().map(CompactString::as_str).unwrap();
            assert!(working == [(ProxyType::Http, first, proxy.timeout)]);
        }
    }

    #[tokio::test]
    async fn a_failed_check_outside_debug_mode_is_dropped() {
        let config = checking_config(|_| {}).await;
        let proxies = check(config, vec![refusing_proxy().await], CancellationToken::new()).await;
        assert!(proxies.is_empty());
    }

    // The clock only moves when nothing else can, so the slow check takes no
    // real time.
    #[tokio::test(start_paused = true)]
    async fn a_long_check_reports_its_progress() {
        let logs = capture_logs();
        let (address, _held) = serve_then_stall("").await;
        let config = checking_config(|raw| {
            raw.checking.timeout = Duration::from_secs(100);
            raw.checking.connect_timeout = Duration::from_secs(100);
        })
        .await;

        let proxies = check(config, vec![http_proxy(address)], CancellationToken::new()).await;

        assert!(proxies.is_empty());
        let text = logs.text();
        let reports = text.matches("Checked 0 of 1 proxy, 0 working").count();
        assert_eq!(reports, 3, "{text}");
    }

    async fn resolved(check_url: &str) -> Vec<SocketAddr> {
        let config = test_config(move |raw| {
            raw.checking.check_url = Some(check_url.parse().unwrap());
        })
        .await;
        resolve_check_url(&config).await
    }

    #[tokio::test]
    async fn the_check_url_resolves_to_ipv4_only() {
        let addresses = resolved("http://localhost/").await;
        assert_ne!(addresses.len(), 0);
        assert!(addresses.iter().all(SocketAddr::is_ipv4));
        drop(addresses);
        assert_eq!(resolved("http://[::1]/").await.len(), 0);
    }

    #[tokio::test]
    async fn without_a_check_url_there_is_nothing_to_resolve() {
        let config = test_config(|raw| raw.checking.check_url = None).await;
        assert_eq!(resolve_check_url(&config).await, []);
    }

    #[test]
    fn the_queue_releases_what_was_taken_from_it() {
        let proxies = (0_u16..1024)
            .map(|port| Proxy {
                protocol: ProxyType::Http,
                host: CompactString::const_new("1.1.1.1"),
                port,
                auth: None,
                timeout: None,
                exit_ip: None,
            })
            .collect();
        let queue = parking_lot::Mutex::new(proxies);

        for _ in 0_u16..1000 {
            assert!(next_proxy(&queue).is_some());
        }
        assert_eq!(queue.lock().len(), 24);
        assert!(queue.lock().capacity() < 64);

        while next_proxy(&queue).is_some() {}
        assert_eq!(queue.lock().capacity(), 0);
    }

    #[test]
    fn progress_says_how_far_along_and_how_long_is_left() {
        assert_eq!(
            progress_line(25_000, 100_000, 312, Duration::from_secs(60)),
            "Checked 25,000 of 100,000 proxies, 312 working, about 3:00 left"
        );
    }

    #[test]
    fn progress_makes_no_guess_before_anything_is_checked() {
        assert_eq!(
            progress_line(0, 100, 0, Duration::from_secs(30)),
            "Checked 0 of 100 proxies, 0 working"
        );
    }
}
