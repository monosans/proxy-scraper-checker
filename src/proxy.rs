use std::{
    fmt::{self, Write as _},
    hash::{Hash, Hasher},
    str::FromStr,
};

use color_eyre::eyre::{WrapErr as _, eyre};

use crate::{
    config::{Config, HttpbinResponse},
    http::USER_AGENT,
    parsers::parse_ipv4,
};

#[derive(serde::Serialize, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum ProxyType {
    Http,
    Socks4,
    Socks5,
}

impl FromStr for ProxyType {
    type Err = color_eyre::Report;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "http" | "https" => Ok(Self::Http),
            "socks4" => Ok(Self::Socks4),
            "socks5" => Ok(Self::Socks5),
            _ => Err(eyre!("failed to convert {s} to ProxyType")),
        }
    }
}

impl fmt::Display for ProxyType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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

#[derive(Clone, Eq)]
pub struct Proxy {
    pub protocol: ProxyType,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub timeout: Option<tokio::time::Duration>,
    pub exit_ip: Option<String>,
}

impl TryFrom<&mut Proxy> for reqwest::Proxy {
    type Error = color_eyre::Report;

    fn try_from(value: &mut Proxy) -> Result<Self, Self::Error> {
        let proxy = Self::all(format!(
            "{}://{}:{}",
            value.protocol, value.host, value.port
        ))
        .wrap_err("failed to create reqwest::Proxy")?;

        if let (Some(username), Some(password)) =
            (value.username.as_ref(), value.password.as_ref())
        {
            Ok(proxy.basic_auth(username, password))
        } else {
            Ok(proxy)
        }
    }
}

impl Proxy {
    pub const fn is_checked(&self) -> bool {
        self.timeout.is_some()
    }

    pub async fn check(&mut self, config: &Config) -> color_eyre::Result<()> {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .proxy(self.try_into()?)
            .timeout(config.checking.timeout)
            .connect_timeout(config.checking.connect_timeout)
            .pool_max_idle_per_host(0)
            .tcp_keepalive(None)
            .use_rustls_tls()
            .build()
            .wrap_err("failed to create reqwest::Client")?;
        let start = tokio::time::Instant::now();
        let response = client
            .get(&config.checking.check_url)
            .send()
            .await?
            .error_for_status()?;
        drop(client);
        self.timeout = Some(start.elapsed());
        self.exit_ip = response.text().await.map_or(None, |text| {
            if let Ok(httpbin) = serde_json::from_str::<HttpbinResponse>(&text)
            {
                parse_ipv4(&httpbin.origin)
            } else {
                parse_ipv4(&text)
            }
        });
        Ok(())
    }

    pub fn as_str(&self, include_protocol: bool) -> String {
        let mut s = String::new();
        if include_protocol {
            write!(&mut s, "{}://", self.protocol).unwrap();
        }
        if let (Some(username), Some(password)) =
            (self.username.as_ref(), self.password.as_ref())
        {
            write!(&mut s, "{username}:{password}@").unwrap();
        }
        write!(&mut s, "{}:{}", self.host, self.port).unwrap();
        s
    }
}

#[expect(clippy::missing_trait_methods)]
impl PartialEq for Proxy {
    fn eq(&self, other: &Self) -> bool {
        self.protocol == other.protocol
            && self.host == other.host
            && self.port == other.port
            && self.username == other.username
            && self.password == other.password
    }
}

#[expect(clippy::missing_trait_methods)]
impl Hash for Proxy {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.protocol.hash(state);
        self.host.hash(state);
        self.port.hash(state);
        self.username.hash(state);
        self.password.hash(state);
    }
}
