use std::{num::NonZero, path::PathBuf, sync::Arc, time::Duration};

use color_eyre::eyre::{WrapErr as _, eyre};

use crate::{
    HashMap,
    http::BasicAuth,
    proxy::{PerProxyType, ProxyType, ProxyTypeSet},
    raw_config,
    utils::is_container,
};

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
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
    pub scraping: ScrapingConfig,
    pub checking: CheckingConfig,
    pub output: OutputConfig,
}

async fn get_output_path(
    configured: PathBuf,
    in_container: bool,
) -> crate::Result<PathBuf> {
    let output_path = if in_container {
        drop(configured);
        "./out".into()
    } else {
        configured
    };
    tokio::fs::create_dir_all(&output_path).await.wrap_err_with(|| {
        compact_str::format_compact!(
            "failed to create directory: {}",
            output_path.display()
        )
    })?;
    Ok(output_path)
}

// Left for what isn't a check: the event loop, the terminal, DNS, the output.
const RESERVED_FILE_DESCRIPTORS: usize = 16;

// `limit` is what raising the file descriptor limit as far as it goes got, if
// that worked.
fn get_max_concurrent_checks(
    requested: NonZero<usize>,
    limit: Option<u64>,
) -> usize {
    let requested = requested.get();
    let Some(limit) = limit else {
        return requested;
    };
    let limit = usize::try_from(limit)
        .unwrap_or(usize::MAX)
        .saturating_sub(RESERVED_FILE_DESCRIPTORS)
        .max(1);

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

    pub async fn from_raw_config(
        raw_config: raw_config::RawConfig,
    ) -> crate::Result<Self> {
        if !raw_config.output.json.enabled && !raw_config.output.txt.enabled {
            return Err(eyre!(
                "at least one of 'output.json' or 'output.txt' must be \
                 enabled in config"
            ));
        }

        // Every check builds its own client, so this would fail each of them,
        // and failed checks are only logged in debug mode.
        reqwest::header::HeaderValue::from_bytes(
            raw_config.checking.user_agent.as_bytes(),
        )
        .wrap_err("invalid 'checking.user_agent' in config")?;

        let output_path =
            get_output_path(raw_config.output.path, is_container().await)
                .await?;
        let max_concurrent_checks = get_max_concurrent_checks(
            raw_config.checking.max_concurrent_checks,
            rlimit::increase_nofile_limit(u64::MAX).ok(),
        );

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

#[cfg(test)]
pub mod tests {
    use std::{num::NonZero, path::PathBuf};

    use super::{Config, get_max_concurrent_checks, get_output_path};
    use crate::{
        raw_config::RawConfig,
        utils::tests::{capture_logs, listen_everywhere},
    };

    async fn test_config_result(
        edit: impl FnOnce(&mut RawConfig),
    ) -> crate::Result<Config> {
        listen_everywhere();
        let mut raw_config: RawConfig =
            toml::from_str(include_str!("../config.toml")).unwrap();
        raw_config.output.path = PathBuf::from("target");
        edit(&mut raw_config);
        Config::from_raw_config(raw_config).await
    }

    pub async fn test_config(edit: impl FnOnce(&mut RawConfig)) -> Config {
        test_config_result(edit).await.unwrap()
    }

    pub fn temp_directory(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "proxy-scraper-checker-{name}-{}",
            std::process::id()
        ));
        drop(std::fs::remove_dir_all(&path));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[tokio::test]
    async fn in_a_container_the_output_goes_to_its_volume() {
        let configured = temp_directory("configured-output");
        let path = get_output_path(configured.join("out"), true).await.unwrap();
        assert_eq!(path, PathBuf::from("./out"));
        assert!(path.is_dir());
        assert!(!configured.join("out").exists());

        let path =
            get_output_path(configured.join("out"), false).await.unwrap();
        assert_eq!(path, configured.join("out"));
        assert!(path.is_dir());

        std::fs::remove_dir_all(configured).unwrap();
    }

    #[tokio::test]
    async fn an_output_path_that_cannot_be_a_directory_is_refused() {
        let directory = temp_directory("output-under-a-file");
        let file = directory.join("file");
        std::fs::write(&file, "").unwrap();

        let error =
            test_config_result(|raw| raw.output.path = file.join("out"))
                .await
                .err()
                .unwrap();
        assert!(
            error.to_string().starts_with("failed to create directory: "),
            "{error}"
        );

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn concurrent_checks_stay_within_the_file_descriptor_limit() {
        let requested = NonZero::new(100).unwrap();
        let logs = capture_logs();

        assert_eq!(get_max_concurrent_checks(requested, None), 100);
        assert_eq!(get_max_concurrent_checks(requested, Some(u64::MAX)), 100);
        assert_eq!(get_max_concurrent_checks(requested, Some(116)), 100);
        assert!(logs.text().is_empty(), "{}", logs.text());

        assert_eq!(get_max_concurrent_checks(requested, Some(66)), 50);
        assert!(logs.text().contains("50 will be used"), "{}", logs.text());
        // Even a limit too low to leave anything still allows one.
        assert_eq!(get_max_concurrent_checks(requested, Some(3)), 1);
    }

    #[tokio::test]
    async fn at_least_one_output_is_required() {
        let mut raw_config: RawConfig =
            toml::from_str(include_str!("../config.toml")).unwrap();
        raw_config.output.json.enabled = false;
        raw_config.output.txt.enabled = false;
        assert!(Config::from_raw_config(raw_config).await.is_err());
    }

    #[tokio::test]
    async fn a_check_user_agent_that_cannot_be_sent_is_refused() {
        let mut raw_config: RawConfig =
            toml::from_str(include_str!("../config.toml")).unwrap();
        raw_config.checking.user_agent = "a\nb".into();
        assert!(Config::from_raw_config(raw_config).await.is_err());
    }

    #[tokio::test]
    async fn disabled_protocols_get_no_sources() {
        let config = test_config(|raw| {
            raw.scraping.http.enabled = true;
            raw.scraping.socks4.enabled = false;
            raw.scraping.socks5.enabled = true;
        })
        .await;

        assert_eq!(config.scraping.enabled_protocols.iter().count(), 2);
        assert!(
            !config
                .scraping
                .sources
                .get(crate::proxy::ProxyType::Http)
                .is_empty()
        );
        assert!(
            config
                .scraping
                .sources
                .get(crate::proxy::ProxyType::Socks4)
                .is_empty()
        );
    }

    #[tokio::test]
    async fn exit_ip_data_needs_json_and_checking() {
        let enabled = |config: &Config| {
            (
                config.exit_ip_enabled(),
                config.asn_enabled(),
                config.geolocation_enabled(),
            )
        };

        let config = test_config(|raw| {
            raw.output.json.include_asn = true;
            raw.output.json.include_geolocation = false;
        })
        .await;
        assert_eq!(enabled(&config), (true, true, false));
        drop(config);

        let config = test_config(|raw| {
            raw.output.json.include_asn = true;
            raw.checking.check_url = None;
        })
        .await;
        assert_eq!(enabled(&config), (false, false, false));
        drop(config);

        let config = test_config(|raw| {
            raw.output.json.enabled = false;
            raw.output.json.include_geolocation = true;
        })
        .await;
        assert_eq!(enabled(&config), (false, false, false));
    }
}
