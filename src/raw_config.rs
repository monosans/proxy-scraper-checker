use std::{collections::HashSet, env, num::NonZero, path::PathBuf};

use color_eyre::eyre::WrapErr as _;
use serde::{Deserialize, Deserializer};

use crate::utils::{is_docker, is_http_url};

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

    pub http: ScrapingProtocolConfig,
    pub socks4: ScrapingProtocolConfig,
    pub socks5: ScrapingProtocolConfig,
}

#[derive(Deserialize)]
pub struct CheckingConfig {
    #[serde(deserialize_with = "validate_http_url")]
    pub check_url: String,
    pub debug: bool,
    pub max_concurrent_checks: NonZero<usize>,
    #[serde(deserialize_with = "validate_positive_f64")]
    pub timeout: f64,
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

const CONFIG_ENV_VAR: &str = "PROXY_SCRAPER_CHECKER_CONFIG";
const DEFAULT_CONFIG_PATH: &str = "config.toml";

pub async fn get_config_path() -> String {
    if is_docker().await {
        DEFAULT_CONFIG_PATH.to_owned()
    } else if let Ok(config_path) = env::var(CONFIG_ENV_VAR) {
        config_path
    } else {
        DEFAULT_CONFIG_PATH.to_owned()
    }
}

pub async fn read_config(path: &str) -> color_eyre::Result<RawConfig> {
    let raw_config = tokio::fs::read_to_string(path)
        .await
        .wrap_err_with(move || format!("failed to read {path} to string"))?;
    toml::from_str(&raw_config).wrap_err_with(move || {
        format!("failed to parse {path} as TOML config file")
    })
}
