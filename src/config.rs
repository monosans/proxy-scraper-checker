use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use color_eyre::eyre::WrapErr as _;

use crate::{
    HashMap,
    http::BasicAuth,
    proxy::{ProxyType, ProxyTypeSet},
    raw_config,
    utils::is_container,
};

pub const APP_DIRECTORY_NAME: &str = "proxy_scraper_checker";

#[derive(serde::Deserialize)]
pub struct HttpbinResponse {
    pub origin: compact_str::CompactString,
}

pub struct Source {
    pub url: compact_str::CompactString,
    pub basic_auth: Option<BasicAuth>,
    pub headers:
        Option<HashMap<compact_str::CompactString, compact_str::CompactString>>,
}

pub struct ScrapingConfig {
    pub max_proxies_per_source: usize,
    pub timeout: Duration,
    pub connect_timeout: Duration,
    pub proxy: Option<url::Url>,
    pub user_agent: compact_str::CompactString,
    pub sources: HashMap<ProxyType, Vec<Arc<Source>>>,
    pub enabled_protocols: ProxyTypeSet,
}

pub struct CheckingConfig {
    pub check_url: Option<url::Url>,
    pub max_concurrent_checks: usize,
    pub timeout: Duration,
    pub connect_timeout: Duration,
    pub user_agent: compact_str::CompactString,
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
    pub checking: CheckingConfig,
    pub output: OutputConfig,
}

impl From<raw_config::SourceConfig> for Source {
    fn from(sc: raw_config::SourceConfig) -> Self {
        match sc {
            raw_config::SourceConfig::Simple(url) => {
                Self { url, basic_auth: None, headers: None }
            }
            raw_config::SourceConfig::Detailed(config) => Self {
                url: config.url,
                basic_auth: config.basic_auth,
                headers: config.headers,
            },
        }
    }
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

impl Config {
    pub const fn exit_ip_enabled(&self) -> bool {
        self.output.json.enabled
    }

    pub const fn asn_enabled(&self) -> bool {
        self.output.json.enabled && self.output.json.include_asn
    }

    pub const fn geolocation_enabled(&self) -> bool {
        self.output.json.enabled && self.output.json.include_geolocation
    }

    pub const fn protocol_is_enabled(&self, protocol: ProxyType) -> bool {
        self.scraping.enabled_protocols.contains(protocol)
    }

    pub async fn from_raw_config(
        raw_config: raw_config::RawConfig,
    ) -> crate::Result<Self> {
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

        let sources: HashMap<ProxyType, Vec<Arc<Source>>> = [
            (ProxyType::Http, raw_config.scraping.http),
            (ProxyType::Socks4, raw_config.scraping.socks4),
            (ProxyType::Socks5, raw_config.scraping.socks5),
        ]
        .into_iter()
        .filter_map(|(proxy_type, section)| {
            section.enabled.then(move || {
                (
                    proxy_type,
                    section
                        .urls
                        .into_iter()
                        .map(Into::into)
                        .map(Arc::new)
                        .collect(),
                )
            })
        })
        .collect();

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
                enabled_protocols: sources.keys().copied().collect(),
                sources,
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

pub async fn load_config() -> crate::Result<Arc<Config>> {
    let raw_config = {
        let raw_config_path = raw_config::get_config_path();
        raw_config::read_config(Path::new(&raw_config_path)).await
    }?;

    let config = Config::from_raw_config(raw_config).await?;

    Ok(Arc::new(config))
}
