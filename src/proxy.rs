use std::{
    hash::{Hash, Hasher},
    str::FromStr,
    sync::Arc,
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
        } else if s.eq_ignore_ascii_case("socks4") {
            Ok(Self::Socks4)
        } else if s.eq_ignore_ascii_case("socks5") {
            Ok(Self::Socks5)
        } else {
            Err(eyre!("failed to convert {s} to ProxyType"))
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
    pub host: compact_str::CompactString,
    pub port: u16,
    pub username: Option<compact_str::CompactString>,
    pub password: Option<compact_str::CompactString>,
    pub timeout: Option<Duration>,
    pub exit_ip: Option<compact_str::CompactString>,
}

impl TryFrom<&mut Proxy> for reqwest::Proxy {
    type Error = crate::Error;

    #[inline]
    fn try_from(value: &mut Proxy) -> Result<Self, Self::Error> {
        let proxy = Self::all(format!(
            "{}://{}:{}",
            value.protocol.as_str(),
            value.host,
            value.port
        ))?;

        if let (Some(username), Some(password)) =
            (value.username.as_ref(), value.password.as_ref())
        {
            Ok(proxy.basic_auth(username, password))
        } else {
            Ok(proxy)
        }
    }
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
        self.push(b as char);
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
    pub const fn is_checked(&self) -> bool {
        self.timeout.is_some()
    }

    pub async fn check<R: reqwest::dns::Resolve + 'static>(
        &mut self,
        config: &Config,
        dns_resolver: Arc<R>,
    ) -> crate::Result<()> {
        if let Some(check_url) = config.checking.check_url.clone() {
            let builder = reqwest::ClientBuilder::new()
                .user_agent(config.checking.user_agent.as_bytes())
                .proxy(self.try_into()?)
                .timeout(config.checking.timeout)
                .connect_timeout(config.checking.connect_timeout)
                .pool_idle_timeout(Duration::ZERO)
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
            let request = {
                let client = builder.build()?;
                client.get(check_url)
            };
            let start = Instant::now();
            let response = request.send().await?.error_for_status()?;
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

    pub fn write_to_sink<S: ProxySink>(
        &self,
        sink: &mut S,
        include_protocol: bool,
    ) {
        if include_protocol {
            sink.push_str(self.protocol.as_str());
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
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.protocol.hash(state);
        self.host.hash(state);
        self.port.hash(state);
        self.username.hash(state);
        self.password.hash(state);
    }
}
