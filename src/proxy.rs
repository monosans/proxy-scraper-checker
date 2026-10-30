use std::{
    hash::{Hash, Hasher},
    net::{Ipv4Addr, SocketAddr},
    str::FromStr,
    time::{Duration, Instant},
};

use color_eyre::eyre::eyre;

use crate::{config::Config, parsers::parse_ipv4};

#[derive(serde::Deserialize)]
struct HttpbinResponse {
    origin: compact_str::CompactString,
}

#[repr(u8)]
#[derive(
    Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize,
)]
#[serde(rename_all = "lowercase")]
pub enum ProxyType {
    Http = 1 << 0_u8,
    Socks4 = 1 << 1_u8,
    Socks5 = 1 << 2_u8,
}

impl ProxyType {
    const ALL: [Self; 3] = [Self::Http, Self::Socks4, Self::Socks5];

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
        self.0 & proxy_type as u8 != 0
    }

    pub const fn insert(&mut self, proxy_type: ProxyType) {
        self.0 |= proxy_type as u8;
    }

    pub fn iter(self) -> impl Iterator<Item = ProxyType> {
        ProxyType::ALL
            .into_iter()
            .filter(move |&proxy_type| self.contains(proxy_type))
    }
}

#[derive(Clone, Copy, Default)]
pub struct PerProxyType<T> {
    http: T,
    socks4: T,
    socks5: T,
}

impl<T> PerProxyType<T> {
    pub const fn get(&self, proxy_type: ProxyType) -> &T {
        match proxy_type {
            ProxyType::Http => &self.http,
            ProxyType::Socks4 => &self.socks4,
            ProxyType::Socks5 => &self.socks5,
        }
    }

    pub const fn get_mut(&mut self, proxy_type: ProxyType) -> &mut T {
        match proxy_type {
            ProxyType::Http => &mut self.http,
            ProxyType::Socks4 => &mut self.socks4,
            ProxyType::Socks5 => &mut self.socks5,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
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
    ) -> reqwest::Result<reqwest::Proxy> {
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
    ) -> reqwest::Result<()> {
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

    pub fn write_to(&self, buf: &mut String, include_protocol: bool) {
        if include_protocol {
            buf.push_str(self.protocol.as_str_lowercase());
            buf.push_str("://");
        }

        if let Some(auth) = &self.auth {
            buf.push_str(&auth.username);
            buf.push(':');
            buf.push_str(&auth.password);
            buf.push('@');
        }

        buf.push_str(&self.host);
        buf.push(':');
        buf.push_str(itoa::Buffer::new().format(self.port));
    }

    pub fn to_compact_string(
        &self,
        include_protocol: bool,
    ) -> compact_str::CompactString {
        let mut buf = String::new();
        self.write_to(&mut buf, include_protocol);
        buf.into()
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

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use compact_str::CompactString;

    use super::{Duration, Proxy, ProxyAuth, ProxyType, ProxyTypeSet};

    fn proxy(auth: Option<ProxyAuth>) -> Proxy {
        Proxy {
            protocol: ProxyType::Socks5,
            host: CompactString::const_new("1.2.3.4"),
            port: 1080,
            auth: auth.map(Box::new),
            timeout: None,
            exit_ip: None,
        }
    }

    fn auth() -> ProxyAuth {
        ProxyAuth {
            username: CompactString::const_new("user"),
            password: CompactString::const_new("pass"),
        }
    }

    #[test]
    fn scheme_aliases_collapse_to_one_protocol() {
        for (input, expected) in [
            ("http", "http"),
            ("HTTPS", "http"),
            ("socks4", "socks4"),
            ("Socks4a", "socks4"),
            ("socks5", "socks5"),
            ("SOCKS5H", "socks5"),
        ] {
            assert_eq!(
                input.parse::<ProxyType>().unwrap().as_str_lowercase(),
                expected
            );
        }
        assert!("ftp".parse::<ProxyType>().is_err());
    }

    #[test]
    fn proxy_type_set_iterates_in_protocol_order() {
        let mut set = ProxyTypeSet::default();
        set.insert(ProxyType::Socks5);
        set.insert(ProxyType::Http);

        let names: Vec<_> =
            set.iter().map(ProxyType::as_str_lowercase).collect();
        assert_eq!(names, ["http", "socks5"]);
        assert!(set.contains(ProxyType::Http));
        assert!(!set.contains(ProxyType::Socks4));
    }

    #[test]
    fn text_output_matches_documented_format() {
        assert_eq!(proxy(None).to_compact_string(false), "1.2.3.4:1080");
        assert_eq!(
            proxy(None).to_compact_string(true),
            "socks5://1.2.3.4:1080"
        );
        assert_eq!(
            proxy(Some(auth())).to_compact_string(false),
            "user:pass@1.2.3.4:1080"
        );
        assert_eq!(
            proxy(Some(auth())).to_compact_string(true),
            "socks5://user:pass@1.2.3.4:1080"
        );
    }

    #[test]
    fn deduplication_ignores_check_results() {
        let mut measured = proxy(None);
        measured.timeout = Some(Duration::from_secs(1));
        measured.exit_ip = Some(Ipv4Addr::LOCALHOST);

        let mut seen = crate::HashSet::default();
        assert!(seen.insert(measured));
        assert!(!seen.insert(proxy(None)));
        assert!(seen.insert(proxy(Some(auth()))));
    }
}
