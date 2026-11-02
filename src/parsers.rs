pub struct ProxyMatch<'h> {
    pub protocol: Option<&'h str>,
    pub username: Option<&'h str>,
    pub password: Option<&'h str>,
    pub host_cidr: &'h str,
    pub host: &'h str,
    pub port: &'h str,
}

const fn not_alphanumeric(c: char) -> bool {
    !c.is_ascii_alphanumeric()
}

fn split(span: &str) -> Option<(ProxyMatch<'_>, usize)> {
    let after_leading = span.trim_start_matches(not_alphanumeric);
    let core = after_leading.trim_end_matches(not_alphanumeric);
    let trailing_len = after_leading.len().checked_sub(core.len())?;

    let (protocol, rest) = match core.find("://") {
        Some(scheme_end) => {
            (Some(core.get(..scheme_end)?), core.get(scheme_end.checked_add(3)?..)?)
        }
        None => (None, core),
    };

    let (username, password, rest) = match rest.find('@') {
        Some(at) => {
            let userinfo = rest.get(..at)?;
            let colon = userinfo.find(':')?;
            (
                Some(userinfo.get(..colon)?),
                Some(userinfo.get(colon.checked_add(1)?..)?),
                rest.get(at.checked_add(1)?..)?,
            )
        }
        None => (None, None, rest),
    };

    let colon = rest.rfind(':')?;
    let host_cidr = rest.get(..colon)?;
    let port = rest.get(colon.checked_add(1)?..)?;
    let host = match host_cidr.find('/') {
        Some(slash) => host_cidr.get(..slash)?,
        None => host_cidr,
    };

    Some((ProxyMatch { protocol, username, password, host_cidr, host, port }, trailing_len))
}

pub fn proxy_matches(text: &str) -> impl Iterator<Item = ProxyMatch<'_>> {
    let pattern = regex::regex!(
        r"(?:^|[^0-9A-Za-z])(?:(?i-u:https?|socks4a?|socks5h?):\/\/)?(?:[0-9A-Za-z]{1,64}:[0-9A-Za-z]{1,64}@)?(?:(?:[0-9A-Za-z](?:[\-0-9A-Za-z]{0,61}[0-9A-Za-z])?\.){1,15}[A-Za-z](?:[\-0-9A-Za-z]{0,61}[0-9A-Za-z])?|[A-Za-z](?:[\-A-Za-z]{0,61}[A-Za-z])?|(?:[0-9]|[1-9][0-9]|1[0-9]{2}|2[0-4][0-9]|25[0-5])(?:\.(?:[0-9]|[1-9][0-9]|1[0-9]{2}|2[0-4][0-9]|25[0-5])){3})(?:\/(?:1[6-9]|2[0-9]|3[0-2]))?:(?:[0-9]|[1-9][0-9]{1,3}|[1-5][0-9]{4}|6[0-4][0-9]{3}|65[0-4][0-9]{2}|655[0-2][0-9]|6553[0-5])(?:[^0-9A-Za-z]|$)"
    );
    let mut start = 0;

    std::iter::from_fn(move || {
        loop {
            let whole_match = pattern.find_at(text, start)?;
            let end = whole_match.end();

            if let Some((proxy_match, trailing_len)) = split(whole_match.as_str()) {
                start = end.saturating_sub(trailing_len);
                return Some(proxy_match);
            }

            start = end;
        }
    })
}

fn parse_ipv4(mut s: &str) -> Option<std::net::Ipv4Addr> {
    s = s.trim();
    let host = if let Some((host, port)) = s.rsplit_once(':') {
        if port.parse::<u16>().is_ok() { host } else { s }
    } else {
        s
    };

    host.parse().ok()
}

#[derive(serde::Deserialize)]
struct HttpbinResponse {
    origin: compact_str::CompactString,
}

pub fn parse_exit_ip(body: &str) -> Option<std::net::Ipv4Addr> {
    let body = body.strip_prefix('\u{feff}').unwrap_or(body);
    match serde_json::from_str::<HttpbinResponse>(body) {
        // A proxy that reveals its client turns this into "client, exit".
        Ok(httpbin) => httpbin.origin.rsplit(',').next().and_then(parse_ipv4),
        Err(_) => parse_ipv4(body),
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use compact_str::{CompactString, format_compact};

    use super::{parse_exit_ip, parse_ipv4, proxy_matches};

    fn endpoints(text: &str) -> Vec<CompactString> {
        proxy_matches(text).map(|m| format_compact!("{}:{}", m.host_cidr, m.port)).collect()
    }

    fn matches_nothing(text: &str) -> bool {
        proxy_matches(text).next().is_none()
    }

    #[test]
    fn plain_host_port() {
        assert_eq!(endpoints("1.2.3.4:8080"), ["1.2.3.4:8080"]);
    }

    #[test]
    fn surrounding_text_is_ignored() {
        assert_eq!(endpoints("proxy = 1.2.3.4:8080 (elite)"), ["1.2.3.4:8080"]);
    }

    #[test]
    fn every_proxy_on_a_line_is_found() {
        assert_eq!(
            endpoints("1.2.3.4:8080,5.6.7.8:1080,9.10.11.12:3128"),
            ["1.2.3.4:8080", "5.6.7.8:1080", "9.10.11.12:3128"]
        );
    }

    #[test]
    fn adjacent_proxies_share_one_separator() {
        assert_eq!(endpoints("1.2.3.4:8080 5.6.7.8:1080"), ["1.2.3.4:8080", "5.6.7.8:1080"]);
    }

    #[test]
    fn html_and_json_need_no_cleanup() {
        assert_eq!(
            endpoints("<td>1.2.3.4</td><td>8080</td><td>1.2.3.4:8080</td>"),
            ["1.2.3.4:8080"]
        );
        assert_eq!(endpoints(r#"{"proxy": "socks5://1.2.3.4:1080"}"#), ["1.2.3.4:1080"]);
    }

    #[test]
    fn scheme_is_extracted() {
        let protocols: Vec<_> =
            proxy_matches("http://1.1.1.1:80 socks4a://2.2.2.2:80 SOCKS5H://3.3.3.3:80 4.4.4.4:80")
                .map(|m| m.protocol)
                .collect();
        assert_eq!(protocols, [Some("http"), Some("socks4a"), Some("SOCKS5H"), None]);
    }

    #[test]
    fn only_ascii_letters_spell_a_scheme() {
        for text in
            ["\u{17f}ocks5://1.2.3.4:80", "http\u{17f}://1.2.3.4:80", "soc\u{212a}s5://1.2.3.4:80"]
        {
            let matches: Vec<_> = proxy_matches(text).collect();
            assert_eq!(matches.len(), 1, "{text}");
            assert_eq!(matches.first().unwrap().protocol, None, "{text}");
        }
    }

    #[test]
    fn credentials_are_extracted() {
        let mut matches = proxy_matches("socks5://user:pass@1.2.3.4:1080");
        let m = matches.next().unwrap();
        assert_eq!(m.protocol, Some("socks5"));
        assert_eq!(m.username, Some("user"));
        assert_eq!(m.password, Some("pass"));
        assert_eq!(m.host, "1.2.3.4");
        assert_eq!(m.port, "1080");
        assert!(matches.next().is_none());
    }

    #[test]
    fn cidr_keeps_prefix_in_host_cidr_only() {
        let m = proxy_matches("1.2.3.0/24:8080").next().unwrap();
        assert_eq!(m.host_cidr, "1.2.3.0/24");
        assert_eq!(m.host, "1.2.3.0");
        assert_eq!(m.port, "8080");
    }

    #[test]
    fn hostnames_are_matched() {
        assert_eq!(endpoints("proxy.example.com:3128"), ["proxy.example.com:3128"]);
        assert_eq!(endpoints("eu1.proxy.example.com:3128"), ["eu1.proxy.example.com:3128"]);
        assert_eq!(
            endpoints("my-proxy.example.com:80 1proxy.example.com:80"),
            ["my-proxy.example.com:80", "1proxy.example.com:80"]
        );
        assert_eq!(endpoints("localhost:8080"), ["localhost:8080"]);
    }

    #[test]
    fn digits_in_a_bare_word_do_not_make_it_a_host() {
        assert!(matches_nothing("sha256:1234"));
        assert!(matches_nothing("user1:12345"));
        assert!(matches_nothing("2024-01-15T12:34:56Z"));
    }

    #[test]
    fn out_of_range_values_are_rejected() {
        assert!(matches_nothing("1.2.3.256:8080"));
        assert!(matches_nothing("1.2.3.4:65536"));
        assert!(matches_nothing("1.2.3.4"));
        assert!(matches_nothing("1.2.3.0/8:8080"));
    }

    #[test]
    fn timestamps_and_versions_are_not_proxies() {
        assert!(matches_nothing("updated 2024-01-15 12:30"));
        assert!(matches_nothing("bumped to 2.5.0:8080"));
    }

    #[test]
    fn parse_ipv4_accepts_plain_and_padded_addresses() {
        let expected = Some(Ipv4Addr::new(1, 2, 3, 4));
        assert_eq!(parse_ipv4("1.2.3.4"), expected);
        assert_eq!(parse_ipv4("  1.2.3.4\n"), expected);
        assert_eq!(parse_ipv4("1.2.3.4:8080"), expected);
    }

    #[test]
    fn parse_ipv4_rejects_non_addresses() {
        assert_eq!(parse_ipv4("<html>not an ip</html>"), None);
        assert_eq!(parse_ipv4(""), None);
        assert_eq!(parse_ipv4("2001:db8::1"), None);
        assert_eq!(parse_ipv4("1.2.3.4:notaport"), None);
    }

    #[test]
    fn exit_ip_comes_from_either_body_shape() {
        let expected = Some(Ipv4Addr::new(1, 2, 3, 4));
        assert_eq!(parse_exit_ip("1.2.3.4\n"), expected);
        assert_eq!(parse_exit_ip("\u{feff}1.2.3.4\n"), expected);
        assert_eq!(parse_exit_ip(r#"{"origin": "1.2.3.4"}"#), expected);
        assert_eq!(parse_exit_ip(r#"{"origin": "9.9.9.9, 1.2.3.4"}"#), expected);
        assert_eq!(parse_exit_ip(r#"{"ip": "1.2.3.4"}"#), None);
        assert_eq!(parse_exit_ip("<html>hello</html>"), None);
    }
}
