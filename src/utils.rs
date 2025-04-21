pub(crate) async fn is_docker() -> bool {
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

pub(crate) fn is_http_url(value: &str) -> bool {
    if let Ok(parsed_url) = url::Url::parse(value) {
        let scheme = parsed_url.scheme();
        (scheme == "http" || scheme == "https")
            && parsed_url.host_str().is_some()
    } else {
        false
    }
}
