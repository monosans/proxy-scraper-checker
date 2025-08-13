use std::sync::LazyLock;

pub static PROXY_REGEX: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
    let pattern = r"(?:^|[^0-9A-Za-z])(?:(?P<protocol>https?|socks[45]):\/\/)?(?:(?P<username>[0-9A-Za-z]{1,64}):(?P<password>[0-9A-Za-z]{1,64})@)?(?P<host>[A-Za-z][\-\.A-Za-z]{0,251}[A-Za-z]|[A-Za-z]|(?:[0-9]|[1-9][0-9]|1[0-9]{2}|2[0-4][0-9]|25[0-5])(?:\.(?:[0-9]|[1-9][0-9]|1[0-9]{2}|2[0-4][0-9]|25[0-5])){3}):(?P<port>[0-9]|[1-9][0-9]{1,3}|[1-5][0-9]{4}|6[0-4][0-9]{3}|65[0-4][0-9]{2}|655[0-2][0-9]|6553[0-5])(?=[^0-9A-Za-z]|$)";
    fancy_regex::RegexBuilder::new(pattern)
        .backtrack_limit(usize::MAX)
        .build()
        .unwrap()
});

static IPV4_REGEX: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
    let pattern = r"^\s*(?P<host>(?:[0-9]|[1-9][0-9]|1[0-9]{2}|2[0-4][0-9]|25[0-5])(?:\.(?:[0-9]|[1-9][0-9]|1[0-9]{2}|2[0-4][0-9]|25[0-5])){3})(?::(?:[0-9]|[1-9][0-9]{1,3}|[1-5][0-9]{4}|6[0-4][0-9]{3}|65[0-4][0-9]{2}|655[0-2][0-9]|6553[0-5]))?\s*$";
    fancy_regex::Regex::new(pattern).unwrap()
});

pub fn parse_ipv4(s: &str) -> Option<String> {
    if let Ok(Some(captures)) = IPV4_REGEX.captures(s) {
        captures.name("host").map(|capture| capture.as_str().to_owned())
    } else {
        None
    }
}
