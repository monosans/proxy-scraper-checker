use std::{
    hash::{Hash, Hasher},
    net::{Ipv4Addr, SocketAddr},
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

#[derive(Clone, Copy, Default)]
pub struct ProxyTypeSet(u8);

impl ProxyTypeSet {
    pub const fn contains(self, proxy_type: ProxyType) -> bool {
        self.0 & proxy_type.bit() != 0
    }

    pub const fn insert(&mut self, proxy_type: ProxyType) {
        self.0 |= proxy_type.bit();
    }

    pub fn iter(self) -> impl Iterator<Item = ProxyType> {
        [ProxyType::Http, ProxyType::Socks4, ProxyType::Socks5]
            .into_iter()
            .filter(move |&proxy_type| self.contains(proxy_type))
    }
}

impl FromIterator<ProxyType> for ProxyTypeSet {
    fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = ProxyType>,
    {
        let mut set = Self::default();
        for proxy_type in iter {
            set.insert(proxy_type);
        }
        set
    }
}

impl ProxyType {
    const fn bit(self) -> u8 {
        match self {
            Self::Http => 1 << 0_u8,
            Self::Socks4 => 1 << 1_u8,
            Self::Socks5 => 1 << 2_u8,
        }
    }

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

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ProxyAuth {
    pub username: compact_str::CompactString,
    pub password: compact_str::CompactString,
}

#[derive(Eq)]
pub struct Proxy {
    pub protocol: ProxyType,
    pub host: compact_str::CompactString,
    pub port: u16,
    pub auth: Option<Box<ProxyAuth>>,
    pub timeout: Option<Duration>,
    pub exit_ip: Option<Ipv4Addr>,
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

        if let Some(auth) = self.auth.as_ref() {
            Ok(proxy.basic_auth(&auth.username, &auth.password))
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
        let mut response = request.send().await?.error_for_status()?;

        if config.exit_ip_enabled() {
            let text = response.text().await?;
            self.timeout = Some(start.elapsed());

            self.exit_ip = if let Ok(httpbin) =
                serde_json::from_str::<HttpbinResponse>(&text)
            {
                parse_ipv4(&httpbin.origin)
            } else {
                parse_ipv4(&text)
            };
        } else {
            while response.chunk().await?.is_some() {}
            self.timeout = Some(start.elapsed());
        }

        Ok(())
    }

    pub fn write_to(&self, buf: &mut Vec<u8>, include_protocol: bool) {
        if include_protocol {
            buf.extend_from_slice(self.protocol.as_str_lowercase().as_bytes());
            buf.extend_from_slice(b"://");
        }

        if let Some(auth) = &self.auth {
            buf.extend_from_slice(auth.username.as_bytes());
            buf.push(b':');
            buf.extend_from_slice(auth.password.as_bytes());
            buf.push(b'@');
        }

        buf.extend_from_slice(self.host.as_bytes());
        buf.push(b':');
        buf.extend_from_slice(itoa::Buffer::new().format(self.port).as_bytes());
    }

    pub fn to_string(
        &self,
        include_protocol: bool,
    ) -> compact_str::CompactString {
        let mut buf = Vec::new();
        self.write_to(&mut buf, include_protocol);
        compact_str::CompactString::from_utf8_lossy(&buf)
    }
}

#[expect(clippy::missing_trait_methods)]
impl PartialEq for Proxy {
    fn eq(&self, other: &Self) -> bool {
        self.protocol == other.protocol
            && self.host == other.host
            && self.port == other.port
            && self.auth == other.auth
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
        self.auth.hash(state);
    }
}
