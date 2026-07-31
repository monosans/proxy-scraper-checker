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
