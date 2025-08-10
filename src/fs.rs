use std::path::PathBuf;

use color_eyre::eyre::{OptionExt as _, WrapErr as _};

use crate::config::APP_DIRECTORY_NAME;

pub async fn get_cache_path() -> crate::Result<PathBuf> {
    static CACHE: tokio::sync::OnceCell<PathBuf> =
        tokio::sync::OnceCell::const_new();
    Ok(CACHE
        .get_or_try_init(async || -> crate::Result<PathBuf> {
            let mut path = tokio::task::spawn_blocking(dirs::cache_dir)
                .await
                .wrap_err("failed to spawn task to get user's cache directory")?
                .ok_or_eyre("failed to get user's cache directory")?;
            path.push(APP_DIRECTORY_NAME);
            tokio::fs::create_dir_all(&path).await.wrap_err_with(|| {
                format!("failed to create directory: {}", path.display())
            })?;
            Ok(path)
        })
        .await?
        .clone())
}
