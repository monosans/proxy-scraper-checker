use std::{io, path::PathBuf};

use color_eyre::eyre::{WrapErr as _, eyre};
use tokio::io::AsyncWriteExt as _;

#[cfg(feature = "tui")]
use crate::event::{AppEvent, Event};
use crate::{fs::get_cache_path, utils::is_docker};

#[derive(Clone, Copy)]
pub enum DbType {
    Asn,
    Geo,
}

impl DbType {
    const fn name(self) -> &'static str {
        match self {
            Self::Asn => "ASN",
            Self::Geo => "geolocation",
        }
    }

    const fn url(self) -> &'static str {
        match self {
            Self::Asn => "https://raw.githubusercontent.com/P3TERX/GeoLite.mmdb/download/GeoLite2-ASN.mmdb",
            Self::Geo => "https://raw.githubusercontent.com/P3TERX/GeoLite.mmdb/download/GeoLite2-City.mmdb",
        }
    }

    async fn db_path(self) -> color_eyre::Result<PathBuf> {
        let mut cache_path =
            get_cache_path().await.wrap_err("failed to get cache path")?;
        match self {
            Self::Asn => cache_path.push("asn_database.mmdb"),
            Self::Geo => cache_path.push("geolocation_database.mmdb"),
        }
        Ok(cache_path)
    }

    async fn etag_path(self) -> color_eyre::Result<PathBuf> {
        let mut db_path = self.db_path().await.wrap_err_with(move || {
            format!("failed to get {} database path", self.name())
        })?;
        db_path.set_extension("mmdb.etag");
        Ok(db_path)
    }

    async fn save_db(
        self,
        mut response: reqwest::Response,
        #[cfg(feature = "tui")] tx: tokio::sync::mpsc::UnboundedSender<Event>,
    ) -> color_eyre::Result<()> {
        #[cfg(feature = "tui")]
        drop(tx.send(Event::App(AppEvent::IpDbTotal(
            self,
            response.content_length(),
        ))));

        let db_path = self.db_path().await?;
        let mut file =
            tokio::fs::File::create(&db_path).await.wrap_err_with(|| {
                format!("failed to create file {}", db_path.display())
            })?;
        while let Some(chunk) =
            response.chunk().await.wrap_err_with(move || {
                format!(
                    "failed to read {} database response chunk",
                    self.name()
                )
            })?
        {
            file.write_all(&chunk).await.wrap_err_with(|| {
                format!("failed to write to file {}", db_path.display())
            })?;
            #[cfg(feature = "tui")]
            drop(
                tx.send(Event::App(AppEvent::IpDbDownloaded(
                    self,
                    chunk.len(),
                ))),
            );
        }
        Ok(())
    }

    async fn save_etag(self, etag: impl AsRef<[u8]>) -> color_eyre::Result<()> {
        let path = self.etag_path().await?;
        tokio::fs::write(&path, etag).await.wrap_err_with(move || {
            format!("failed to write to file {}", path.display())
        })
    }

    async fn read_etag(
        self,
    ) -> color_eyre::Result<Option<reqwest::header::HeaderValue>> {
        let path = self.etag_path().await?;
        match tokio::fs::read_to_string(&path).await {
            Ok(text) => Ok(text.parse().ok()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).wrap_err_with(move || {
                format!("failed to read file {} to string", path.display())
            }),
        }
    }

    async fn remove_etag(self) -> color_eyre::Result<()> {
        let path = self.etag_path().await?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).wrap_err_with(move || {
                format!("failed to remove {}", path.display())
            }),
        }
    }

    pub async fn download(
        self,
        http_client: reqwest::Client,
        #[cfg(feature = "tui")] tx: tokio::sync::mpsc::UnboundedSender<Event>,
    ) -> color_eyre::Result<()> {
        let db_path = self.db_path().await?;
        let mut headers = reqwest::header::HeaderMap::new();
        #[expect(clippy::collapsible_if)]
        if tokio::fs::metadata(&db_path).await.is_ok_and(|m| m.is_file()) {
            if let Some(etag) =
                self.read_etag().await.wrap_err("failed to read ETag")?
            {
                headers.insert(reqwest::header::IF_NONE_MATCH, etag);
            }
        }

        let response = http_client
            .get(self.url())
            .headers(headers)
            .send()
            .await
            .wrap_err_with(move || {
                format!(
                    "failed to send {} database download request",
                    self.name()
                )
            })?
            .error_for_status()
            .wrap_err_with(move || {
                format!(
                    "got error HTTP status code when downloading {} database",
                    self.name()
                )
            })?;

        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            tracing::info!(
                "Latest {} database is already cached at {}",
                self.name(),
                db_path.display()
            );
            return Ok(());
        }

        if response.status() != reqwest::StatusCode::OK {
            return Err(eyre!(
                "HTTP status error ({}) for url ({})",
                response.status(),
                response.url()
            ));
        }

        let etag = response.headers().get(reqwest::header::ETAG).cloned();

        self.save_db(
            response,
            #[cfg(feature = "tui")]
            tx.clone(),
        )
        .await
        .wrap_err_with(move || {
            format!("failed to save {} database", self.name())
        })?;

        if is_docker().await {
            tracing::info!(
                "Downloaded {} database to Docker volume ({} in container)",
                self.name(),
                db_path.display()
            );
        } else {
            tracing::info!(
                "Downloaded {} database to {}",
                self.name(),
                db_path.display()
            );
        }

        if let Some(etag_value) = etag {
            self.save_etag(etag_value).await.wrap_err_with(move || {
                format!("failed to save {} database ETag", self.name())
            })
        } else {
            self.remove_etag().await.wrap_err_with(move || {
                format!("failed to remove {} database ETag", self.name())
            })
        }
    }

    pub async fn open_mmap(
        self,
    ) -> color_eyre::Result<maxminddb::Reader<maxminddb::Mmap>> {
        let path = self.db_path().await?;
        tokio::task::spawn_blocking(move || maxminddb::Reader::open_mmap(path))
            .await
            .wrap_err("failed to spawn tokio blocking task")?
            .wrap_err_with(move || {
                format!("failed to open {} database", self.name())
            })
    }
}
