use std::{io, path::PathBuf, time::Duration};

use color_eyre::eyre::{OptionExt as _, WrapErr as _, eyre};
use tokio::io::AsyncWriteExt as _;

#[cfg(feature = "tui")]
use crate::event::{AppEvent, Event};
use crate::utils::{is_container, pretty_error, seen_from_outside};

const CACHE_DIRECTORY_NAME: &str = "proxy_scraper_checker";
const STALL_TIMEOUT: Duration = Duration::from_secs(60);

async fn create_cache_directory(parent: Option<PathBuf>) -> crate::Result<PathBuf> {
    let path =
        parent.ok_or_eyre("failed to get user's cache directory")?.join(CACHE_DIRECTORY_NAME);
    tokio::fs::create_dir_all(&path).await.wrap_err_with(|| {
        compact_str::format_compact!("failed to create directory: {}", path.display())
    })?;
    Ok(path)
}

async fn get_cache_path() -> crate::Result<PathBuf> {
    static CACHE: tokio::sync::OnceCell<PathBuf> = tokio::sync::OnceCell::const_new();

    Ok(CACHE
        .get_or_try_init(async || {
            let parent = dirs::cache_dir();
            // The real databases must not decide what a test sees.
            #[cfg(test)]
            let parent = parent.map(|_| crate::config::tests::temp_directory("ip-databases"));
            create_cache_directory(parent).await
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
        self.download_from(
            self.url(),
            http_client,
            #[cfg(feature = "tui")]
            tx,
        )
        .await
    }

    async fn download_from(
        self,
        url: &str,
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

        let request = http_client.get(url).headers(headers).timeout(Duration::MAX);
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

        let location =
            seen_from_outside("Docker volume", &self.db_path().await?, is_container().await);
        tracing::info!("Downloaded {} database to {location}", self.name());
        drop(location);

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

    use super::{DbType, Duration, create_cache_directory, open_reader, within};
    use crate::{
        config::tests::{temp_directory, test_config},
        http::{
            self,
            tests::{serve, serve_then_stall},
        },
        utils::tests::capture_logs,
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

        // Every IPv4 address belongs to this autonomous system.
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

    struct Downloaded {
        result: crate::Result<()>,
        // Missing if the download gave up before asking.
        request: Option<String>,
        #[cfg(feature = "tui")]
        events: Vec<crate::event::Event>,
    }

    fn reply(head: &str, body: &[u8]) -> Vec<u8> {
        let mut reply = format!(
            "HTTP/1.1 {head}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        reply.extend_from_slice(body);
        reply
    }

    async fn client() -> reqwest_middleware::ClientWithMiddleware {
        let config = test_config(|_| {}).await;
        http::create_reqwest_client(&config).await.unwrap()
    }

    #[cfg(target_os = "linux")]
    async fn download_from(address: std::net::SocketAddr) -> crate::Result<()> {
        #[cfg(feature = "tui")]
        let tx = tokio::sync::mpsc::unbounded_channel().0;
        DbType::Asn
            .download_from(
                &format!("http://{address}/asn.mmdb"),
                client().await,
                #[cfg(feature = "tui")]
                tx,
            )
            .await
    }

    async fn download(reply: Vec<u8>) -> Downloaded {
        let (address, requests) = serve(vec![reply]).await;
        #[cfg(feature = "tui")]
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        let result = DbType::Asn
            .download_from(
                &format!("http://{address}/asn.mmdb"),
                client().await,
                #[cfg(feature = "tui")]
                tx,
            )
            .await;

        let request = requests.lock().pop().map(|r| r.to_ascii_lowercase());
        Downloaded {
            result,
            request,
            #[cfg(feature = "tui")]
            events: std::iter::from_fn(|| rx.try_recv().ok()).collect(),
        }
    }

    async fn paths() -> [std::path::PathBuf; 3] {
        [
            cache::database_path(DbType::Asn).await,
            DbType::Asn.etag_path().await.unwrap(),
            DbType::Asn.tmp_path().await.unwrap(),
        ]
    }

    fn error_of(downloaded: Downloaded) -> String {
        downloaded.result.unwrap_err().to_string()
    }

    #[tokio::test]
    async fn a_new_database_is_saved_with_its_etag() {
        let cache = cache::take().await;
        let logs = capture_logs();
        let [db_path, etag_path, tmp_path] = paths().await;
        let database = cache::database(7);

        let downloaded = download(reply("200 OK\r\nETag: \"v1\"", &database)).await;

        let request = downloaded.request.as_deref().unwrap();
        assert!(request.starts_with("get /asn.mmdb http/1.1\r\n"), "{request}");
        assert!(!request.contains("if-none-match"), "{request}");
        assert_eq!(std::fs::read(&db_path).unwrap(), database);
        assert_eq!(std::fs::read(&etag_path).unwrap(), b"\"v1\"");
        assert!(!tmp_path.exists());
        let text = logs.text();
        assert!(text.contains("Downloaded ASN database to "), "{text}");
        assert!(text.contains(&db_path.display().to_string()), "{text}");

        #[cfg(feature = "tui")]
        assert_eq!(progress(&downloaded.events), (Some(database.len()), database.len()));
        downloaded.result.unwrap();
        drop(cache);
    }

    // The size the download announced, and how much of it arrived.
    #[cfg(feature = "tui")]
    fn progress(events: &[crate::event::Event]) -> (Option<usize>, usize) {
        use crate::event::{AppEvent, Event};

        let total = events
            .iter()
            .filter_map(|event| {
                if let Event::App(AppEvent::IpDbTotal(DbType::Asn, bytes)) = event {
                    Some(bytes.map(|bytes| usize::try_from(bytes).unwrap()))
                } else {
                    None
                }
            })
            .next_back()
            .flatten();
        let received = events
            .iter()
            .filter_map(|event| {
                if let Event::App(AppEvent::IpDbDownloaded(DbType::Asn, bytes)) = event {
                    Some(*bytes)
                } else {
                    None
                }
            })
            .sum();
        (total, received)
    }

    #[tokio::test]
    async fn an_unchanged_database_is_not_downloaded_again() {
        let cache = cache::take().await;
        let logs = capture_logs();
        let [db_path, etag_path, tmp_path] = paths().await;
        let database = cache::database(7);
        std::fs::write(&db_path, &database).unwrap();
        std::fs::write(&etag_path, "\"v1\"").unwrap();
        // Left by a download that was cut short.
        std::fs::write(&tmp_path, "partial").unwrap();

        let downloaded = download(reply("304 Not Modified", b"")).await;

        let request = downloaded.request.as_deref().unwrap();
        assert!(request.contains("\r\nif-none-match: \"v1\"\r\n"), "{request}");
        assert_eq!(std::fs::read(&db_path).unwrap(), database);
        assert!(!tmp_path.exists());
        let text = logs.text();
        assert!(text.contains("Latest ASN database is already cached at "), "{text}");
        downloaded.result.unwrap();
        drop(cache);
    }

    #[tokio::test]
    async fn a_damaged_database_is_downloaded_again_whatever_its_etag() {
        let cache = cache::take().await;
        let [db_path, etag_path, _] = paths().await;
        std::fs::write(&db_path, "<html>not a database</html>").unwrap();
        std::fs::write(&etag_path, "\"v1\"").unwrap();
        let database = cache::database(7);

        // Without an ETag, there is nothing to vouch for the new one.
        let downloaded = download(reply("200 OK", &database)).await;

        let request = downloaded.request.as_deref().unwrap();
        assert!(!request.contains("if-none-match"), "{request}");
        assert_eq!(std::fs::read(&db_path).unwrap(), database);
        assert!(!etag_path.exists());
        downloaded.result.unwrap();
        drop(cache);
    }

    #[tokio::test]
    async fn an_etag_that_is_no_header_value_is_not_sent() {
        let cache = cache::take().await;
        let [db_path, etag_path, _] = paths().await;
        std::fs::write(&db_path, cache::database(7)).unwrap();
        std::fs::write(&etag_path, "\"v1\"\n\"v2\"").unwrap();

        let downloaded = download(reply("200 OK", &cache::database(8))).await;

        let request = downloaded.request.as_deref().unwrap();
        assert!(!request.contains("if-none-match"), "{request}");
        assert_eq!(std::fs::read(&db_path).unwrap(), cache::database(8));
        downloaded.result.unwrap();
        drop(cache);
    }

    #[tokio::test]
    async fn a_reply_that_is_no_database_leaves_the_cached_one() {
        let cache = cache::take().await;
        let [db_path, ..] = paths().await;
        std::fs::write(&db_path, cache::database(7)).unwrap();

        let downloaded = download(reply("200 OK", b"<html>not a database</html>")).await;

        let error = error_of(downloaded);
        assert!(error.starts_with("failed to open IP database: "), "{error}");
        assert_eq!(std::fs::read(&db_path).unwrap(), cache::database(7));
        drop(cache);
    }

    #[tokio::test]
    async fn only_a_whole_database_counts_as_a_download() {
        let cache = cache::take().await;

        let error = error_of(download(reply("204 No Content", b"")).await);
        assert!(error.starts_with("HTTP status error (204 No Content) for url"), "{error}");

        let error = error_of(download(reply("404 Not Found", b"")).await);
        assert!(error.contains("404 Not Found"), "{error}");

        assert!(!cache::database_path(DbType::Asn).await.exists());
        drop(cache);
    }

    #[tokio::test]
    async fn an_etag_that_cannot_be_read_stops_the_download() {
        let cache = cache::take().await;
        let [db_path, etag_path, _] = paths().await;
        std::fs::write(&db_path, cache::database(7)).unwrap();
        std::fs::create_dir_all(etag_path.join("in-the-way")).unwrap();

        let downloaded = download(reply("200 OK", &cache::database(8))).await;

        assert!(downloaded.request.is_none());
        let error = error_of(downloaded);
        assert!(error.starts_with("failed to read file: "), "{error}");
        assert_eq!(std::fs::read(&db_path).unwrap(), cache::database(7));
        drop(cache);
    }

    #[tokio::test]
    async fn a_lost_etag_only_costs_a_repeat_download() {
        let cache = cache::take().await;
        let logs = capture_logs();
        let [db_path, etag_path, _] = paths().await;
        std::fs::create_dir_all(etag_path.join("in-the-way")).unwrap();

        let downloaded = download(reply("200 OK\r\nETag: \"v1\"", &cache::database(7))).await;
        downloaded.result.unwrap();
        assert_eq!(std::fs::read(&db_path).unwrap(), cache::database(7));
        let text = logs.text();
        assert!(
            text.contains(
                "ASN database will be downloaded again next run: failed to write to file: "
            ),
            "{text}"
        );

        // A database that opens would have the ETag read first.
        std::fs::remove_file(&db_path).unwrap();
        let downloaded = download(reply("200 OK", &cache::database(8))).await;
        downloaded.result.unwrap();
        assert_eq!(std::fs::read(&db_path).unwrap(), cache::database(8));
        let text = logs.text();
        assert!(
            text.contains(
                "ASN database will be downloaded again next run: failed to remove file: "
            ),
            "{text}"
        );
        drop(cache);
    }

    #[tokio::test]
    async fn a_database_that_cannot_be_put_in_place_is_an_error() {
        let cache = cache::take().await;
        let [db_path, _, tmp_path] = paths().await;

        std::fs::create_dir_all(tmp_path.join("in-the-way")).unwrap();
        let error = error_of(download(reply("200 OK", &cache::database(7))).await);
        assert!(error.starts_with("failed to create file: "), "{error}");
        std::fs::remove_dir_all(&tmp_path).unwrap();

        std::fs::create_dir_all(db_path.join("in-the-way")).unwrap();
        let error = error_of(download(reply("200 OK", &cache::database(7))).await);
        assert!(error.starts_with("failed to rename "), "{error}");
        assert!(db_path.join("in-the-way").is_dir());
        drop(cache);
    }

    // Only Linux has a device that is always full.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_full_disk_fails_the_download() {
        use tokio::{
            io::{AsyncReadExt as _, AsyncWriteExt as _},
            net::TcpListener,
        };

        let cache = cache::take().await;
        let [_, _, tmp_path] = paths().await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        // Any file left where the download goes is removed before it starts,
        // so the device takes its place only once it has.
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            assert_ne!(socket.read(&mut request).await.unwrap(), 0);
            std::os::unix::fs::symlink("/dev/full", tmp_path).unwrap();
            let body = vec![0_u8; 64 * 1024];
            socket.write_all(&reply("200 OK", &body)).await.unwrap();
        });

        let error = download_from(address).await.unwrap_err().to_string();

        assert!(error.starts_with("failed to write to file: "), "{error}");
        server.await.unwrap();
        drop(cache);
    }

    #[tokio::test]
    async fn the_cache_directory_is_created_where_it_belongs() {
        let directory = temp_directory("cache-directory");

        let path = create_cache_directory(Some(directory.clone())).await;
        assert_eq!(path.unwrap(), directory.join("proxy_scraper_checker"));
        assert!(directory.join("proxy_scraper_checker").is_dir());

        let error = create_cache_directory(None).await.unwrap_err();
        assert_eq!(error.to_string(), "failed to get user's cache directory");

        let file = directory.join("file");
        std::fs::write(&file, "").unwrap();
        let error = create_cache_directory(Some(file)).await.unwrap_err();
        assert!(error.to_string().starts_with("failed to create directory: "), "{error}");

        std::fs::remove_dir_all(directory).unwrap();
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
