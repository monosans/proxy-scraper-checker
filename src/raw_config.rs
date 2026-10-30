use std::{
    env,
    num::NonZero,
    path::{Path, PathBuf},
    time::Duration,
};

use color_eyre::eyre::WrapErr as _;
use serde::Deserialize as _;

use crate::config::{JsonOutputConfig, Source, TxtOutputConfig};

const CONFIG_ENV: &str = "PROXY_SCRAPER_CHECKER_CONFIG";

fn validate_timeout<'de, D>(deserializer: D) -> Result<Duration, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let secs = f64::deserialize(deserializer)?;
    if secs <= 0.0_f64 {
        return Err(serde::de::Error::custom("value must be positive"));
    }
    Duration::try_from_secs_f64(secs).map_err(serde::de::Error::custom)
}

fn validate_url<'de, D>(
    deserializer: D,
    allowed_schemes: &[&str],
) -> Result<Option<url::Url>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = compact_str::CompactString::deserialize(deserializer)?;
    if s.trim().is_empty() {
        return Ok(None);
    }
    match url::Url::parse(&s) {
        Ok(url)
            if allowed_schemes.contains(&url.scheme())
                && url.host_str().is_some() =>
        {
            Ok(Some(url))
        }
        _ => Err(serde::de::Error::custom(compact_str::format_compact!(
            "'{s}' is not a valid url with one of these schemes: {}",
            allowed_schemes.join(", ")
        ))),
    }
}

fn validate_proxy_url<'de, D>(
    deserializer: D,
) -> Result<Option<url::Url>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    validate_url(deserializer, &["http", "https", "socks4", "socks5"])
}

fn validate_http_url<'de, D>(
    deserializer: D,
) -> Result<Option<url::Url>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    validate_url(deserializer, &["http", "https"])
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
pub enum SourceConfig {
    Simple(compact_str::CompactString),
    Detailed(Box<Source>),
}

impl From<SourceConfig> for Source {
    fn from(source_config: SourceConfig) -> Self {
        match source_config {
            SourceConfig::Simple(url) => {
                Self { url, basic_auth: None, headers: None }
            }
            SourceConfig::Detailed(source) => *source,
        }
    }
}

#[derive(serde::Deserialize)]
pub struct ScrapingProtocolConfig {
    pub enabled: bool,
    pub urls: Vec<SourceConfig>,
}

#[derive(serde::Deserialize)]
pub struct ScrapingConfig {
    pub max_proxies_per_source: usize,
    #[serde(deserialize_with = "validate_timeout")]
    pub timeout: Duration,
    #[serde(deserialize_with = "validate_timeout")]
    pub connect_timeout: Duration,
    #[serde(deserialize_with = "validate_proxy_url")]
    pub proxy: Option<url::Url>,
    pub user_agent: compact_str::CompactString,

    pub http: ScrapingProtocolConfig,
    pub socks4: ScrapingProtocolConfig,
    pub socks5: ScrapingProtocolConfig,
}

#[derive(serde::Deserialize)]
pub struct CheckingConfig {
    #[serde(deserialize_with = "validate_http_url")]
    pub check_url: Option<url::Url>,
    pub max_concurrent_checks: NonZero<usize>,
    #[serde(deserialize_with = "validate_timeout")]
    pub timeout: Duration,
    #[serde(deserialize_with = "validate_timeout")]
    pub connect_timeout: Duration,
    pub user_agent: compact_str::CompactString,
}

#[derive(serde::Deserialize)]
pub struct OutputConfig {
    pub path: PathBuf,
    pub sort_by_speed: bool,
    pub txt: TxtOutputConfig,
    pub json: JsonOutputConfig,
}

#[derive(serde::Deserialize)]
pub struct RawConfig {
    pub debug: bool,
    pub scraping: ScrapingConfig,
    pub checking: CheckingConfig,
    pub output: OutputConfig,
}

pub fn get_config_path() -> PathBuf {
    env::var_os(CONFIG_ENV)
        .map_or_else(|| PathBuf::from("config.toml"), PathBuf::from)
}

pub async fn read_config(path: &Path) -> crate::Result<RawConfig> {
    let raw_config =
        tokio::fs::read_to_string(path).await.wrap_err_with(move || {
            compact_str::format_compact!(
                "failed to read file to string: {}",
                path.display()
            )
        })?;
    toml::from_str(&raw_config).wrap_err_with(move || {
        compact_str::format_compact!(
            "failed to parse TOML config file: {}",
            path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{RawConfig, SourceConfig};
    use crate::config::Source;

    const SHIPPED_CONFIG: &str = include_str!("../config.toml");

    fn source(toml: &str) -> Source {
        #[derive(serde::Deserialize)]
        struct Wrapper {
            source: SourceConfig,
        }

        toml::from_str::<Wrapper>(toml).unwrap().source.into()
    }

    #[test]
    fn shipped_config_parses() {
        let config: RawConfig = toml::from_str(SHIPPED_CONFIG).unwrap();
        assert_eq!(config.scraping.timeout, Duration::from_secs(10));
        assert!(config.checking.check_url.is_some());
    }

    #[test]
    fn source_can_be_a_bare_url() {
        let source = source(r#"source = "https://example.com/proxies.txt""#);
        assert_eq!(source.url, "https://example.com/proxies.txt");
        assert!(source.basic_auth.is_none());
        assert!(source.headers.is_none());
    }

    #[test]
    fn source_table_needs_only_a_url() {
        let source = source(r#"source = { url = "proxies.txt" }"#);
        assert_eq!(source.url, "proxies.txt");
        assert!(source.basic_auth.is_none());
        assert!(source.headers.is_none());
    }

    #[test]
    fn source_password_is_optional() {
        let source = source(
            r#"
            [source]
            url = "https://example.com/proxies.txt"
            basic_auth = { username = "user" }
            headers = { Authorization = "Bearer token" }
            "#,
        );
        let auth = source.basic_auth.unwrap();
        assert_eq!(auth.username, "user");
        assert!(auth.password.is_none());
        assert_eq!(
            source
                .headers
                .unwrap()
                .get("Authorization")
                .map(compact_str::CompactString::as_str),
            Some("Bearer token")
        );
    }

    #[test]
    fn empty_check_url_disables_checking() {
        let config: RawConfig = toml::from_str(&SHIPPED_CONFIG.replace(
            r#"check_url = "https://ipv4.icanhazip.com""#,
            r#"check_url = """#,
        ))
        .unwrap();
        assert!(config.checking.check_url.is_none());
    }

    #[test]
    fn timeouts_must_be_positive() {
        let config = SHIPPED_CONFIG.replace("timeout = 10.0", "timeout = 0.0");
        assert!(toml::from_str::<RawConfig>(&config).is_err());
    }
}
