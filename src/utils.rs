use std::fmt::Write as _;

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

pub trait CompactStrJoin: Iterator {
    fn join(&mut self, sep: &str) -> compact_str::CompactString
    where
        Self::Item: std::fmt::Display,
    {
        self.next().map_or_else(
            || compact_str::CompactString::const_new(""),
            move |first_elt| {
                let (lower, _) = self.size_hint();
                let mut result = compact_str::CompactString::with_capacity(
                    sep.len().saturating_mul(lower),
                );
                write!(&mut result, "{first_elt}").unwrap();
                for elt in self {
                    write!(&mut result, "{sep}{elt}").unwrap();
                }
                result
            },
        )
    }
}

#[expect(clippy::missing_trait_methods)]
impl<T: Iterator> CompactStrJoin for T {}

pub fn pretty_error(e: &crate::Error) -> compact_str::CompactString {
    e.chain().join(" \u{2192} ")
}
