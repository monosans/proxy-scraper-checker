use std::path::PathBuf;

use color_eyre::eyre::{OptionExt as _, WrapErr as _};

use crate::config::APP_DIRECTORY_NAME;

static CACHE: tokio::sync::OnceCell<PathBuf> =
    tokio::sync::OnceCell::const_new();

pub async fn get_cache_path() -> crate::Result<PathBuf> {
    Ok(CACHE
        .get_or_try_init(async || -> crate::Result<PathBuf> {
            let mut path = tokio::task::spawn_blocking(dirs::cache_dir)
                .await?
                .ok_or_eyre("failed to get user's cache directory")?;
            path.push(APP_DIRECTORY_NAME);
            tokio::fs::create_dir_all(&path).await.wrap_err_with(|| {
                compact_str::format_compact!(
                    "failed to create directory: {}",
                    path.display()
                )
            })?;
            Ok(path)
        })
        .await?
        .clone())
}
