use std::{collections::HashSet, path::PathBuf};

use color_eyre::eyre::WrapErr;
use serde::{Deserialize, Deserializer};

use crate::utils::is_http_url;

fn validate_positive_f64<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<f64, D::Error> {
    let val = f64::deserialize(deserializer)?;
    if val > 0.0 {
        Ok(val)
    } else {
        Err(serde::de::Error::custom("Value must be positive"))
    }
}

fn validate_positive_usize<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<usize, D::Error> {
    let val = usize::deserialize(deserializer)?;
    if val > 0 {
        Ok(val)
    } else {
        Err(serde::de::Error::custom("Value must be positive"))
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
            "{s:?} is not a valid 'http' or 'https' url"
        )))
    }
}

#[derive(Deserialize)]
pub(crate) struct RawConfig {
    #[serde(deserialize_with = "validate_positive_f64")]
    pub(crate) timeout: f64,
    #[serde(deserialize_with = "validate_positive_f64")]
    pub(crate) source_timeout: f64,
    pub(crate) proxies_per_source_limit: usize,
    #[serde(deserialize_with = "validate_positive_usize")]
    pub(crate) max_concurrent_checks: usize,
    #[serde(deserialize_with = "validate_http_url")]
    pub(crate) check_website: String,
    pub(crate) sort_by_speed: bool,
    pub(crate) enable_geolocation: bool,
    pub(crate) debug: bool,
    pub(crate) output: Output,
    pub(crate) http: ProxySection,
    pub(crate) socks4: ProxySection,
    pub(crate) socks5: ProxySection,
}

pub(crate) struct Output {
    pub(crate) path: PathBuf,
    pub(crate) json: bool,
    pub(crate) txt: bool,
}

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

        Ok(Output { path: inner.path, json: inner.json, txt: inner.txt })
    }
}

#[derive(Deserialize)]
pub(crate) struct ProxySection {
    pub(crate) enabled: bool,
    pub(crate) sources: HashSet<String>,
}

pub(crate) async fn read_config(path: &str) -> color_eyre::Result<RawConfig> {
    let raw_config = tokio::fs::read_to_string(path)
        .await
        .wrap_err_with(move || format!("failed to read {path} to string"))?;
    toml::from_str(&raw_config).wrap_err_with(move || {
        format!("failed to parse {path} as TOML config file")
    })
}
