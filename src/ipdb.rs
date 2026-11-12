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
        // SAFETY: a mapped file must not change while the reader is alive. This
        // program never writes to a file once it's opened here: a download fills
        // the temp file before checking it, then renames it over the database,
        // which leaves the old file and any mapping of it as they were. Only
        // another process writing to the cache directory could break this.
        #[expect(unsafe_code)]
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

    use super::{DbType, Duration, open_reader, within};
    #[cfg(feature = "tui")]
    use crate::event::{AppEvent, Event};
    use crate::{
        config::tests::{temp_directory, test_config},
        http::{
            self,
            tests::{serve, serve_then_stall},
        },
    };

    const LIMIT: Duration = Duration::from_millis(200);

    // A test process has only the one cache, so the tests that use it take
    // turns.
    pub mod cache {
        use std::path::PathBuf;

        use tokio::sync::{Mutex, MutexGuard};

        use crate::ipdb::{DbType, get_cache_path};

        static TURN: Mutex<()> = Mutex::const_new(());

        const MAP_OF_ONE: u8 = 0xE1;

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
        // same record, so every IPv4 address gets it.
        pub fn database_of(record: &[u8]) -> Vec<u8> {
            const MAP_OF_NINE: u8 = 0xE9;
            const EMPTY_ARRAY: [u8; 2] = [0x00, 0x04];
            const UINT64_OF_ONE: [u8; 3] = [0x01, 0x02, 0x01];
            const FIRST_RECORD: [u8; 3] = [0x00, 0x00, 0x11];

            [
                FIRST_RECORD.as_slice(),
                &FIRST_RECORD,
                &[0; 16],
                record,
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

        // Every IPv4 address belongs to this autonomous system. Only the tests
        // of main.rs, which need the TUI, use it.
        #[cfg(feature = "tui")]
        pub fn database(autonomous_system: u8) -> Vec<u8> {
            database_of(
                &[
                    [MAP_OF_ONE].as_slice(),
                    &text("autonomous_system_number"),
                    &uint32(autonomous_system),
                ]
                .concat(),
            )
        }

        // Every IPv4 address is in Munich, which is named in two languages,
        // and so is its state.
        pub fn city_database() -> Vec<u8> {
            const MAP_OF_TWO: u8 = 0xE2;
            const ARRAY_OF_ONE: [u8; 2] = [0x01, 0x04];

            let names = |german: &str, english: &str| {
                [
                    [MAP_OF_ONE].as_slice(),
                    &text("names"),
                    &[MAP_OF_TWO],
                    &text("de"),
                    &text(german),
                    &text("en"),
                    &text(english),
                ]
                .concat()
            };
            database_of(
                &[
                    [MAP_OF_TWO].as_slice(),
                    &text("city"),
                    &names("Muenchen", "Munich"),
                    &text("subdivisions"),
                    &ARRAY_OF_ONE,
                    &names("Bayern", "Bavaria"),
                ]
                .concat(),
            )
        }

        // A record that no ASN can be read from.
        pub fn damaged_database() -> Vec<u8> {
            database_of(
                &[[MAP_OF_ONE].as_slice(), &text("autonomous_system_number"), &text("seven")]
                    .concat(),
            )
        }

        // Empty, and this test's alone until dropped.
        pub struct Cache {
            path: PathBuf,
            _turn: MutexGuard<'static, ()>,
        }

        impl Drop for Cache {
            fn drop(&mut self) {
                drop(std::fs::remove_dir_all(self.path.parent().unwrap()));
            }
        }

        pub async fn take() -> Cache {
            let turn = TURN.lock().await;
            let path = get_cache_path().await.unwrap();
            std::fs::create_dir_all(&path).unwrap();
            Cache { path, _turn: turn }
        }

        pub async fn database_path(db_type: DbType) -> PathBuf {
            db_type.db_path().await.unwrap()
        }
    }

    async fn client() -> reqwest_middleware::ClientWithMiddleware {
        let config = test_config(|_| {}).await;
        http::create_reqwest_client(&config).await.unwrap()
    }

    // A response whose body is `body`, as a download gets it.
    async fn response_with(body: &[u8]) -> reqwest::Response {
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let (address, _) = serve(vec![[head.as_bytes(), body].concat()]).await;
        client().await.get(format!("http://{address}/")).send().await.unwrap()
    }

    async fn save_asn(body: &[u8]) -> crate::Result<()> {
        let response = response_with(body).await;
        DbType::Asn
            .save_db(
                response,
                #[cfg(feature = "tui")]
                tokio::sync::mpsc::unbounded_channel().0,
            )
            .await
    }

    #[tokio::test]
    async fn a_downloaded_database_takes_the_place_of_the_cached_one() {
        let _cache = cache::take().await;
        let path = cache::database_path(DbType::Asn).await;
        std::fs::write(&path, cache::damaged_database()).unwrap();

        save_asn(&cache::city_database()).await.unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), cache::city_database());
        assert!(!DbType::Asn.tmp_path().await.unwrap().exists());
        drop(DbType::Asn.open_mmap().await.unwrap());
    }

    #[tokio::test]
    async fn a_body_that_is_no_database_leaves_the_cached_one() {
        let _cache = cache::take().await;
        let path = cache::database_path(DbType::Asn).await;
        std::fs::write(&path, cache::city_database()).unwrap();

        let error = save_asn(b"<html>rate limited</html>").await.unwrap_err();

        let tmp_path = DbType::Asn.tmp_path().await.unwrap();
        assert_eq!(
            error.to_string(),
            format!("failed to open IP database: {}", tmp_path.display())
        );
        assert_eq!(std::fs::read(&path).unwrap(), cache::city_database());
    }

    #[tokio::test]
    async fn a_database_that_cannot_be_put_in_place_is_reported() {
        let _cache = cache::take().await;
        let path = cache::database_path(DbType::Asn).await;
        std::fs::create_dir_all(path.join("in the way")).unwrap();

        let error = save_asn(&cache::city_database()).await.unwrap_err();

        let tmp_path = DbType::Asn.tmp_path().await.unwrap();
        assert_eq!(
            error.to_string(),
            format!("failed to rename {} to {}", tmp_path.display(), path.display())
        );
    }

    #[cfg(feature = "tui")]
    #[tokio::test]
    async fn a_download_reports_its_size_and_every_chunk() {
        let _cache = cache::take().await;
        let body = cache::city_database();
        let response = response_with(&body).await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        DbType::Geo.save_db(response, tx).await.unwrap();

        let events: Vec<_> = std::iter::from_fn(move || rx.try_recv().ok()).collect();
        let total = u64::try_from(body.len()).unwrap();
        assert!(matches!(
            events.first(),
            Some(Event::App(AppEvent::IpDbTotal(DbType::Geo, Some(size)))) if *size == total
        ));
        let chunks: Vec<_> = events
            .iter()
            .skip(1)
            .filter_map(|event| match *event {
                Event::App(AppEvent::IpDbDownloaded(DbType::Geo, bytes)) => Some(bytes),
                Event::Tick | Event::Crossterm(_) | Event::App(_) => None,
            })
            .collect();
        assert_eq!(chunks.len(), events.len().checked_sub(1).unwrap());
        assert_eq!(chunks.iter().sum::<usize>(), body.len());
    }

    #[tokio::test]
    async fn an_etag_is_kept_until_it_is_removed() {
        let _cache = cache::take().await;
        assert_eq!(DbType::Geo.read_etag().await.unwrap(), None);

        DbType::Geo.save_etag("\"v1\"").await.unwrap();
        assert_eq!(DbType::Geo.read_etag().await.unwrap().unwrap(), "\"v1\"");
        assert_eq!(DbType::Asn.read_etag().await.unwrap(), None);

        DbType::Geo.remove_etag().await.unwrap();
        assert_eq!(DbType::Geo.read_etag().await.unwrap(), None);
        DbType::Geo.remove_etag().await.unwrap();
    }

    #[tokio::test]
    async fn an_etag_that_cannot_be_sent_back_is_ignored() {
        let _cache = cache::take().await;
        DbType::Asn.save_etag(b"\"v1\"\n").await.unwrap();
        assert_eq!(DbType::Asn.read_etag().await.unwrap(), None);
    }

    #[tokio::test]
    async fn an_etag_file_taken_by_a_directory_is_an_error() {
        let _cache = cache::take().await;
        let path = DbType::Asn.etag_path().await.unwrap();
        std::fs::create_dir_all(&path).unwrap();

        let error = DbType::Asn.read_etag().await.unwrap_err();
        assert_eq!(error.to_string(), format!("failed to read file: {}", path.display()));

        let error = DbType::Asn.remove_etag().await.unwrap_err();
        assert_eq!(error.to_string(), format!("failed to remove file: {}", path.display()));

        let error = DbType::Asn.save_etag("\"v1\"").await.unwrap_err();
        assert_eq!(error.to_string(), format!("failed to write to file: {}", path.display()));
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
        let (address, _held) = serve_then_stall("").await;
        let request = client().await.get(format!("http://{address}/")).send();
        let error = within(LIMIT, request).await.unwrap_err();
        assert!(error.to_string().starts_with("received no data"));
    }

    #[tokio::test]
    async fn a_download_that_stops_midway_is_given_up_on() {
        let (address, _held) =
            serve_then_stall("HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nabc").await;
        let request = client().await.get(format!("http://{address}/")).send();
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
