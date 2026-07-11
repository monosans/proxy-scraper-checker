pub fn proxy_captures(text: &str) -> impl Iterator<Item = regex::Captures<'_>> {
    let mut start = 0;
    std::iter::from_fn(move || {
        let pattern = regex::regex!(
            r"(?:^|[^0-9A-Za-z])(?:(?P<protocol>https?|socks[45]):\/\/)?(?:(?P<username>[0-9A-Za-z]{1,64}):(?P<password>[0-9A-Za-z]{1,64})@)?(?P<host_cidr>(?P<host>[A-Za-z][\-\.A-Za-z]{0,251}[A-Za-z]|[A-Za-z]|(?:[0-9]|[1-9][0-9]|1[0-9]{2}|2[0-4][0-9]|25[0-5])(?:\.(?:[0-9]|[1-9][0-9]|1[0-9]{2}|2[0-4][0-9]|25[0-5])){3})(?:\/1[6-9]|2[0-9]|3[0-2])?):(?P<port>[0-9]|[1-9][0-9]{1,3}|[1-5][0-9]{4}|6[0-4][0-9]{3}|65[0-4][0-9]{2}|655[0-2][0-9]|6553[0-5])(?:(?P<trailing>[^0-9A-Za-z])|$)"
        );
        let captures = pattern.captures_at(text, start)?;
        let whole_match = captures.get(0)?;
        start = captures
            .name("trailing")
            .map_or_else(move || whole_match.end(), |m| m.start());
        Some(captures)
    })
}

pub fn parse_ipv4(mut s: &str) -> Option<compact_str::CompactString> {
    s = s.trim();
    let host = if let Some((host, port)) = s.rsplit_once(':') {
        if port.parse::<u16>().is_ok() { host } else { s }
    } else {
        s
    };

    host.parse::<std::net::Ipv4Addr>().is_ok().then(move || host.into())
}
