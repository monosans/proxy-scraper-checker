use std::{fmt, sync::Arc};

use color_eyre::eyre::{OptionExt as _, WrapErr as _, eyre};

use crate::{
    config::{CheckWebsiteType, Config, HttpbinResponse, USER_AGENT},
    parsers::parse_ipv4,
};

#[derive(serde::Serialize, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ProxyType {
    #[serde(rename = "http")]
    Http,
    #[serde(rename = "socks4")]
    Socks4,
    #[serde(rename = "socks5")]
    Socks5,
}

impl TryFrom<&str> for ProxyType {
    type Error = color_eyre::Report;

    fn try_from(string: &str) -> color_eyre::Result<Self> {
        match string {
            "http" | "https" => Ok(Self::Http),
            "socks4" => Ok(Self::Socks4),
            "socks5" => Ok(Self::Socks5),
            _ => Err(eyre!("Failed to convert {string} to ProxyType")),
        }
    }
}

impl fmt::Display for ProxyType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Http => "http",
                Self::Socks4 => "socks4",
                Self::Socks5 => "socks5",
            }
        )
    }
}

#[derive(derivative::Derivative, Eq)]
#[derivative(Hash, PartialEq)]
pub struct Proxy {
    pub protocol: ProxyType,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    #[derivative(Hash = "ignore")]
    #[derivative(PartialEq = "ignore")]
    pub timeout: Option<tokio::time::Duration>,
    #[derivative(Hash = "ignore")]
    #[derivative(PartialEq = "ignore")]
    pub exit_ip: Option<String>,
}

impl Proxy {
    pub async fn check(
        &mut self,
        config: Arc<Config>,
    ) -> color_eyre::Result<()> {
        let mut proxy = reqwest::Proxy::all(format!(
            "{}://{}:{}",
            self.protocol, self.host, self.port
        ))
        .wrap_err("failed to create reqwest::Proxy")?;
        if let (Some(username), Some(password)) =
            (self.username.as_ref(), self.password.as_ref())
        {
            proxy = proxy.basic_auth(username, password);
        }
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .proxy(proxy)
            .timeout(config.timeout)
            .pool_max_idle_per_host(0)
            .tcp_keepalive(None)
            .use_rustls_tls()
            .build()
            .wrap_err("failed to create reqwest::Client")?;
        let start = tokio::time::Instant::now();
        let response = client
            .get(&config.check_website)
            .send()
            .await
            .wrap_err_with(|| {
                format!(
                    "failed to send HTTP request to {}",
                    config.check_website
                )
            })?
            .error_for_status()
            .wrap_err("Got error HTTP status code when checking proxy")?;
        drop(client);
        self.timeout = Some(start.elapsed());
        self.exit_ip =
            match config.check_website_type {
                CheckWebsiteType::HttpbinIp => {
                    let httpbin = response
                        .json::<HttpbinResponse>()
                        .await
                        .wrap_err("failed to parse response as HttpBin")?;
                    Some(parse_ipv4(&httpbin.origin).ok_or_eyre(
                        "failed to parse ipv4 from httpbin response",
                    )?)
                }
                CheckWebsiteType::PlainIp => {
                    let text = response
                        .text()
                        .await
                        .wrap_err("failed to decode response text")?;
                    Some(parse_ipv4(&text).ok_or_eyre(
                        "failed to parse ipv4 from response text",
                    )?)
                }
                CheckWebsiteType::Unknown => None,
            };
        Ok(())
    }

    pub fn as_str(&self, include_protocol: bool) -> String {
        let mut s = String::new();
        if include_protocol {
            s.push_str(&self.protocol.to_string());
            s.push_str("://");
        }
        if let (Some(username), Some(password)) =
            (self.username.as_ref(), self.password.as_ref())
        {
            s.push_str(username);
            s.push(':');
            s.push_str(password);
            s.push('@');
        }
        s.push_str(&self.host);
        s.push(':');
        s.push_str(&self.port.to_string());
        s
    }
}
