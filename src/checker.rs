use std::sync::Arc;

use color_eyre::eyre::WrapErr as _;

use crate::{config::Config, proxy::Proxy, storage::ProxyStorage};

#[cfg(feature = "tui")]
use crate::event::{AppEvent, Event};

async fn check_one(
    config: Arc<Config>,
    mut proxy: Proxy,
    #[cfg(feature = "tui")] tx: tokio::sync::mpsc::UnboundedSender<Event>,
) -> color_eyre::Result<Proxy> {
    let check_result = proxy
        .check(Arc::clone(&config))
        .await
        .wrap_err("proxy did not pass checking");
    #[cfg(feature = "tui")]
    tx.send(Event::App(AppEvent::ProxyChecked(proxy.protocol.clone())))?;
    match check_result {
        Ok(()) => {
            #[cfg(feature = "tui")]
            tx.send(Event::App(AppEvent::ProxyWorking(
                proxy.protocol.clone(),
            )))?;
            Ok(proxy)
        }
        Err(e) => {
            if log::log_enabled!(log::Level::Debug) {
                log::debug!(
                    "{} | {}",
                    proxy.as_str(true),
                    e.chain()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(" \u{2192} ")
                );
            }
            Err(e)
        }
    }
}

pub async fn check_all(
    config: Arc<Config>,
    storage: ProxyStorage,
    #[cfg(feature = "tui")] tx: tokio::sync::mpsc::UnboundedSender<Event>,
) -> color_eyre::Result<ProxyStorage> {
    let semaphore =
        Arc::new(tokio::sync::Semaphore::new(config.max_concurrent_checks));
    let mut join_set = tokio::task::JoinSet::new();
    for proxy in storage {
        let config = Arc::clone(&config);
        #[cfg(feature = "tui")]
        let tx = tx.clone();
        let permit = Arc::clone(&semaphore)
            .acquire_owned()
            .await
            .wrap_err("failed to acquire semaphore")?;
        join_set.spawn(async move {
            let result = check_one(
                config,
                proxy,
                #[cfg(feature = "tui")]
                tx,
            )
            .await;
            drop(permit);
            result
        });
    }
    let mut new_storage =
        ProxyStorage::new(config.sources.keys().cloned().collect());
    while let Some(res) = join_set.join_next().await {
        if let Ok(proxy) = res.wrap_err("failed to join proxy checking task")? {
            new_storage.insert(proxy);
        }
    }
    Ok(new_storage)
}
