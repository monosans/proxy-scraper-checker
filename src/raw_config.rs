use std::{collections::HashSet, env, path::PathBuf};

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

fn validate_positive_usize<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<usize, D::Error> {
    let val = usize::deserialize(deserializer)?;
    if val > 0 {
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
pub struct RawConfig {
    #[serde(deserialize_with = "validate_positive_f64")]
    pub timeout: f64,
    #[serde(deserialize_with = "validate_positive_f64")]
    pub source_timeout: f64,
    pub proxies_per_source_limit: usize,
    #[serde(deserialize_with = "validate_positive_usize")]
    pub max_concurrent_checks: usize,
    #[serde(deserialize_with = "validate_http_url")]
    pub check_website: String,
    pub sort_by_speed: bool,
    pub enable_geolocation: bool,
    pub debug: bool,
    pub output: Output,
    pub http: ProxySection,
    pub socks4: ProxySection,
    pub socks5: ProxySection,
}

pub struct Output {
    pub path: PathBuf,
    pub json: bool,
    pub txt: bool,
}

#[expect(clippy::missing_trait_methods)]
impl<'de> Deserialize<'de> for Output {
    fn deserialize<D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct InnerOutput {
            path: PathBuf,
            json: bool,
            txt: bool,
        }

        let inner = InnerOutput::deserialize(deserializer)?;
        if !inner.json && !inner.txt {
            return Err(serde::de::Error::custom(
                "at least one of 'output.json' or 'output.txt' must be \
                 enabled in config",
            ));
        }

        Ok(Self { path: inner.path, json: inner.json, txt: inner.txt })
    }
}

#[derive(Deserialize)]
pub struct ProxySection {
    pub enabled: bool,
    pub sources: HashSet<String>,
}

const CONFIG_ENV_VAR: &str = "PROXY_SCRAPER_CHECKER_CONFIG";

pub fn get_config_path() -> String {
    env::var(CONFIG_ENV_VAR).unwrap_or_else(|_| "config.toml".to_owned())
}

pub async fn read_config(path: &str) -> color_eyre::Result<RawConfig> {
    let raw_config = tokio::fs::read_to_string(path)
        .await
        .wrap_err_with(move || format!("failed to read {path} to string"))?;
    toml::from_str(&raw_config).wrap_err_with(move || {
        format!("failed to parse {path} as TOML config file")
    })
}
