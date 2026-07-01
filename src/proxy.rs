use std::{
    hash::{Hash, Hasher},
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

impl TryFrom<&mut Proxy> for reqwest::Proxy {
    type Error = crate::Error;

    #[inline]
    fn try_from(value: &mut Proxy) -> Result<Self, Self::Error> {
        let proxy = Self::all(
            compact_str::format_compact!(
                "{}://{}:{}",
                value.protocol.as_str_lowercase(),
                value.host,
                value.port
            )
            .as_str(),
        )?;

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
    pub async fn check(
        &mut self,
        config: &Config,
        dns_resolver: crate::http::HickoryDnsResolver,
        tls_backend: rustls::ClientConfig,
    ) -> crate::Result<()> {
        if config.checking.dnsbl.enabled && config.checking.dnsbl.check_host {
            Self::check_dnsbl(
                &self.host,
                &config.checking.dnsbl.lists,
                &dns_resolver,
                config.checking.dnsbl.strict,
            )
            .await?;
        }

        let Some(check_url) = config.checking.check_url.clone() else {
            return Ok(());
        };

        let request = reqwest::ClientBuilder::new()
            .user_agent(config.checking.user_agent.as_bytes())
            .proxy(self.try_into()?)
            .timeout(config.checking.timeout)
            .connect_timeout(config.checking.connect_timeout)
            .pool_idle_timeout(Duration::ZERO)
            .pool_max_idle_per_host(0)
            .http1_only()
            .tcp_keepalive(None)
            .tcp_keepalive_interval(None)
            .tcp_keepalive_retries(None)
            .tls_backend_preconfigured(tls_backend)
            .dns_resolver(dns_resolver.clone())
            .build()?
            .get(check_url);

        let start = Instant::now();
        let response = request.send().await?.error_for_status()?;

        self.timeout = Some(start.elapsed());
        let exit_ip_str = response.text().await.map_or(None, |text| {
            if let Ok(httpbin) = serde_json::from_str::<HttpbinResponse>(&text)
            {
                parse_ipv4(&httpbin.origin)
            } else {
                parse_ipv4(&text)
            }
        });
        self.exit_ip.clone_from(&exit_ip_str);

        if config.checking.dnsbl.enabled
            && config.checking.dnsbl.check_exit_ip
            && let Some(exit_ip) = exit_ip_str
        {
            Self::check_dnsbl(
                &exit_ip,
                &config.checking.dnsbl.lists,
                &dns_resolver,
                config.checking.dnsbl.strict,
            )
            .await?;
        }

        Ok(())
    }

    async fn check_dnsbl(
        host_or_ip: &str,
        dnsbl_lists: &[compact_str::CompactString],
        dns_resolver: &crate::http::HickoryDnsResolver,
        strict: bool,
    ) -> crate::Result<()> {
        let candidates =
            if let Ok(ipv4) = std::net::Ipv4Addr::from_str(host_or_ip) {
                vec![ipv4]
            } else {
                match dns_resolver.lookup_ip(host_or_ip).await {
                    Ok(response) => response
                        .iter()
                        .filter_map(|ip| match ip {
                            std::net::IpAddr::V4(v4) => Some(v4),
                            std::net::IpAddr::V6(_) => None,
                        })
                        .collect(),
                    Err(e) => {
                        if strict {
                            return Err(eyre!(
                                "DNSBL host resolution failed for {}: {}",
                                host_or_ip,
                                e
                            ));
                        }
                        vec![]
                    }
                }
            };

        for ipv4 in candidates {
            let octets = ipv4.octets();
            let reversed = compact_str::format_compact!(
                "{}.{}.{}.{}",
                octets[3],
                octets[2],
                octets[1],
                octets[0]
            );
            for list in dnsbl_lists {
                let list = list.trim_end_matches('.');
                let query = compact_str::format_compact!("{reversed}.{list}.");
                match dns_resolver.lookup_ip_raw(&query).await {
                    Ok(response) => {
                        if response.iter().next().is_some() {
                            return Err(eyre!(
                                "IP {} is blacklisted in {}",
                                ipv4,
                                list
                            ));
                        }
                    }
                    Err(e) => {
                        if e.is_no_records_found() || e.is_nx_domain() {
                            // NXDOMAIN or no records means the IP is clean
                            continue;
                        }
                        if strict {
                            return Err(eyre!(
                                "DNSBL {} lookup failed for IP {}: {}",
                                list,
                                ipv4,
                                e
                            ));
                        }
                    }
                }
            }
        }
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
