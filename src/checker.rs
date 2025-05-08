use std::{collections::VecDeque, sync::Arc};

use color_eyre::eyre::WrapErr as _;

#[cfg(feature = "tui")]
use crate::event::{AppEvent, Event};
use crate::{config::Config, storage::ProxyStorage, utils::pretty_error};

pub async fn check_all(
    config: Arc<Config>,
    storage: ProxyStorage,
    #[cfg(feature = "tui")] tx: tokio::sync::mpsc::UnboundedSender<Event>,
) -> color_eyre::Result<ProxyStorage> {
    let workers_count = config.max_concurrent_checks.min(storage.len());
    if workers_count == 0 {
        return Ok(ProxyStorage::new(config.sources.keys().cloned().collect()));
    }

    let queue = Arc::new(tokio::sync::Mutex::new(
        storage.into_iter().collect::<VecDeque<_>>(),
    ));

    let (result_tx, mut result_rx) = tokio::sync::mpsc::unbounded_channel();

    let mut join_set = tokio::task::JoinSet::<color_eyre::Result<()>>::new();

    for _ in 0..workers_count {
        let queue = Arc::clone(&queue);
        let config = Arc::clone(&config);
        #[cfg(feature = "tui")]
        let tx = tx.clone();
        let result_tx = result_tx.clone();
        join_set.spawn(async move {
            loop {
                let Some(mut proxy) = queue.lock().await.pop_front() else {
                    break Ok(());
                };
                let check_result = proxy
                    .check(Arc::clone(&config))
                    .await
                    .wrap_err("proxy did not pass checking");
                #[cfg(feature = "tui")]
                tx.send(Event::App(AppEvent::ProxyChecked(
                    proxy.protocol.clone(),
                )))?;
                match check_result {
                    Ok(()) => {
                        #[cfg(feature = "tui")]
                        tx.send(Event::App(AppEvent::ProxyWorking(
                            proxy.protocol.clone(),
                        )))?;
                        result_tx.send(proxy)?;
                    }
                    Err(e) => {
                        if log::log_enabled!(log::Level::Debug) {
                            log::debug!(
                                "{} | {}",
                                proxy.as_str(true),
                                pretty_error(&e)
                            );
                        }
                    }
                }
            }
        });
    }

    drop(result_tx);

    while let Some(res) = join_set.join_next().await {
        res.wrap_err("failed to join proxy checking task")?
            .wrap_err("proxy checking worker failed")?;
    }

    let mut new_storage =
        ProxyStorage::new(config.sources.keys().cloned().collect());
    while let Some(proxy) = result_rx.recv().await {
        new_storage.insert(proxy);
    }
    Ok(new_storage)
}
