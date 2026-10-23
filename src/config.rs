use std::{path::PathBuf, sync::Arc, time::Duration};

use color_eyre::eyre::{WrapErr as _, eyre};

use crate::{
    HashMap,
    http::BasicAuth,
    proxy::{PerProxyType, ProxyType, ProxyTypeSet},
    raw_config,
    utils::is_container,
};

#[derive(serde::Deserialize)]
pub struct Source {
    pub url: compact_str::CompactString,
    #[serde(default)]
    pub basic_auth: Option<BasicAuth>,
    #[serde(default)]
    pub headers:
        Option<HashMap<compact_str::CompactString, compact_str::CompactString>>,
}

pub struct ScrapingConfig {
    pub max_proxies_per_source: usize,
    pub timeout: Duration,
    pub connect_timeout: Duration,
    pub proxy: Option<url::Url>,
    pub user_agent: compact_str::CompactString,
    pub sources: PerProxyType<Vec<Arc<Source>>>,
    pub enabled_protocols: ProxyTypeSet,
}

pub struct CheckingConfig {
    pub check_url: Option<url::Url>,
    pub max_concurrent_checks: usize,
    pub timeout: Duration,
    pub connect_timeout: Duration,
    pub user_agent: compact_str::CompactString,
}

#[derive(serde::Deserialize)]
pub struct TxtOutputConfig {
    pub enabled: bool,
}

#[derive(serde::Deserialize)]
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
    pub checking: CheckingConfig,
    pub output: OutputConfig,
}

async fn get_output_path(
    raw_config: &raw_config::RawConfig,
) -> crate::Result<PathBuf> {
    let output_path = if is_container().await {
        "./out".into()
    } else {
        raw_config.output.path.clone()
    };
    tokio::fs::create_dir_all(&output_path).await.wrap_err_with(|| {
        compact_str::format_compact!(
            "failed to create directory: {}",
            output_path.display()
        )
    })?;
    Ok(output_path)
}

fn get_max_concurrent_checks(raw_config: &raw_config::RawConfig) -> usize {
    let requested = raw_config.checking.max_concurrent_checks.get();
    let Ok(limit) = rlimit::increase_nofile_limit(u64::MAX) else {
        return requested;
    };
    let limit = usize::try_from(limit).unwrap_or(usize::MAX).max(1);

    if requested > limit {
        tracing::warn!(
            "max_concurrent_checks config value is too high for your OS. It \
             will be ignored and {limit} will be used."
        );
        limit
    } else {
        requested
    }
}

impl Config {
    pub const fn exit_ip_enabled(&self) -> bool {
        self.output.json.enabled && self.checking.check_url.is_some()
    }

    pub const fn asn_enabled(&self) -> bool {
        self.exit_ip_enabled() && self.output.json.include_asn
    }

    pub const fn geolocation_enabled(&self) -> bool {
        self.exit_ip_enabled() && self.output.json.include_geolocation
    }

    pub const fn protocol_is_enabled(&self, protocol: ProxyType) -> bool {
        self.scraping.enabled_protocols.contains(protocol)
    }

    async fn from_raw_config(
        raw_config: raw_config::RawConfig,
    ) -> crate::Result<Self> {
        if !raw_config.output.json.enabled && !raw_config.output.txt.enabled {
            return Err(eyre!(
                "at least one of 'output.json' or 'output.txt' must be \
                 enabled in config"
            ));
        }

        let output_path = get_output_path(&raw_config).await?;
        let max_concurrent_checks = get_max_concurrent_checks(&raw_config);

        let mut sources = PerProxyType::<Vec<Arc<Source>>>::default();
        let mut enabled_protocols = ProxyTypeSet::default();
        for (proxy_type, section) in [
            (ProxyType::Http, raw_config.scraping.http),
            (ProxyType::Socks4, raw_config.scraping.socks4),
            (ProxyType::Socks5, raw_config.scraping.socks5),
        ] {
            if section.enabled {
                enabled_protocols.insert(proxy_type);
                *sources.get_mut(proxy_type) = section
                    .urls
                    .into_iter()
                    .map(Into::into)
                    .map(Arc::new)
                    .collect();
            }
        }

        Ok(Self {
            debug: raw_config.debug,
            scraping: ScrapingConfig {
                max_proxies_per_source: raw_config
                    .scraping
                    .max_proxies_per_source,
                timeout: raw_config.scraping.timeout,
                connect_timeout: raw_config.scraping.connect_timeout,
                proxy: raw_config.scraping.proxy,
                user_agent: raw_config.scraping.user_agent,
                sources,
                enabled_protocols,
            },
            checking: CheckingConfig {
                check_url: raw_config.checking.check_url,
                max_concurrent_checks,
                timeout: raw_config.checking.timeout,
                connect_timeout: raw_config.checking.connect_timeout,
                user_agent: raw_config.checking.user_agent,
            },
            output: OutputConfig {
                path: output_path,
                sort_by_speed: raw_config.output.sort_by_speed,
                txt: raw_config.output.txt,
                json: raw_config.output.json,
            },
        })
    }
}

pub async fn load_config() -> crate::Result<Arc<Config>> {
    let raw_config =
        raw_config::read_config(&raw_config::get_config_path()).await?;
    Ok(Arc::new(Config::from_raw_config(raw_config).await?))
}
