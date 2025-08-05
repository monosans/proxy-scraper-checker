pub async fn is_docker() -> bool {
    #[cfg(target_os = "linux")]
    {
        static CACHE: tokio::sync::OnceCell<bool> =
            tokio::sync::OnceCell::const_new();

        *CACHE
            .get_or_init(async || {
                tokio::fs::try_exists("/.dockerenv").await.unwrap_or(false)
            })
            .await
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

pub fn pretty_error(e: &color_eyre::Report) -> String {
    e.chain().map(ToString::to_string).collect::<Vec<_>>().join(" \u{2192} ")
}
