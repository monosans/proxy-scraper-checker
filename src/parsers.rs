use std::sync::LazyLock;

use color_eyre::eyre::{OptionExt as _, WrapErr as _};

pub static PROXY_REGEX: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
    fancy_regex::Regex::new(
        r"(?:^|[^\dA-Za-z])(?:(?P<protocol>https?|socks[45]):\/\/)?(?:(?P<username>[^\s:@]+):(?P<password>[^\s:@]+)@)?(?P<host>(?:[\-\.\dA-Za-z]+|(?:\d|[1-9]\d|1\d{2}|2[0-4]\d|25[0-5])(?:\.(?:\d|[1-9]\d|1\d{2}|2[0-4]\d|25[0-5])){3})):(?P<port>\d|[1-9]\d{1,3}|[1-5]\d{4}|6[0-4]\d{3}|65[0-4]\d{2}|655[0-2]\d|6553[0-5])(?=[^\dA-Za-z]|$)"
    ).unwrap()
});

static IPV4_REGEX: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
    fancy_regex::Regex::new(
        r"^(?:[0-9:A-Fa-f]+,)?\s*(?P<host>(?:\d|[1-9]\d|1\d{2}|2[0-4]\d|25[0-5])(?:\.(?:\d|[1-9]\d|1\d{2}|2[0-4]\d|25[0-5])){3})(?::(?:\d|[1-9]\d{1,3}|[1-5]\d{4}|6[0-4]\d{3}|65[0-4]\d{2}|655[0-2]\d|6553[0-5]))?\s*$"
    ).unwrap()
});

pub fn parse_ipv4(s: &str) -> color_eyre::Result<Option<String>> {
    match IPV4_REGEX
        .captures(s)
        .wrap_err("failed to match regex capture groups")?
    {
        Some(captures) => Ok(Some(
            captures
                .name("host")
                .ok_or_eyre("failed to match \"host\" regex capture group")?
                .as_str()
                .to_owned(),
        )),
        None => Ok(None),
    }
}
