use std::{collections::HashSet, sync::Arc};

use color_eyre::eyre::WrapErr as _;

#[cfg(feature = "tui")]
use crate::event::{AppEvent, Event};
use crate::{config::Config, proxy::Proxy, utils::pretty_error};

pub async fn check_all(
    config: Arc<Config>,
    proxies: Arc<tokio::sync::Mutex<HashSet<Proxy>>>,
    token: tokio_util::sync::CancellationToken,
    #[cfg(feature = "tui")] tx: tokio::sync::mpsc::UnboundedSender<Event>,
) -> color_eyre::Result<()> {
    let workers_count =
        config.checking.max_concurrent_checks.min(proxies.lock().await.len());
    if workers_count == 0 {
        return Ok(());
    }

    let queue = Arc::new(tokio::sync::Mutex::new(
        proxies.lock().await.drain().collect::<Vec<_>>(),
    ));

    let mut join_set = tokio::task::JoinSet::<color_eyre::Result<()>>::new();
    for _ in 0..workers_count {
        let queue = Arc::clone(&queue);
        let config = Arc::clone(&config);
        let proxies = Arc::clone(&proxies);
        let token = token.clone();
        #[cfg(feature = "tui")]
        let tx = tx.clone();
        join_set.spawn(async move {
            tokio::select! {
                biased;
                res = async move {
                    loop {
                        let Some(mut proxy) = queue.lock().await.pop() else {
                            break Ok(());
                        };
                        let check_result = proxy.check(&config).await;
                        #[cfg(feature = "tui")]
                        drop(tx.send(Event::App(AppEvent::ProxyChecked(
                            proxy.protocol.clone(),
                        ))));
                        match check_result {
                            Ok(()) => {
                                #[cfg(feature = "tui")]
                                drop(tx.send(Event::App(AppEvent::ProxyWorking(
                                    proxy.protocol.clone(),
                                ))));
                                proxies.lock().await.insert(proxy);
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
                () = token.cancelled() => Ok(())
            }
        });
    }

    while let Some(res) = join_set.join_next().await {
        match res {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                return Err(e).wrap_err("proxy checking task failed");
            }
            Err(e) => {
                tracing::error!(
                    "proxy checking task panicked or was cancelled: {}",
                    e
                );
            }
        }
    }

    Ok(())
}
