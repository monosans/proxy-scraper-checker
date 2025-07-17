use std::{
    collections::{HashMap, HashSet, hash_map},
    path::PathBuf,
};

use color_eyre::eyre::{OptionExt as _, WrapErr as _};

use crate::{proxy::ProxyType, raw_config::RawConfig, utils::is_docker};

pub const APP_DIRECTORY_NAME: &str = "proxy_scraper_checker";
pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
                              AppleWebKit/537.36 (KHTML, like Gecko) \
                              Chrome/138.0.0.0 Safari/537.36";

#[derive(serde::Deserialize)]
pub struct HttpbinResponse {
    pub origin: String,
}

pub struct DiscoveryConfig {
    pub enabled: bool,
    pub shodan_api_key: Option<String>,
    pub search_query: String,
    pub max_results: usize,
    pub timeout: tokio::time::Duration,
}

pub struct ScrapingConfig {
    pub max_proxies_per_source: usize,
    pub timeout: tokio::time::Duration,
    pub sources: HashMap<ProxyType, HashSet<String>>,
}

pub struct CheckingConfig {
    pub check_url: String,
    pub max_concurrent_checks: usize,
    pub timeout: tokio::time::Duration,
}

pub struct TxtOutputConfig {
    pub enabled: bool,
}

pub struct JsonOutputConfig {
    pub enabled: bool,
    pub include_asn: bool,
    pub include_geolocation: bool,
}

pub struct OutputConfig {
    pub path: PathBuf,
    pub sort_by_speed: bool,
    pub txt: TxtOutputConfig,
    pub json: JsonOutputConfig,
}

pub struct Config {
    pub debug: bool,
    pub scraping: ScrapingConfig,
    pub discovery: Option<DiscoveryConfig>,
    pub checking: CheckingConfig,
    pub output: OutputConfig,
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
    tokio::fs::create_dir_all(&output_path).await.wrap_err_with(|| {
        format!("failed to create output directory: {}", output_path.display())
    })?;
    Ok(output_path)
}

impl Config {
    pub const fn asn_enabled(&self) -> bool {
        self.output.json.enabled && self.output.json.include_asn
    }

    pub const fn geolocation_enabled(&self) -> bool {
        self.output.json.enabled && self.output.json.include_geolocation
    }

    pub fn enabled_protocols(
        &self,
    ) -> hash_map::Keys<'_, ProxyType, HashSet<String>> {
        self.scraping.sources.keys()
    }

    pub fn protocol_is_enabled(&self, protocol: &ProxyType) -> bool {
        self.scraping.sources.contains_key(protocol)
    }

    pub async fn from_raw_config(
        raw_config: RawConfig,
    ) -> color_eyre::Result<Self> {
        let output_path = get_output_path(&raw_config).await?;

        let max_concurrent_checks =
            if let Ok(lim) = rlimit::increase_nofile_limit(u64::MAX) {
                let lim = usize::try_from(lim).unwrap_or(usize::MAX);

                if raw_config.checking.max_concurrent_checks.get() > lim {
                    tracing::warn!(
                        "max_concurrent_checks config value is too high for \
                         your OS. It will be ignored and {lim} will be used."
                    );
                    lim
                } else {
                    raw_config.checking.max_concurrent_checks.get()
                }
            } else {
                raw_config.checking.max_concurrent_checks.get()
            };

        Ok(Self {
            debug: raw_config.debug,
            scraping: ScrapingConfig {
                max_proxies_per_source: raw_config
                    .scraping
                    .max_proxies_per_source,
                timeout: tokio::time::Duration::from_secs_f64(
                    raw_config.scraping.timeout,
                ),
                sources: [
                    (ProxyType::Http, raw_config.scraping.http),
                    (ProxyType::Socks4, raw_config.scraping.socks4),
                    (ProxyType::Socks5, raw_config.scraping.socks5),
                ]
                .into_iter()
                .filter_map(|(proxy_type, section)| {
                    section.enabled.then_some((proxy_type, section.urls))
                })
                .collect(),
            },
            discovery: raw_config.discovery.and_then(|d| {
                if d.enabled && d.shodan_api_key.is_some() {
                    Some(DiscoveryConfig {
                        enabled: d.enabled,
                        shodan_api_key: d.shodan_api_key,
                        search_query: d.search_query,
                        max_results: d.max_results,
                        timeout: tokio::time::Duration::from_secs_f64(d.timeout),
                    })
                } else {
                    None
                }
            }),
            checking: CheckingConfig {
                check_url: raw_config.checking.check_url,
                max_concurrent_checks,
                timeout: tokio::time::Duration::from_secs_f64(
                    raw_config.checking.timeout,
                ),
            },
            output: OutputConfig {
                path: output_path,
                sort_by_speed: raw_config.output.sort_by_speed,
                txt: TxtOutputConfig { enabled: raw_config.output.txt.enabled },
                json: JsonOutputConfig {
                    enabled: raw_config.output.json.enabled,
                    include_asn: raw_config.output.json.include_asn,
                    include_geolocation: raw_config
                        .output
                        .json
                        .include_geolocation,
                },
            },
        })
    }
}
