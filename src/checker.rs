use std::sync::Arc;

use color_eyre::eyre::WrapErr;

use crate::{
    config::Config,
    event::{AppEvent, Event},
    proxy::Proxy,
    storage::ProxyStorage,
};

async fn check_one(
    config: Arc<Config>,
    mut proxy: Proxy,
    tx: tokio::sync::mpsc::UnboundedSender<Event>,
) -> color_eyre::Result<Proxy> {
    let check_result = proxy
        .check(config.clone())
        .await
        .wrap_err("proxy did not pass checking");
    tx.send(Event::App(AppEvent::ProxyChecked(proxy.protocol.clone())))?;
    match check_result {
        Ok(()) => {
            tx.send(Event::App(AppEvent::ProxyWorking(
                proxy.protocol.clone(),
            )))?;
            Ok(proxy)
        }
        Err(e) => {
            if log::log_enabled!(log::Level::Debug) {
                let mut s = proxy.as_str(true);
                s.push_str(" | ");
                s.push_str(
                    &e.chain()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(" → "),
                );
                log::debug!("{s}");
            }
            Err(e)
        }
    }
}

pub(crate) async fn check_all(
    config: Arc<Config>,
    storage: ProxyStorage,
    tx: tokio::sync::mpsc::UnboundedSender<Event>,
) -> color_eyre::Result<ProxyStorage> {
    let semaphore =
        Arc::new(tokio::sync::Semaphore::new(config.max_concurrent_checks));
    let mut join_set = tokio::task::JoinSet::new();
    for proxy in storage {
        let config = config.clone();
        let tx = tx.clone();
        let permit = semaphore
            .clone()
            .acquire_owned()
            .await
            .wrap_err("failed to acquire semaphore")?;
        join_set.spawn(async move {
            let result = check_one(config, proxy, tx).await;
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
