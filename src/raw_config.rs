use std::{
    collections::HashSet,
    env,
    num::NonZero,
    path::{Path, PathBuf},
};

use color_eyre::eyre::WrapErr as _;
use serde::{Deserialize, Deserializer};

use crate::utils::is_http_url;

fn validate_positive_f64<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<f64, D::Error> {
    let val = f64::deserialize(deserializer)?;
    if val > 0.0 {
        Ok(val)
    } else {
        Err(serde::de::Error::custom("value must be positive"))
    }
}

fn validate_http_url<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let s = String::deserialize(deserializer)?;
    if s.is_empty() || is_http_url(&s) {
        Ok(s)
    } else {
        Err(serde::de::Error::custom(format!(
            "'{s}' is not a valid 'http' or 'https' url"
        )))
    }
}

#[derive(Deserialize)]
pub struct ScrapingProtocolConfig {
    pub enabled: bool,
    pub urls: HashSet<String>,
}

#[derive(Deserialize)]
pub struct ScrapingConfig {
    pub max_proxies_per_source: usize,
    #[serde(deserialize_with = "validate_positive_f64")]
    pub timeout: f64,
    #[serde(deserialize_with = "validate_positive_f64")]
    pub connect_timeout: f64,

    pub http: ScrapingProtocolConfig,
    pub socks4: ScrapingProtocolConfig,
    pub socks5: ScrapingProtocolConfig,
}

#[derive(Deserialize)]
pub struct CheckingConfig {
    #[serde(deserialize_with = "validate_http_url")]
    pub check_url: String,
    pub max_concurrent_checks: NonZero<usize>,
    #[serde(deserialize_with = "validate_positive_f64")]
    pub timeout: f64,
    #[serde(deserialize_with = "validate_positive_f64")]
    pub connect_timeout: f64,
}

#[derive(Deserialize)]
pub struct TxtOutputConfig {
    pub enabled: bool,
}

#[derive(Deserialize)]
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

#[derive(Deserialize)]
pub struct RawConfig {
    pub debug: bool,
    pub scraping: ScrapingConfig,
    pub checking: CheckingConfig,
    pub output: OutputConfig,
}

#[expect(clippy::missing_trait_methods)]
impl<'de> Deserialize<'de> for OutputConfig {
    fn deserialize<D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct InnerOutputConfig {
            pub path: PathBuf,
            pub sort_by_speed: bool,
            pub txt: TxtOutputConfig,
            pub json: JsonOutputConfig,
        }

        let inner = InnerOutputConfig::deserialize(deserializer)?;
        if !inner.json.enabled && !inner.txt.enabled {
            return Err(serde::de::Error::custom(
                "at least one of 'output.json' or 'output.txt' must be \
                 enabled in config",
            ));
        }

        Ok(Self {
            path: inner.path,
            sort_by_speed: inner.sort_by_speed,
            txt: inner.txt,
            json: inner.json,
        })
    }
}

const CONFIG_ENV: &str = "PROXY_SCRAPER_CHECKER_CONFIG";

pub fn get_config_path() -> String {
    env::var(CONFIG_ENV).unwrap_or_else(|_| "config.toml".to_owned())
}

pub async fn read_config(path: &Path) -> color_eyre::Result<RawConfig> {
    let raw_config =
        tokio::fs::read_to_string(path).await.wrap_err_with(move || {
            format!("failed to read {} to string", path.display())
        })?;
    toml::from_str(&raw_config).wrap_err_with(move || {
        format!("failed to parse {} as TOML config file", path.display())
    })
}
