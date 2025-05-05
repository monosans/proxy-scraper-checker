use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use color_eyre::eyre::{OptionExt as _, WrapErr as _};

use crate::{proxy::ProxyType, raw_config::RawConfig, utils::is_docker};

pub const APP_DIRECTORY_NAME: &str = "proxy_scraper_checker";
pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
                              AppleWebKit/537.36 (KHTML, like Gecko) \
                              Chrome/135.0.0.0 Safari/537.36";

#[derive(serde::Deserialize)]
pub struct HttpbinResponse {
    pub origin: String,
}

#[expect(clippy::struct_excessive_bools)]
pub struct Config {
    pub timeout: tokio::time::Duration,
    pub source_timeout: tokio::time::Duration,
    pub proxies_per_source_limit: usize,
    pub max_concurrent_checks: usize,
    pub check_website: String,
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
    tokio::fs::create_dir_all(&output_path).await.wrap_err_with(|| {
        format!("failed to create output directory: {}", output_path.display())
    })?;
    Ok(output_path)
}

impl Config {
    pub async fn from_raw_config(
        raw_config: RawConfig,
    ) -> color_eyre::Result<Self> {
        let output_path = get_output_path(&raw_config).await?;

        let max_concurrent_checks =
            match rlimit::increase_nofile_limit(u64::MAX) {
                Ok(lim) => {
                    let lim = usize::try_from(lim).unwrap_or(usize::MAX);

                    if raw_config.max_concurrent_checks > lim {
                        log::warn!(
                            "max_concurrent_checks config value is too high \
                             for your OS. It will be ignored and {lim} will \
                             be used."
                        );
                        lim
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
            sort_by_speed: raw_config.sort_by_speed,
            enable_geolocation: raw_config.enable_geolocation,
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
            .filter_map(move |(proxy_type, section)| {
                section.enabled.then_some((proxy_type, section.sources))
            })
            .collect(),
        })
    }
}
