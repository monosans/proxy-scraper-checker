use std::{collections::HashSet, sync::Arc};

use color_eyre::eyre::{OptionExt as _, WrapErr as _};
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

    // Add rate limiting - Shodan free accounts are limited to 1 request per second
    tokio::time::sleep(tokio::time::Duration::from_millis(1100)).await;

    let response = http_client
        .get(&url)
        .timeout(discovery_config.timeout)
        .send()
        .await
        .wrap_err("Failed to send request to Shodan API")?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        return Err(color_eyre::eyre::eyre!(
            "Shodan API request failed with status {}: {}",
            status,
            error_text
        ));
    }

    let shodan_response: ShodanResponse = response
        .json()
        .await
        .wrap_err("Failed to parse Shodan API response as JSON")?;

    let mut proxies = Vec::new();
    let max_results = discovery_config.max_results.min(shodan_response.matches.len());

    for result in shodan_response.matches.into_iter().take(max_results) {
        // Validate that we have a valid IP and port
        if result.ip_str.is_empty() || result.port == 0 {
            continue;
        }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::ProxyType;

    #[test]
    fn test_determine_proxy_type() {
        // Test HTTP detection
        assert_eq!(determine_proxy_type(&Some("squid".to_string())), ProxyType::Http);
        assert_eq!(determine_proxy_type(&Some("HTTP proxy".to_string())), ProxyType::Http);
        assert_eq!(determine_proxy_type(&Some("Apache HTTP Server".to_string())), ProxyType::Http);

        // Test SOCKS5 detection
        assert_eq!(determine_proxy_type(&Some("socks5".to_string())), ProxyType::Socks5);
        assert_eq!(determine_proxy_type(&Some("SOCKS5 proxy".to_string())), ProxyType::Socks5);

        // Test SOCKS4 detection
        assert_eq!(determine_proxy_type(&Some("socks4".to_string())), ProxyType::Socks4);
        assert_eq!(determine_proxy_type(&Some("SOCKS4 server".to_string())), ProxyType::Socks4);

        // Test default fallback
        assert_eq!(determine_proxy_type(&Some("unknown service".to_string())), ProxyType::Http);
        assert_eq!(determine_proxy_type(&None), ProxyType::Http);
    }

    #[test]
    fn test_shodan_response_parsing() {
        let json_response = r#"
        {
            "matches": [
                {
                    "ip_str": "192.168.1.1",
                    "port": 8080,
                    "product": "squid"
                },
                {
                    "ip_str": "10.0.0.1", 
                    "port": 1080,
                    "product": "socks5"
                }
            ]
        }
        "#;

        let response: ShodanResponse = serde_json::from_str(json_response).unwrap();
        assert_eq!(response.matches.len(), 2);
        assert_eq!(response.matches[0].ip_str, "192.168.1.1");
        assert_eq!(response.matches[0].port, 8080);
        assert_eq!(response.matches[1].ip_str, "10.0.0.1");
        assert_eq!(response.matches[1].port, 1080);
    }
}