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
        Some(scheme_end) => (
            Some(core.get(..scheme_end)?),
            core.get(scheme_end.checked_add(3)?..)?,
        ),
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

    Some((
        ProxyMatch { protocol, username, password, host_cidr, host, port },
        trailing_len,
    ))
}

pub fn proxy_matches(text: &str) -> impl Iterator<Item = ProxyMatch<'_>> {
    let pattern = regex::regex!(
        r"(?:^|[^0-9A-Za-z])(?:(?i:https?|socks4a?|socks5h?):\/\/)?(?:[0-9A-Za-z]{1,64}:[0-9A-Za-z]{1,64}@)?(?:[A-Za-z][\-\.A-Za-z]{0,251}[A-Za-z]|[A-Za-z]|(?:[0-9]|[1-9][0-9]|1[0-9]{2}|2[0-4][0-9]|25[0-5])(?:\.(?:[0-9]|[1-9][0-9]|1[0-9]{2}|2[0-4][0-9]|25[0-5])){3})(?:\/(?:1[6-9]|2[0-9]|3[0-2]))?:(?:[0-9]|[1-9][0-9]{1,3}|[1-5][0-9]{4}|6[0-4][0-9]{3}|65[0-4][0-9]{2}|655[0-2][0-9]|6553[0-5])(?:[^0-9A-Za-z]|$)"
    );
    let mut start = 0;

    std::iter::from_fn(move || {
        let whole_match = pattern.find_at(text, start)?;
        let (proxy_match, trailing_len) = split(whole_match.as_str())?;

        start = whole_match.end().checked_sub(trailing_len)?;

        Some(proxy_match)
    })
}

pub fn parse_ipv4(mut s: &str) -> Option<std::net::Ipv4Addr> {
    s = s.trim();
    let host = if let Some((host, port)) = s.rsplit_once(':') {
        if port.parse::<u16>().is_ok() { host } else { s }
    } else {
        s
    };

    host.parse().ok()
}
