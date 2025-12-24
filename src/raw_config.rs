use std::{
    env,
    num::NonZero,
    path::{Path, PathBuf},
};

use color_eyre::eyre::WrapErr as _;
use serde::Deserialize as _;

use crate::{HashMap, http::BasicAuth};

fn validate_positive_f64<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<f64, D::Error> {
    let val = f64::deserialize(deserializer)?;
    if val > 0.0 {
        Ok(val)
    } else {
        Err(serde::de::Error::custom("value must be positive"))
    }
}

fn validate_url_generic<'de, D>(
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
    if let Ok(u) = url::Url::parse(&s)
        && allowed_schemes.contains(&u.scheme())
        && u.host_str().is_some()
    {
        Ok(Some(u))
    } else {
        let type_label = match allowed_schemes {
            [] => compact_str::CompactString::const_new(""),
            [single] => compact_str::format_compact!("'{single}'"),
            [rest @ .., last] => {
                let mut t = compact_str::CompactString::const_new("");
                let mut first = true;
                for s in rest {
                    if first {
                        first = false;
                    } else {
                        t.push_str(", ");
                    }
                    t.push('\'');
                    t.push_str(s);
                    t.push('\'');
                }
                t.push_str(" or '");
                t.push_str(last);
                t.push('\'');
                t
            }
        };
        Err(serde::de::Error::custom(compact_str::format_compact!(
            "'{s}' is not a valid {type_label} url"
        )))
    }
}

fn validate_proxy_url<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<url::Url>, D::Error> {
    validate_url_generic(deserializer, &["http", "https", "socks4", "socks5"])
}

fn validate_http_url<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<url::Url>, D::Error> {
    validate_url_generic(deserializer, &["http", "https"])
}

#[derive(serde::Deserialize)]
pub struct DetailedSourceConfig {
    pub url: String,
    #[serde(default)]
    pub basic_auth: Option<BasicAuth>,
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
pub enum SourceConfig {
    Simple(String),
    Detailed(Box<DetailedSourceConfig>),
}

#[derive(serde::Deserialize)]
pub struct ScrapingProtocolConfig {
    pub enabled: bool,
    pub urls: Vec<SourceConfig>,
}

#[derive(serde::Deserialize)]
pub struct ScrapingConfig {
    pub max_proxies_per_source: usize,
    #[serde(deserialize_with = "validate_positive_f64")]
    pub timeout: f64,
    #[serde(deserialize_with = "validate_positive_f64")]
    pub connect_timeout: f64,
    #[serde(deserialize_with = "validate_proxy_url")]
    pub proxy: Option<url::Url>,
    pub user_agent: String,

    pub http: ScrapingProtocolConfig,
    pub socks4: ScrapingProtocolConfig,
    pub socks5: ScrapingProtocolConfig,
}

#[derive(serde::Deserialize)]
pub struct CheckingConfig {
    #[serde(deserialize_with = "validate_http_url")]
    pub check_url: Option<url::Url>,
    pub max_concurrent_checks: NonZero<usize>,
    #[serde(deserialize_with = "validate_positive_f64")]
    pub timeout: f64,
    #[serde(deserialize_with = "validate_positive_f64")]
    pub connect_timeout: f64,
    pub user_agent: String,
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

#[derive(serde::Deserialize)]
pub struct RawConfig {
    pub debug: bool,
    pub scraping: ScrapingConfig,
    pub checking: CheckingConfig,
    pub output: OutputConfig,
}

#[expect(clippy::missing_trait_methods)]
impl<'de> serde::Deserialize<'de> for OutputConfig {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
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

pub fn get_config_path() -> compact_str::CompactString {
    env::var(CONFIG_ENV).map_or_else(
        move |_| compact_str::CompactString::const_new("config.toml"),
        Into::into,
    )
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
