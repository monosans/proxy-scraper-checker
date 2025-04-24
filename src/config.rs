use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use color_eyre::eyre::{OptionExt as _, WrapErr as _};
use serde::Deserialize;

use crate::{
    APP_DIRECTORY_NAME, parsers::parse_ipv4, proxy::ProxyType,
    raw_config::RawConfig, utils::is_docker,
};

#[derive(Clone)]
pub enum CheckWebsiteType {
    Unknown,
    PlainIp,
    HttpbinIp,
}

#[derive(Deserialize)]
pub struct HttpbinResponse {
    pub origin: String,
}

impl CheckWebsiteType {
    pub async fn guess(
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

    pub const fn supports_geolocation(&self) -> bool {
        match self {
            Self::Unknown => false,
            Self::PlainIp | Self::HttpbinIp => true,
        }
    }
}

#[expect(clippy::struct_excessive_bools)]
pub struct Config {
    pub timeout: tokio::time::Duration,
    pub source_timeout: tokio::time::Duration,
    pub proxies_per_source_limit: usize,
    pub max_concurrent_checks: usize,
    pub check_website: String,
    pub check_website_type: CheckWebsiteType,
    pub sort_by_speed: bool,
    pub enable_geolocation: bool,
    pub debug: bool,
    pub output_path: PathBuf,
    pub output_json: bool,
    pub output_txt: bool,
    pub sources: HashMap<ProxyType, HashSet<String>>,
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
    pub async fn from_raw_config(
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

        let max_concurrent_checks =
            match rlimit::increase_nofile_limit(u64::MAX) {
                Ok(lim) => {
                    #[expect(clippy::as_conversions)]
                    #[expect(clippy::cast_possible_truncation)]
                    if raw_config.max_concurrent_checks > (lim as usize) {
                        log::warn!(
                            "max_concurrent_checks config value is too high \
                             for your OS. It will be ignored and {lim} will \
                             be used."
                        );
                        lim as usize
                    } else {
                        raw_config.max_concurrent_checks
                    }
                }
                Err(_) => raw_config.max_concurrent_checks,
            };

        Ok(Self {
            timeout: tokio::time::Duration::from_secs_f64(raw_config.timeout),
            source_timeout: tokio::time::Duration::from_secs_f64(
                raw_config.source_timeout,
            ),
            proxies_per_source_limit: raw_config.proxies_per_source_limit,
            max_concurrent_checks,
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
            .filter_map(|(proxy_type, section)| {
                section.enabled.then_some((proxy_type, section.sources))
            })
            .collect(),
        })
    }
}
