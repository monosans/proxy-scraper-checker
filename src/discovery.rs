use std::{collections::HashSet, sync::Arc};

use color_eyre::eyre::{OptionExt as _};
use serde::Deserialize;

#[cfg(feature = "tui")]
use crate::event::{AppEvent, Event};
use crate::{
    config::Config,
    proxy::{Proxy, ProxyType},
    utils::pretty_error,
};

#[derive(Deserialize)]
struct ShodanResult {
    ip_str: String,
    port: u16,
    #[serde(default)]
    product: Option<String>,
}

#[derive(Deserialize)]
struct ShodanResponse {
    matches: Vec<ShodanResult>,
}

async fn query_shodan(
    config: &Config,
    http_client: reqwest::Client,
) -> color_eyre::Result<Vec<Proxy>> {
    let discovery_config = config
        .discovery
        .as_ref()
        .ok_or_eyre("discovery config is None")?;
    
    let api_key = discovery_config
        .shodan_api_key
        .as_ref()
        .ok_or_eyre("Shodan API key is required")?;

    let url = format!(
        "https://api.shodan.io/shodan/host/search?key={}&query={}",
        api_key,
        urlencoding::encode(&discovery_config.search_query)
    );

    tracing::info!("Querying Shodan API with query: {}", discovery_config.search_query);

    let response = http_client
        .get(&url)
        .timeout(discovery_config.timeout)
        .send()
        .await?
        .error_for_status()?;

    let shodan_response: ShodanResponse = response.json().await?;

    let mut proxies = Vec::new();
    let max_results = discovery_config.max_results.min(shodan_response.matches.len());

    for result in shodan_response.matches.into_iter().take(max_results) {
        // Try to determine proxy type from product/banner info
        let proxy_type = determine_proxy_type(&result.product);
        
        proxies.push(Proxy {
            protocol: proxy_type,
            host: result.ip_str,
            port: result.port,
            username: None,
            password: None,
            timeout: None,
            exit_ip: None,
        });
    }

    tracing::info!("Discovered {} proxies from Shodan", proxies.len());
    Ok(proxies)
}

fn determine_proxy_type(product: &Option<String>) -> ProxyType {
    if let Some(product) = product {
        let product_lower = product.to_lowercase();
        if product_lower.contains("socks5") {
            ProxyType::Socks5
        } else if product_lower.contains("socks4") {
            ProxyType::Socks4
        } else if product_lower.contains("http") || product_lower.contains("proxy") {
            ProxyType::Http
        } else {
            // Default to HTTP if we can't determine the type
            ProxyType::Http
        }
    } else {
        // Default to HTTP if no product info
        ProxyType::Http
    }
}

pub async fn discover_all(
    config: Arc<Config>,
    http_client: reqwest::Client,
    proxies: Arc<tokio::sync::Mutex<HashSet<Proxy>>>,
    token: tokio_util::sync::CancellationToken,
    #[cfg(feature = "tui")] tx: tokio::sync::mpsc::UnboundedSender<Event>,
) -> color_eyre::Result<()> {
    if config.discovery.is_none() {
        return Ok(());
    }

    let discover_result = tokio::select! {
        biased;
        () = token.cancelled() => return Ok(()),
        result = query_shodan(&config, http_client) => result,
    };

    #[cfg(feature = "tui")]
    drop(tx.send(Event::App(AppEvent::SourceScraped(ProxyType::Http))));

    let discovered_proxies = match discover_result {
        Ok(proxies) => proxies,
        Err(e) => {
            tracing::warn!("Shodan discovery failed: {}", pretty_error(&e));
            return Ok(());
        }
    };

    #[cfg(feature = "tui")]
    let mut seen_protocols = HashSet::new();
    let mut proxies = proxies.lock().await;
    for proxy in discovered_proxies {
        if config.protocol_is_enabled(&proxy.protocol) {
            #[cfg(feature = "tui")]
            seen_protocols.insert(proxy.protocol.clone());
            proxies.insert(proxy);
        }
    }
    #[cfg(feature = "tui")]
    for proto in seen_protocols {
        let count = proxies.iter().filter(|p| p.protocol == proto).count();
        drop(tx.send(Event::App(AppEvent::TotalProxies(proto, count))));
    }
    drop(proxies);
    Ok(())
}