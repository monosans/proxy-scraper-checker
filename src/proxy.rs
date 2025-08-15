use std::{
    hash::{Hash, Hasher},
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};

use color_eyre::eyre::{WrapErr as _, eyre};

use crate::{
    config::{Config, HttpbinResponse},
    parsers::parse_ipv4,
};

#[derive(
    Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize,
)]
#[serde(rename_all = "lowercase")]
pub enum ProxyType {
    Http,
    Socks4,
    Socks5,
}

impl FromStr for ProxyType {
    type Err = crate::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "http" | "https" => Ok(Self::Http),
            "socks4" => Ok(Self::Socks4),
            "socks5" => Ok(Self::Socks5),
            _ => Err(eyre!("failed to convert {s} to ProxyType")),
        }
    }
}

impl ProxyType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Socks4 => "socks4",
            Self::Socks5 => "socks5",
        }
    }
}

#[derive(Eq)]
pub struct Proxy {
    pub protocol: ProxyType,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub timeout: Option<Duration>,
    pub exit_ip: Option<String>,
}

impl TryFrom<&mut Proxy> for reqwest::Proxy {
    type Error = crate::Error;

    fn try_from(value: &mut Proxy) -> Result<Self, Self::Error> {
        let proxy = Self::all(format!(
            "{}://{}:{}",
            value.protocol.as_str(),
            value.host,
            value.port
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

    pub async fn check<R: reqwest::dns::Resolve + 'static>(
        &mut self,
        config: &Config,
        dns_resolver: Arc<R>,
    ) -> crate::Result<()> {
        if let Some(check_url) = &config.checking.check_url {
            let builder = reqwest::ClientBuilder::new()
                .user_agent(&config.checking.user_agent)
                .proxy(self.try_into()?)
                .timeout(config.checking.timeout)
                .connect_timeout(config.checking.connect_timeout)
                .pool_max_idle_per_host(0)
                .http1_only()
                .tcp_keepalive(None)
                .tcp_keepalive_interval(Duration::ZERO)
                .tcp_keepalive_retries(0)
                .dns_resolver(dns_resolver);
            #[cfg(any(
                target_os = "android",
                target_os = "fuchsia",
                target_os = "linux"
            ))]
            let builder = builder.tcp_user_timeout(None);
            let client = builder.build()?;
            let start = Instant::now();
            let response = client
                .get(check_url.clone())
                .send()
                .await?
                .error_for_status()?;
            drop(client);
            self.timeout = Some(start.elapsed());
            self.exit_ip = response.text().await.map_or(None, |text| {
                if let Ok(httpbin) =
                    serde_json::from_str::<HttpbinResponse>(&text)
                {
                    parse_ipv4(&httpbin.origin)
                } else {
                    parse_ipv4(&text)
                }
            });
        }
        Ok(())
    }

    pub fn as_str(&self, include_protocol: bool) -> String {
        let mut s = String::new();
        if include_protocol {
            s.push_str(self.protocol.as_str());
            s.push_str("://");
        }
        if let (Some(username), Some(password)) =
            (&self.username, &self.password)
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
