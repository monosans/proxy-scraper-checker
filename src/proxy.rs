use std::{
    hash::{Hash, Hasher},
    net::SocketAddr,
    str::FromStr,
    time::{Duration, Instant},
};

use color_eyre::eyre::eyre;

use crate::{
    config::{Config, HttpbinResponse},
    parsers::parse_ipv4,
};

#[derive(
    Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize,
)]
#[cfg_attr(feature = "tui", derive(strum::EnumCount))]
#[serde(rename_all = "lowercase")]
pub enum ProxyType {
    Http,
    Socks4,
    Socks5,
}

impl FromStr for ProxyType {
    type Err = crate::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("http") || s.eq_ignore_ascii_case("https") {
            Ok(Self::Http)
        } else if s.eq_ignore_ascii_case("socks4")
            || s.eq_ignore_ascii_case("socks4a")
        {
            Ok(Self::Socks4)
        } else if s.eq_ignore_ascii_case("socks5")
            || s.eq_ignore_ascii_case("socks5h")
        {
            Ok(Self::Socks5)
        } else {
            Err(eyre!("failed to convert {s} to ProxyType"))
        }
    }
}

impl ProxyType {
    const fn as_proxy_url_scheme(self, force_remote_dns: bool) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Socks4 => {
                if force_remote_dns {
                    "socks4a"
                } else {
                    "socks4"
                }
            }
            Self::Socks5 => "socks5h",
        }
    }

    pub const fn as_str_lowercase(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Socks4 => "socks4",
            Self::Socks5 => "socks5",
        }
    }

    #[cfg(feature = "tui")]
    pub const fn as_str_uppercase(self) -> &'static str {
        match self {
            Self::Http => "HTTP",
            Self::Socks4 => "SOCKS4",
            Self::Socks5 => "SOCKS5",
        }
    }
}

#[derive(Eq)]
pub struct Proxy {
    pub protocol: ProxyType,
    pub host: compact_str::CompactString,
    pub port: u16,
    pub username: Option<compact_str::CompactString>,
    pub password: Option<compact_str::CompactString>,
    pub timeout: Option<Duration>,
    pub exit_ip: Option<compact_str::CompactString>,
}

pub trait ProxySink {
    fn push_str(&mut self, s: &str);
    fn push_byte(&mut self, b: u8);
}

impl ProxySink for compact_str::CompactString {
    fn push_str(&mut self, s: &str) {
        self.push_str(s);
    }

    fn push_byte(&mut self, b: u8) {
        self.push(b.into());
    }
}

impl ProxySink for Vec<u8> {
    fn push_str(&mut self, s: &str) {
        self.extend_from_slice(s.as_bytes());
    }

    fn push_byte(&mut self, b: u8) {
        self.push(b);
    }
}

impl Proxy {
    fn as_reqwest_proxy(
        &self,
        force_remote_dns: bool,
    ) -> crate::Result<reqwest::Proxy> {
        let proxy = reqwest::Proxy::all(
            compact_str::format_compact!(
                "{}://{}:{}",
                self.protocol.as_proxy_url_scheme(force_remote_dns),
                self.host,
                self.port
            )
            .as_str(),
        )?;

        if let (Some(username), Some(password)) =
            (self.username.as_ref(), self.password.as_ref())
        {
            Ok(proxy.basic_auth(username, password))
        } else {
            Ok(proxy)
        }
    }

    pub async fn check(
        &mut self,
        config: &Config,
        check_url_addrs: &[SocketAddr],
        tls_backend: rustls::ClientConfig,
    ) -> crate::Result<()> {
        let Some(check_url) = config.checking.check_url.clone() else {
            return Ok(());
        };

        let force_remote_dns = check_url_addrs.is_empty();

        let mut builder = reqwest::ClientBuilder::new()
            .user_agent(config.checking.user_agent.as_bytes())
            .proxy(self.as_reqwest_proxy(force_remote_dns)?)
            .timeout(config.checking.timeout)
            .connect_timeout(config.checking.connect_timeout)
            .pool_idle_timeout(Duration::ZERO)
            .pool_max_idle_per_host(0)
            .http1_only()
            .tcp_keepalive(None)
            .tcp_keepalive_interval(None)
            .tcp_keepalive_retries(None)
            .tls_backend_preconfigured(tls_backend);

        if !force_remote_dns && let Some(host) = check_url.host_str() {
            builder = builder.resolve_to_addrs(host, check_url_addrs);
        }

        let request = builder.build()?.get(check_url);

        let start = Instant::now();
        let response = request.send().await?.error_for_status()?;

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

    pub fn write_to_sink<S>(&self, sink: &mut S, include_protocol: bool)
    where
        S: ProxySink,
    {
        if include_protocol {
            sink.push_str(self.protocol.as_str_lowercase());
            sink.push_str("://");
        }

        if let (Some(username), Some(password)) =
            (&self.username, &self.password)
        {
            sink.push_str(username);
            sink.push_byte(b':');
            sink.push_str(password);
            sink.push_byte(b'@');
        }

        sink.push_str(&self.host);
        sink.push_byte(b':');
        sink.push_str(itoa::Buffer::new().format(self.port));
    }

    pub fn to_string(
        &self,
        include_protocol: bool,
    ) -> compact_str::CompactString {
        let mut s = compact_str::CompactString::const_new("");
        self.write_to_sink(&mut s, include_protocol);
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
    fn hash<H>(&self, state: &mut H)
    where
        H: Hasher,
    {
        self.protocol.hash(state);
        self.host.hash(state);
        self.port.hash(state);
        self.username.hash(state);
        self.password.hash(state);
    }
}
