use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use color_eyre::eyre::{OptionExt, WrapErr};
use serde::Deserialize;

use crate::{
    APP_DIRECTORY_NAME, parsers::parse_ipv4, proxy::ProxyType,
    raw_config::RawConfig, utils::is_docker,
};

#[derive(Clone)]
pub(crate) enum CheckWebsiteType {
    Unknown,
    PlainIp,
    HttpbinIp,
}

#[derive(Deserialize)]
pub(crate) struct HttpbinResponse {
    pub(crate) origin: String,
}

impl CheckWebsiteType {
    pub(crate) async fn guess(
        check_website: &str,
        http_client: reqwest::Client,
    ) -> Self {
        if check_website.is_empty() {
            return Self::Unknown;
        }

        let response = match http_client.get(check_website).send().await {
            Ok(resp) => resp,
            Err(err) => {
                log::error!(
                    "Failed to open check_website without proxy, it will be \
                     impossible to determine anonymity and geolocation of \
                     proxies: {err}",
                );
                return Self::Unknown;
            }
        };

        let response = match response.error_for_status() {
            Ok(response) => response,
            Err(err) => {
                if let Some(status) = err.status() {
                    log::error!(
                        "check_website returned error HTTP status code: \
                         {status}"
                    );
                } else {
                    log::error!(
                        "check_website returned error HTTP status code"
                    );
                }
                return Self::Unknown;
            }
        };

        let body = match response.text().await {
            Ok(text) => text,
            Err(err) => {
                log::error!("Failed to decode check_website response: {err}");
                return Self::Unknown;
            }
        };

        if let Ok(httpbin) = serde_json::from_str::<HttpbinResponse>(&body) {
            if parse_ipv4(&httpbin.origin).is_some() {
                return Self::HttpbinIp;
            }
            log::error!("Failed to parse ipv4 from httpbin response");
        } else if parse_ipv4(&body).is_some() {
            return Self::PlainIp;
        }

        Self::Unknown
    }

    pub(crate) fn supports_geolocation(&self) -> bool {
        match self {
            CheckWebsiteType::Unknown => false,
            CheckWebsiteType::PlainIp | CheckWebsiteType::HttpbinIp => true,
        }
    }

    pub(crate) fn headers(&self) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        if let CheckWebsiteType::HttpbinIp = self {
            headers.insert(
                reqwest::header::ACCEPT,
                "application/json".parse().unwrap(),
            );
        }
        headers
    }
}
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct Config {
    pub(crate) timeout: tokio::time::Duration,
    pub(crate) source_timeout: tokio::time::Duration,
    pub(crate) proxies_per_source_limit: usize,
    pub(crate) max_concurrent_checks: usize,
    pub(crate) check_website: String,
    pub(crate) check_website_type: CheckWebsiteType,
    pub(crate) sort_by_speed: bool,
    pub(crate) enable_geolocation: bool,
    pub(crate) debug: bool,
    pub(crate) output_path: PathBuf,
    pub(crate) output_json: bool,
    pub(crate) output_txt: bool,
    pub(crate) sources: HashMap<ProxyType, HashSet<String>>,
}

async fn get_output_path(
    raw_config: &RawConfig,
) -> color_eyre::Result<PathBuf> {
    let output_path = if is_docker().await {
        let mut path = tokio::task::spawn_blocking(dirs::data_local_dir)
            .await
            .wrap_err(
                "failed to spawn task for getting user's local data directory",
            )?
            .ok_or_eyre("failed to get user's local data directory")?;
        path.push(APP_DIRECTORY_NAME);
        path
    } else {
        raw_config.output.path.clone()
    };
    tokio::fs::create_dir_all(&output_path).await?;
    Ok(output_path)
}

impl Config {
    pub(crate) async fn from_raw_config(
        raw_config: RawConfig,
        http_client: reqwest::Client,
    ) -> color_eyre::Result<Self> {
        let (check_website_type, output_path) = tokio::try_join!(
            async {
                Ok(CheckWebsiteType::guess(
                    &raw_config.check_website,
                    http_client,
                )
                .await)
            },
            get_output_path(&raw_config)
        )?;

        Ok(Self {
            timeout: tokio::time::Duration::from_secs_f64(raw_config.timeout),
            source_timeout: tokio::time::Duration::from_secs_f64(
                raw_config.source_timeout,
            ),
            proxies_per_source_limit: raw_config.proxies_per_source_limit,
            max_concurrent_checks: raw_config.max_concurrent_checks,
            check_website: raw_config.check_website,
            check_website_type: check_website_type.clone(),
            sort_by_speed: raw_config.sort_by_speed,
            enable_geolocation: raw_config.enable_geolocation
                && check_website_type.supports_geolocation(),
            debug: raw_config.debug,
            output_path,
            output_json: raw_config.output.json,
            output_txt: raw_config.output.txt,
            sources: [
                (ProxyType::Http, raw_config.http),
                (ProxyType::Socks4, raw_config.socks4),
                (ProxyType::Socks5, raw_config.socks5),
            ]
            .into_iter()
            .filter(|(_, section)| section.enabled)
            .map(|(proxy_type, section)| (proxy_type, section.sources))
            .collect(),
        })
    }
}
