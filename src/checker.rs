use std::{
    net::{SocketAddr, ToSocketAddrs as _},
    sync::Arc,
};

use color_eyre::eyre::OptionExt as _;

#[cfg(feature = "tui")]
use crate::event::{AppEvent, Event};
use crate::{config::Config, http, proxy::Proxy, utils::pretty_error};

async fn resolve_check_url(config: &Config) -> Vec<SocketAddr> {
    let Some((host, port)) =
        config.checking.check_url.as_ref().and_then(|url| {
            Some((
                compact_str::CompactString::new(url.host_str()?),
                url.port_or_known_default()?,
            ))
        })
    else {
        return Vec::new();
    };

    let lookup = tokio::task::spawn_blocking(move || {
        (host.as_str(), port).to_socket_addrs().map(Iterator::collect)
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

    let workers_count =
        config.checking.max_concurrent_checks.min(proxies.len());
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

    #[cfg(not(feature = "tui"))]
    tracing::info!("Started checking {} proxies", proxies.len());

    let queue = Arc::new(parking_lot::Mutex::new(proxies));
    let checked_proxies = Arc::new(parking_lot::Mutex::new(Vec::new()));

    let mut join_set = tokio::task::JoinSet::<()>::new();
    for _ in 0..workers_count {
        let check_url_addrs = Arc::clone(&check_url_addrs);
        let checked_proxies = Arc::clone(&checked_proxies);
        let config = Arc::clone(&config);
        let queue = Arc::clone(&queue);
        let tls_backend = tls_backend.clone();
        let token = token.clone();
        #[cfg(feature = "tui")]
        let tx = tx.clone();
        join_set.spawn(async move {
            let work = async move {
                loop {
                    let Some(mut proxy) = queue.lock().pop() else {
                        break;
                    };
                    let check_result = proxy
                        .check(&config, &check_url_addrs, tls_backend.clone())
                        .await;
                    #[cfg(feature = "tui")]
                    drop(tx.send(Event::App(AppEvent::ProxyChecked(
                        proxy.protocol,
                    ))));
                    match check_result {
                        Ok(()) => {
                            #[cfg(feature = "tui")]
                            drop(tx.send(Event::App(AppEvent::ProxyWorking(
                                proxy.protocol,
                            ))));
                            checked_proxies.lock().push(proxy);
                        }
                        Err(e)
                            if tracing::event_enabled!(
                                tracing::Level::DEBUG
                            ) =>
                        {
                            tracing::debug!(
                                "{}: {}",
                                proxy.to_string(true),
                                pretty_error(&e)
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

    while let Some(res) = join_set.join_next().await {
        match res {
            Ok(()) => {}
            Err(e) if e.is_panic() => {
                tracing::error!(
                    "Proxy checking task panicked: {}",
                    pretty_error(&e.into())
                );
            }
            Err(e) => {
                return Err(e.into());
            }
        }
    }

    Ok(Arc::into_inner(checked_proxies)
        .ok_or_eyre("failed to unwrap Arc")?
        .into_inner())
}
