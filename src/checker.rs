use std::sync::Arc;

use color_eyre::eyre::OptionExt as _;

#[cfg(feature = "tui")]
use crate::event::{AppEvent, Event};
use crate::{config::Config, proxy::Proxy, utils::pretty_error};

pub async fn check_all(
    config: Arc<Config>,
    proxies: Vec<Proxy>,
    token: tokio_util::sync::CancellationToken,
    #[cfg(feature = "tui")] tx: tokio::sync::mpsc::UnboundedSender<Event>,
) -> color_eyre::Result<Vec<Proxy>> {
    if config.checking.check_url.is_none() {
        return Ok(proxies);
    }

    let workers_count =
        config.checking.max_concurrent_checks.min(proxies.len());
    if workers_count == 0 {
        return Ok(Vec::new());
    }

    #[cfg(not(feature = "tui"))]
    tracing::info!("Started checking {} proxies", proxies.len());

    let queue = Arc::new(tokio::sync::Mutex::new(proxies));
    let checked_proxies = Arc::new(tokio::sync::Mutex::new(Vec::new()));

    let mut join_set = tokio::task::JoinSet::<()>::new();
    for _ in 0..workers_count {
        let queue = Arc::clone(&queue);
        let config = Arc::clone(&config);
        let checked_proxies = Arc::clone(&checked_proxies);
        let token = token.clone();
        #[cfg(feature = "tui")]
        let tx = tx.clone();
        join_set.spawn(async move {
            tokio::select! {
                biased;
                res = async move {
                    loop {
                        let Some(mut proxy) = queue.lock().await.pop() else {
                            break;
                        };
                        let check_result = proxy.check(&config).await;
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
                                checked_proxies.lock().await.push(proxy);
                            }
                            Err(e)
                                if tracing::event_enabled!(
                                    tracing::Level::DEBUG
                                ) =>
                            {
                                tracing::debug!(
                                    "{} | {}",
                                    proxy.as_str(true),
                                    pretty_error(&e)
                                );
                            }
                            Err(_) => {}
                        }
                    }
                } => res,
                () = token.cancelled() => ()
            }
        });
    }

    while let Some(res) = join_set.join_next().await {
        match res {
            Ok(()) => {}
            Err(e) if e.is_panic() => {
                tracing::error!("proxy checking task panicked: {}", e);
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
