use std::{io, path::PathBuf, time::Duration};

use color_eyre::eyre::{OptionExt as _, WrapErr as _, eyre};
use tokio::io::AsyncWriteExt as _;

#[cfg(feature = "tui")]
use crate::event::{AppEvent, Event};
use crate::utils::{is_container, pretty_error};

const CACHE_DIRECTORY_NAME: &str = "proxy_scraper_checker";
const STALL_TIMEOUT: Duration = Duration::from_secs(60);

async fn get_cache_path() -> crate::Result<PathBuf> {
    static CACHE: tokio::sync::OnceCell<PathBuf> = tokio::sync::OnceCell::const_new();

    Ok(CACHE
        .get_or_try_init(async || -> crate::Result<PathBuf> {
            let parent = dirs::cache_dir();
            // The real databases must not decide what a test sees.
            #[cfg(test)]
            let parent = parent.map(|_| crate::config::tests::temp_directory("ip-databases"));

            let path = parent
                .ok_or_eyre("failed to get user's cache directory")?
                .join(CACHE_DIRECTORY_NAME);
            tokio::fs::create_dir_all(&path).await.wrap_err_with(|| {
                compact_str::format_compact!("failed to create directory: {}", path.display())
            })?;
            Ok(path)
        })
        .await?
        .clone())
}

// A database download has no total time limit, however slow the connection,
// and the checks wait for it. Only a connection that delivers nothing at all
// for this long is given up on, so a stall can't hang the run.
async fn within<T, E>(
    limit: Duration,
    future: impl Future<Output = Result<T, E>>,
) -> crate::Result<T>
where
    crate::Error: From<E>,
{
    let result = tokio::time::timeout(limit, future).await.wrap_err_with(move || {
        compact_str::format_compact!("received no data for {} seconds", limit.as_secs())
    })?;
    Ok(result?)
}

async fn open_reader(path: PathBuf) -> crate::Result<maxminddb::Reader<maxminddb::Mmap>> {
    tokio::task::spawn_blocking(move || {
        #[expect(unsafe_code, clippy::undocumented_unsafe_blocks)]
        unsafe { maxminddb::Reader::open_mmap(&path) }.wrap_err_with(move || {
            compact_str::format_compact!("failed to open IP database: {}", path.display())
        })
    })
    .await?
}

#[derive(Clone, Copy)]
pub enum DbType {
    Asn,
    Geo,
}

impl DbType {
    pub async fn download(
        self,
        http_client: reqwest_middleware::ClientWithMiddleware,
        #[cfg(feature = "tui")] tx: tokio::sync::mpsc::UnboundedSender<Event>,
    ) -> crate::Result<()> {
        // Left by a download that was cut short. Nothing else would remove it
        // if the database turns out to be up to date.
        drop(tokio::fs::remove_file(self.tmp_path().await?).await);

        let mut headers = reqwest::header::HeaderMap::new();
        // The ETag only vouches for a file that still opens; otherwise a
        // damaged database would be reported as up to date forever.
        if self.open_mmap().await.is_ok()
            && let Some(etag) = self.read_etag().await?
        {
            headers.insert(reqwest::header::IF_NONE_MATCH, etag);
        }

        let request = http_client.get(self.url()).headers(headers).timeout(Duration::MAX);
        drop(http_client);

        let response = within(STALL_TIMEOUT, request.send()).await?.error_for_status()?;

        let status = response.status();
        if status == reqwest::StatusCode::NOT_MODIFIED {
            drop(response);
            #[cfg(feature = "tui")]
            drop(tx);
            let db_path = self.db_path().await?;
            tracing::info!(
                "Latest {} database is already cached at {}",
                self.name(),
                db_path.display()
            );
            return Ok(());
        }

        if status != reqwest::StatusCode::OK {
            return Err(eyre!("HTTP status error ({}) for url ({})", status, response.url(),));
        }

        let etag = response.headers().get(reqwest::header::ETAG).cloned();

        self.save_db(
            response,
            #[cfg(feature = "tui")]
            tx,
        )
        .await?;

        let in_container = is_container().await;
        let db_path = self.db_path().await?;
        if in_container {
            tracing::info!(
                "Downloaded {} database to Docker volume ({} in container)",
                self.name(),
                db_path.display()
            );
        } else {
            tracing::info!("Downloaded {} database to {}", self.name(), db_path.display());
        }
        drop(db_path);

        let etag_saved = if let Some(etag_value) = etag {
            self.save_etag(etag_value).await
        } else {
            self.remove_etag().await
        };
        // The database itself is in place; this only costs a repeat download.
        if let Err(e) = etag_saved {
            tracing::warn!(
                "{} database will be downloaded again next run: {}",
                self.name(),
                pretty_error(&e)
            );
        }
        Ok(())
    }

    pub async fn open_mmap(self) -> crate::Result<maxminddb::Reader<maxminddb::Mmap>> {
        open_reader(self.db_path().await?).await
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Asn => "ASN",
            Self::Geo => "geolocation",
        }
    }

    const fn url(self) -> &'static str {
        match self {
            Self::Asn => {
                "https://raw.githubusercontent.com/P3TERX/GeoLite.mmdb/refs/heads/download/GeoLite2-ASN.mmdb"
            }
            Self::Geo => {
                "https://raw.githubusercontent.com/P3TERX/GeoLite.mmdb/refs/heads/download/GeoLite2-City.mmdb"
            }
        }
    }

    async fn db_path(self) -> crate::Result<PathBuf> {
        let mut cache_path = get_cache_path().await?;
        cache_path.push(match self {
            Self::Asn => "asn_database.mmdb",
            Self::Geo => "geolocation_database.mmdb",
        });
        Ok(cache_path)
    }

    async fn tmp_path(self) -> crate::Result<PathBuf> {
        let mut db_path = self.db_path().await?;
        db_path.set_extension("mmdb.tmp");
        Ok(db_path)
    }

    async fn etag_path(self) -> crate::Result<PathBuf> {
        let mut db_path = self.db_path().await?;
        db_path.set_extension("mmdb.etag");
        Ok(db_path)
    }

    async fn save_db(
        self,
        mut response: reqwest::Response,
        #[cfg(feature = "tui")] tx: tokio::sync::mpsc::UnboundedSender<Event>,
    ) -> crate::Result<()> {
        #[cfg(feature = "tui")]
        drop(tx.send(Event::App(AppEvent::IpDbTotal(self, response.content_length()))));

        let tmp_path = self.tmp_path().await?;
        let file = tokio::fs::File::create(&tmp_path).await.wrap_err_with(|| {
            compact_str::format_compact!("failed to create file: {}", tmp_path.display())
        })?;
        let mut writer = tokio::io::BufWriter::new(file);

        let failed_to_write =
            || compact_str::format_compact!("failed to write to file: {}", tmp_path.display());

        while let Some(chunk) = within(STALL_TIMEOUT, response.chunk()).await? {
            writer.write_all(&chunk).await.wrap_err_with(failed_to_write)?;
            #[cfg(feature = "tui")]
            drop(tx.send(Event::App(AppEvent::IpDbDownloaded(self, chunk.len()))));
        }
        drop(response);
        #[cfg(feature = "tui")]
        drop(tx);

        writer.flush().await.wrap_err_with(failed_to_write)?;
        drop(writer);

        // A body that isn't a database must not replace one that works.
        open_reader(tmp_path.clone()).await?;

        let db_path = self.db_path().await?;
        tokio::fs::rename(&tmp_path, &db_path).await.wrap_err_with(move || {
            compact_str::format_compact!(
                "failed to rename {} to {}",
                tmp_path.display(),
                db_path.display()
            )
        })
    }

    async fn save_etag(self, etag: impl AsRef<[u8]>) -> crate::Result<()> {
        let path = self.etag_path().await?;
        tokio::fs::write(&path, etag).await.wrap_err_with(move || {
            compact_str::format_compact!("failed to write to file: {}", path.display())
        })
    }

    async fn read_etag(self) -> crate::Result<Option<reqwest::header::HeaderValue>> {
        let path = self.etag_path().await?;
        match tokio::fs::read(&path).await {
            // Unreadable contents only mean downloading the database again.
            Ok(bytes) => Ok(reqwest::header::HeaderValue::from_bytes(&bytes).ok()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).wrap_err_with(move || {
                compact_str::format_compact!("failed to read file: {}", path.display())
            }),
        }
    }

    async fn remove_etag(self) -> crate::Result<()> {
        let path = self.etag_path().await?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).wrap_err_with(move || {
                compact_str::format_compact!("failed to remove file: {}", path.display())
            }),
        }
    }
}

#[cfg(test)]
pub mod tests {
    use std::{future, io};

    use super::{Duration, open_reader, within};
    use crate::{
        config::tests::{temp_directory, test_config},
        http::{self, tests::serve_then_stall},
    };

    const LIMIT: Duration = Duration::from_millis(200);

    // For the test of a whole run, which only exists with the TUI's events.
    #[cfg(feature = "tui")]
    pub mod cache {
        use std::path::PathBuf;

        use crate::ipdb::{DbType, get_cache_path};

        fn text(value: &str) -> Vec<u8> {
            let mut encoded = vec![0x40 | u8::try_from(value.len()).unwrap()];
            encoded.extend_from_slice(value.as_bytes());
            encoded
        }

        const fn uint16(value: u8) -> [u8; 2] {
            [0xA1, value]
        }

        const fn uint32(value: u8) -> [u8; 2] {
            [0xC1, value]
        }

        // The smallest valid database: one node whose both sides lead to the
        // same record, so every IPv4 address belongs to this autonomous system.
        pub fn database(autonomous_system: u8) -> Vec<u8> {
            const MAP_OF_ONE: u8 = 0xE1;
            const MAP_OF_NINE: u8 = 0xE9;
            const EMPTY_ARRAY: [u8; 2] = [0x00, 0x04];
            const UINT64_OF_ONE: [u8; 3] = [0x01, 0x02, 0x01];
            const FIRST_RECORD: [u8; 3] = [0x00, 0x00, 0x11];

            [
                FIRST_RECORD.as_slice(),
                &FIRST_RECORD,
                &[0; 16],
                &[MAP_OF_ONE],
                &text("autonomous_system_number"),
                &uint32(autonomous_system),
                b"\xab\xcd\xefMaxMind.com",
                &[MAP_OF_NINE],
                &text("binary_format_major_version"),
                &uint16(2),
                &text("binary_format_minor_version"),
                &uint16(0),
                &text("build_epoch"),
                &UINT64_OF_ONE,
                &text("database_type"),
                &text("Test"),
                &text("description"),
                &[MAP_OF_ONE],
                &text("en"),
                &text("Test"),
                &text("ip_version"),
                &uint16(4),
                &text("languages"),
                &EMPTY_ARRAY,
                &text("node_count"),
                &uint32(1),
                &text("record_size"),
                &uint16(24),
            ]
            .concat()
        }

        pub async fn database_path(db_type: DbType) -> PathBuf {
            db_type.db_path().await.unwrap()
        }

        pub async fn remove() {
            let cache = get_cache_path().await.unwrap();
            std::fs::remove_dir_all(cache.parent().unwrap()).unwrap();
        }
    }

    async fn stalling_server(reply: &'static str) -> String {
        format!("http://{}/", serve_then_stall(reply).await)
    }

    async fn client() -> reqwest_middleware::ClientWithMiddleware {
        let config = test_config(|_| {}).await;
        http::create_reqwest_client(&config).await.unwrap()
    }

    #[tokio::test]
    async fn a_file_that_is_no_database_does_not_open() {
        let directory = temp_directory("garbage");
        let path = directory.join("database.mmdb");
        std::fs::write(&path, "<html>not a database</html>").unwrap();

        drop(open_reader(path).await.unwrap_err());

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn a_server_that_never_answers_is_given_up_on() {
        let url = stalling_server("").await;
        let request = client().await.get(url).send();
        let error = within(LIMIT, request).await.unwrap_err();
        assert!(error.to_string().starts_with("received no data"));
    }

    #[tokio::test]
    async fn a_download_that_stops_midway_is_given_up_on() {
        let url = stalling_server("HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nabc").await;
        let request = client().await.get(url).send();
        let mut response = within(LIMIT, request).await.unwrap();

        let chunk = within(LIMIT, response.chunk()).await.unwrap().unwrap();
        assert_eq!(chunk.as_ref(), b"abc");
        drop(chunk);

        let error = within(LIMIT, response.chunk()).await.unwrap_err();
        assert!(error.to_string().starts_with("received no data"));
    }

    #[tokio::test]
    async fn a_result_inside_the_limit_is_passed_on() {
        let ready = future::ready(Ok::<_, io::Error>(1_u8));
        assert_eq!(within(Duration::from_secs(1), ready).await.unwrap(), 1_u8);

        let failed = future::ready(Err::<(), _>(io::Error::other("failed")));
        let error = within(Duration::from_secs(1), failed).await.unwrap_err();
        assert_eq!(error.to_string(), "failed");
    }

    #[tokio::test]
    async fn silence_past_the_limit_is_an_error() {
        let stalled = future::pending::<Result<(), io::Error>>();
        let error = within(Duration::from_millis(1), stalled).await.unwrap_err();
        assert_eq!(error.to_string(), "received no data for 0 seconds");
    }
}
