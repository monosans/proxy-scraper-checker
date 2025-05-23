use std::sync::LazyLock;

use color_eyre::eyre::WrapErr as _;

pub static PROXY_REGEX: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
    fancy_regex::RegexBuilder::new(
        r"(?:^|[^0-9A-Za-z])(?:(?P<protocol>https?|socks[45]):\/\/)?(?:(?P<username>[\dA-Za-z]+):(?P<password>[\dA-Za-z]+)@)?(?P<host>[A-Za-z][\-\.A-Za-z]*[A-Za-z]|[A-Za-z]|(?:[0-9]|[1-9][0-9]|1[0-9]{2}|2[0-4][0-9]|25[0-5])(?:\.(?:[0-9]|[1-9][0-9]|1[0-9]{2}|2[0-4][0-9]|25[0-5])){3}):(?P<port>[0-9]|[1-9][0-9]{1,3}|[1-5][0-9]{4}|6[0-4][0-9]{3}|65[0-4][0-9]{2}|655[0-2][0-9]|6553[0-5])(?=[^0-9A-Za-z]|$)",
    )
    .case_insensitive(true)
    .backtrack_limit(usize::MAX)
    .build()
    .unwrap()
});

static IPV4_REGEX: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
    fancy_regex::Regex::new(
        r"^\s*(?:[0-9:A-Fa-f]+,)?\s*(?P<host>(?:[0-9]|[1-9][0-9]|1[0-9]{2}|2[0-4][0-9]|25[0-5])(?:\.(?:[0-9]|[1-9][0-9]|1[0-9]{2}|2[0-4][0-9]|25[0-5])){3})(?::(?:[0-9]|[1-9][0-9]{1,3}|[1-5][0-9]{4}|6[0-4][0-9]{3}|65[0-4][0-9]{2}|655[0-2][0-9]|6553[0-5]))?\s*$",
    )
    .unwrap()
});

pub fn parse_ipv4(s: &str) -> Option<String> {
    if let Ok(Some(captures)) =
        IPV4_REGEX.captures(s).wrap_err("failed to match regex capture groups")
    {
        captures.name("host").map(|capture| capture.as_str().to_owned())
    } else {
        None
    }
}
