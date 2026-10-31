use std::fmt::Write as _;

pub fn pretty_error(e: &crate::Error) -> compact_str::CompactString {
    let mut result = compact_str::CompactString::const_new("");
    for (i, cause) in e.chain().enumerate() {
        if i != 0 {
            result.push_str(" \u{2192} ");
        }
        write!(&mut result, "{cause}").unwrap();
    }
    result
}

pub fn grouped(number: usize) -> compact_str::CompactString {
    let digits = itoa::Buffer::new().format(number).to_owned();
    let mut grouped = compact_str::CompactString::default();
    for (remaining, digit) in (1..=digits.len()).rev().zip(digits.chars()) {
        grouped.push(digit);
        if remaining != 1 && remaining.saturating_sub(1).is_multiple_of(3) {
            grouped.push(',');
        }
    }
    grouped
}

pub fn clock(seconds: u64) -> compact_str::CompactString {
    let minutes = seconds.div_euclid(60);
    let hours = minutes.div_euclid(60);
    if hours == 0 {
        compact_str::format_compact!("{minutes}:{:02}", seconds.rem_euclid(60))
    } else {
        compact_str::format_compact!(
            "{hours}:{:02}:{:02}",
            minutes.rem_euclid(60),
            seconds.rem_euclid(60)
        )
    }
}

pub async fn is_container() -> bool {
    #[cfg(target_os = "linux")]
    {
        static CACHE: tokio::sync::OnceCell<bool> =
            tokio::sync::OnceCell::const_new();

        *CACHE
            .get_or_init(async || {
                tokio::fs::try_exists("/.dockerenv").await.unwrap_or(false)
                    || tokio::fs::try_exists("/run/.containerenv")
                        .await
                        .unwrap_or(false)
            })
            .await
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{clock, grouped};

    #[test]
    fn numbers_read_in_groups_of_three() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1000), "1,000");
        assert_eq!(grouped(12_480), "12,480");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }

    #[test]
    fn durations_grow_an_hours_field_only_when_needed() {
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(134), "2:14");
        assert_eq!(clock(3_725), "1:02:05");
    }
}
