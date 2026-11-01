use std::{
    cmp::Ordering,
    io,
    io::Write as _,
    net::{IpAddr, Ipv4Addr},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};

use color_eyre::eyre::WrapErr as _;
use compact_str::{CompactString, format_compact};
use serde::ser::SerializeSeq as _;

use crate::{
    config::Config,
    ipdb,
    proxy::{PerProxyType, Proxy, ProxyAuth, ProxyType, ProxyTypeSet},
    utils::{grouped, is_container, keep_both_errors, plural, pretty_error},
};

type IpDb = maxminddb::Reader<maxminddb::Mmap>;

const WRITE_BUFFER_BYTES: usize = 128 * 1024;

fn compare_timeout(a: &Proxy, b: &Proxy) -> Ordering {
    a.timeout
        .unwrap_or(Duration::MAX)
        .cmp(&b.timeout.unwrap_or(Duration::MAX))
        .then_with(|| a.protocol.cmp(&b.protocol))
        .then_with(|| a.host.cmp(&b.host))
        .then_with(|| a.port.cmp(&b.port))
        .then_with(|| a.auth.cmp(&b.auth))
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum HostSortKey {
    Ipv4([u8; 4]),
    Name(compact_str::CompactString),
}

fn natural_sort_key(
    proxy: &Proxy,
) -> (ProxyType, HostSortKey, u16, Option<Box<ProxyAuth>>) {
    let host = proxy.host.parse::<Ipv4Addr>().map_or_else(
        |_| HostSortKey::Name(proxy.host.clone()),
        |ip| HostSortKey::Ipv4(ip.octets()),
    );
    (proxy.protocol, host, proxy.port, proxy.auth.clone())
}

fn keep_english_name(names: &mut maxminddb::geoip2::Names<'_>) {
    *names = maxminddb::geoip2::Names {
        english: names.english,
        ..Default::default()
    };
}

fn strip_non_english_names(city: &mut maxminddb::geoip2::City<'_>) {
    keep_english_name(&mut city.city.names);
    keep_english_name(&mut city.continent.names);
    keep_english_name(&mut city.country.names);
    keep_english_name(&mut city.registered_country.names);
    keep_english_name(&mut city.represented_country.names);
    for subdivision in &mut city.subdivisions {
        keep_english_name(&mut subdivision.names);
    }
}

// A bad record costs one proxy its ASN or geolocation, not the whole save.
fn lookup<'a, T>(db: &'a IpDb, addr: IpAddr) -> Option<T>
where
    T: serde::Deserialize<'a>,
{
    match db.lookup(addr).and_then(|entry| entry.decode()) {
        Ok(record) => record,
        Err(e) => {
            tracing::warn!("failed to look up {addr} in IP database: {e}");
            None
        }
    }
}

#[derive(serde::Serialize)]
struct ProxyJson<'a> {
    protocol: ProxyType,
    username: Option<&'a str>,
    password: Option<&'a str>,
    host: &'a str,
    port: u16,
    timeout: Option<f64>,
    exit_ip: Option<Ipv4Addr>,
    asn: Option<maxminddb::geoip2::Asn<'a>>,
    geolocation: Option<maxminddb::geoip2::City<'a>>,
}

struct ProxyListJson<'a> {
    proxies: &'a [Proxy],
    asn_db: Option<&'a IpDb>,
    geo_db: Option<&'a IpDb>,
}

impl serde::Serialize for ProxyListJson<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut seq = serializer.serialize_seq(Some(self.proxies.len()))?;
        for proxy in self.proxies {
            let exit_ip_addr = proxy.exit_ip.map(IpAddr::V4);

            let asn = self
                .asn_db
                .zip(exit_ip_addr)
                .and_then(|(db, addr)| lookup(db, addr));

            let geolocation = self
                .geo_db
                .zip(exit_ip_addr)
                .and_then(|(db, addr)| {
                    lookup::<maxminddb::geoip2::City<'_>>(db, addr)
                })
                .map(|mut city| {
                    strip_non_english_names(&mut city);
                    city
                });

            seq.serialize_element(&ProxyJson {
                protocol: proxy.protocol,
                username: proxy.auth.as_ref().map(|a| a.username.as_str()),
                password: proxy.auth.as_ref().map(|a| a.password.as_str()),
                host: &proxy.host,
                port: proxy.port,
                timeout: proxy
                    .timeout
                    .map(|d| (d.as_secs_f64() * 100.0).round() / 100.0_f64),
                exit_ip: proxy.exit_ip,
                asn,
                geolocation,
            })?;
        }
        seq.end()
    }
}

async fn remove_dir(path: &Path) -> crate::Result<()> {
    match tokio::fs::remove_dir_all(path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).wrap_err_with(|| {
            compact_str::format_compact!(
                "failed to remove directory: {}",
                path.display()
            )
        }),
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut path = path.as_os_str().to_owned();
    path.push(suffix);
    PathBuf::from(path)
}

fn tmp_path(path: &Path) -> PathBuf {
    with_suffix(path, ".tmp")
}

fn stage_json_file(
    path: &Path,
    pretty: bool,
    list: &ProxyListJson<'_>,
) -> crate::Result<()> {
    let tmp = tmp_path(path);
    let file = std::fs::File::create(&tmp).wrap_err_with(|| {
        compact_str::format_compact!("failed to create file: {}", tmp.display())
    })?;
    let mut writer =
        std::io::BufWriter::with_capacity(WRITE_BUFFER_BYTES, file);

    let failed = || {
        compact_str::format_compact!(
            "failed to write to file: {}",
            tmp.display()
        )
    };

    if pretty {
        serde_json::to_writer_pretty(&mut writer, list)
    } else {
        serde_json::to_writer(&mut writer, list)
    }
    .wrap_err_with(failed)?;

    writer.flush().wrap_err_with(failed)
}

fn rename_failed(from: &Path, to: &Path) -> compact_str::CompactString {
    compact_str::format_compact!(
        "failed to rename {} to {}",
        from.display(),
        to.display()
    )
}

async fn commit_file(from: &Path, to: &Path) -> crate::Result<()> {
    match tokio::fs::rename(from, to).await {
        Ok(()) => Ok(()),
        // Windows refuses to replace a file that a reader holds open, though
        // it usually allows overwriting it. Unlike a rename, that isn't atomic,
        // so it is only the fallback.
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            tokio::fs::copy(from, to).await.wrap_err_with(|| {
                compact_str::format_compact!(
                    "failed to copy {} to {}",
                    from.display(),
                    to.display()
                )
            })?;
            tokio::fs::remove_file(from).await.wrap_err_with(|| {
                compact_str::format_compact!(
                    "failed to remove file: {}",
                    from.display()
                )
            })
        }
        Err(e) => Err(e).wrap_err_with(|| rename_failed(from, to)),
    }
}

// Whatever something still holds stays for a later save to clear, since a
// reader that stays open across runs must not fail them. One at a time, so
// that it doesn't keep the rest here too.
async fn clear_previous(previous: &Path) {
    let Ok(mut entries) = tokio::fs::read_dir(previous).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let removed = if entry.file_type().await.is_ok_and(|kind| kind.is_dir())
        {
            remove_dir(&path).await
        } else {
            tokio::fs::remove_file(&path).await.wrap_err_with(|| {
                format_compact!("failed to remove file: {}", path.display())
            })
        };
        if let Err(e) = removed {
            tracing::warn!("{}", pretty_error(&e));
        }
    }
    // It stays only if something in it did, which was just reported.
    drop(tokio::fs::remove_dir(previous).await);
}

async fn move_aside(to: &Path, previous: &Path) -> crate::Result<PathBuf> {
    tokio::fs::create_dir_all(previous).await.wrap_err_with(|| {
        format_compact!("failed to create directory: {}", previous.display())
    })?;
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let aside = previous.join(format_compact!("{stamp}"));
    tokio::fs::rename(to, &aside)
        .await
        .wrap_err_with(|| rename_failed(to, &aside))?;
    Ok(aside)
}

// The previous directory is moved aside rather than deleted first: a file
// held open would otherwise fail the deletion halfway through, and a commit
// that fails can still put it back. It goes inside a directory of its own, so
// that one that can't be deleted yet doesn't get in the next one's way.
async fn commit_directory(from: &Path, to: &Path) -> crate::Result<()> {
    let previous = with_suffix(to, ".old");

    // With nothing at `to`, an earlier commit was cut short, and what it moved
    // aside is the last output there is until the new one lands.
    let nothing_there = matches!(
        tokio::fs::symlink_metadata(to).await,
        Err(e) if e.kind() == io::ErrorKind::NotFound
    );
    let aside = if nothing_there {
        None
    } else {
        clear_previous(&previous).await;
        match move_aside(to, &previous).await {
            Ok(aside) => Some(aside),
            Err(e) => {
                clear_previous(&previous).await;
                return Err(e);
            }
        }
    };

    if let Err(e) = tokio::fs::rename(from, to).await {
        let mut failed = rename_failed(from, to);
        if let Some(aside) = &aside {
            match tokio::fs::rename(aside, to).await {
                Ok(()) => clear_previous(&previous).await,
                Err(restore_error) => {
                    failed = format_compact!(
                        "{failed}; the previous output is in {}: \
                         {restore_error}",
                        aside.display()
                    );
                }
            }
        }
        return Err(e).wrap_err(failed);
    }

    clear_previous(&previous).await;
    Ok(())
}

fn write_proxy_list_to_file<'a, I>(
    path: &Path,
    proxies: I,
    include_protocol: bool,
    buf: &mut String,
) -> crate::Result<()>
where
    I: IntoIterator<Item = &'a Proxy>,
{
    let mut file = std::fs::File::create(path).wrap_err_with(|| {
        compact_str::format_compact!(
            "failed to create file: {}",
            path.display()
        )
    })?;

    let write = |file: &mut std::fs::File, buf: &str| {
        file.write_all(buf.as_bytes()).wrap_err_with(|| {
            compact_str::format_compact!(
                "failed to write to file: {}",
                path.display()
            )
        })
    };

    buf.clear();
    let mut first = true;
    for proxy in proxies {
        if first {
            first = false;
        } else {
            buf.push('\n');
        }
        proxy.write_to(buf, include_protocol);

        if buf.len() >= WRITE_BUFFER_BYTES {
            write(&mut file, buf)?;
            buf.clear();
        }
    }

    if !buf.is_empty() {
        write(&mut file, buf)?;
        buf.clear();
    }

    Ok(())
}

async fn open_ipdb(db_type: ipdb::DbType, enabled: bool) -> Option<Arc<IpDb>> {
    if !enabled {
        return None;
    }
    match db_type.open_mmap().await {
        Ok(db) => Some(Arc::new(db)),
        Err(e) => {
            tracing::warn!(
                "{} data will be missing from the output: {}",
                db_type.name(),
                pretty_error(&e)
            );
            None
        }
    }
}

#[expect(
    clippy::rc_buffer,
    reason = "Arc<[Proxy]> would copy every proxy out of the Vec"
)]
async fn stage_json(
    proxies: &Arc<Vec<Proxy>>,
    config: &Config,
    paths: &[(PathBuf, bool); 2],
) -> crate::Result<()> {
    let (asn_db, geo_db) = tokio::join!(
        open_ipdb(ipdb::DbType::Asn, config.asn_enabled()),
        open_ipdb(ipdb::DbType::Geo, config.geolocation_enabled()),
    );

    let tasks = paths.each_ref().map(move |entry| {
        let proxies = Arc::clone(proxies);
        let asn_db = asn_db.clone();
        let geo_db = geo_db.clone();
        let (path, pretty) = (entry.0.clone(), entry.1);
        tokio::task::spawn_blocking(move || {
            let list = ProxyListJson {
                proxies: &proxies,
                asn_db: asn_db.as_deref(),
                geo_db: geo_db.as_deref(),
            };
            stage_json_file(&path, pretty, &list)
        })
    });

    for task in tasks {
        task.await??;
    }

    Ok(())
}

#[expect(
    clippy::rc_buffer,
    reason = "Arc<[Proxy]> would copy every proxy out of the Vec"
)]
async fn stage_txt(
    proxies: Arc<Vec<Proxy>>,
    enabled_protocols: ProxyTypeSet,
    staging_path: PathBuf,
) -> crate::Result<()> {
    remove_dir(&staging_path).await?;
    tokio::fs::create_dir_all(&staging_path).await.wrap_err_with(|| {
        compact_str::format_compact!(
            "failed to create directory: {}",
            staging_path.display()
        )
    })?;

    tokio::task::spawn_blocking(move || {
        let mut buf = String::with_capacity(WRITE_BUFFER_BYTES);

        write_proxy_list_to_file(
            &staging_path.join("all.txt"),
            proxies.iter(),
            true,
            &mut buf,
        )?;

        for proto in enabled_protocols.iter() {
            let mut file_path = staging_path.join(proto.as_str_lowercase());
            file_path.set_extension("txt");
            write_proxy_list_to_file(
                &file_path,
                proxies.iter().filter(move |p| p.protocol == proto),
                false,
                &mut buf,
            )?;
        }

        crate::Result::Ok(())
    })
    .await?
}

// Returns where the proxies went, for the summary of the run.
pub async fn save_proxies(
    config: Arc<Config>,
    mut proxies: Vec<Proxy>,
) -> crate::Result<CompactString> {
    if config.output.sort_by_speed {
        proxies.sort_unstable_by(compare_timeout);
    } else {
        proxies.sort_by_cached_key(natural_sort_key);
    }
    let proxies = Arc::new(proxies);

    let json_paths = [
        (config.output.path.join("proxies.json"), false),
        (config.output.path.join("proxies_pretty.json"), true),
    ];
    let directory_path = config.output.path.join("proxies");
    let staging_path = tmp_path(&directory_path);

    tokio::try_join!(
        async {
            if config.output.json.enabled {
                stage_json(&proxies, &config, &json_paths).await
            } else {
                Ok(())
            }
        },
        async {
            if config.output.txt.enabled {
                stage_txt(
                    Arc::clone(&proxies),
                    config.scraping.enabled_protocols,
                    staging_path.clone(),
                )
                .await
            } else {
                Ok(())
            }
        }
    )?;

    drop(proxies);

    // A file held open somewhere must not cost the formats that can be saved.
    let mut committed = Ok(());
    if config.output.json.enabled {
        for (path, _) in &json_paths {
            committed = keep_both_errors(
                committed,
                commit_file(&tmp_path(path), path).await,
            );
        }
    }
    if config.output.txt.enabled {
        committed = keep_both_errors(
            committed,
            commit_directory(&staging_path, &directory_path).await,
        );
    }
    committed?;

    let path = std::path::absolute(&config.output.path)
        .unwrap_or_else(move |_| config.output.path.clone());
    Ok(if is_container().await {
        format_compact!("./out ({} in container)", path.display())
    } else {
        format_compact!("{}", path.display())
    })
}

pub fn count_by_protocol(proxies: &[Proxy]) -> PerProxyType<usize> {
    let mut counts = PerProxyType::<usize>::default();
    for proxy in proxies {
        let count = counts.get_mut(proxy.protocol);
        *count = count.saturating_add(1);
    }
    counts
}

pub fn summary(
    config: &Config,
    found: &PerProxyType<usize>,
    saved: &PerProxyType<usize>,
    location: &str,
    stopped: bool,
) -> CompactString {
    let protocols = config.scraping.enabled_protocols;
    let total = move |counts: &PerProxyType<usize>| -> usize {
        protocols.iter().map(|protocol| *counts.get(protocol)).sum()
    };

    let saved_total = total(saved);
    let found_total = total(found);
    let mut text = if stopped {
        // Out of everything found, the rest would read as having failed.
        format_compact!(
            "Saved {} {} found before the stop to {location}",
            grouped(saved_total),
            plural(saved_total, "proxy", "proxies")
        )
    } else if config.checking.check_url.is_some() {
        format_compact!(
            "Saved {} of {} {} to {location}",
            grouped(saved_total),
            grouped(found_total),
            plural(found_total, "proxy", "proxies")
        )
    } else {
        format_compact!(
            "Saved {} {} to {location}",
            grouped(saved_total),
            plural(saved_total, "proxy", "proxies")
        )
    };

    if protocols.iter().nth(1).is_some() {
        let breakdown: Vec<_> = protocols
            .iter()
            .map(|protocol| {
                format_compact!(
                    "{} {}",
                    protocol.as_str_uppercase(),
                    grouped(*saved.get(protocol))
                )
            })
            .collect();
        text.push_str(" (");
        text.push_str(&breakdown.join(", "));
        text.push(')');
    }
    text
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Arc, time::Duration};

    use compact_str::CompactString;

    use super::{
        Config, Proxy, ProxyAuth, ProxyType, commit_directory, compare_timeout,
        count_by_protocol, natural_sort_key, save_proxies, summary,
    };
    use crate::config::tests::{temp_directory, test_config};

    fn proxy(host: &str, port: u16, timeout: Option<Duration>) -> Proxy {
        Proxy {
            protocol: ProxyType::Http,
            host: CompactString::new(host),
            port,
            auth: None,
            timeout,
            exit_ip: None,
        }
    }

    fn typed(protocol: ProxyType) -> Proxy {
        Proxy { protocol, ..proxy("1.1.1.1", 80, None) }
    }

    #[tokio::test]
    async fn the_summary_counts_what_was_saved_out_of_what_was_found() {
        let config = test_config(|_| {}).await;
        let found = count_by_protocol(&[
            typed(ProxyType::Http),
            typed(ProxyType::Http),
            typed(ProxyType::Socks5),
        ]);
        let saved = count_by_protocol(&[typed(ProxyType::Http)]);

        assert_eq!(
            summary(&config, &found, &saved, "./out", false),
            "Saved 1 of 3 proxies to ./out (HTTP 1, SOCKS4 0, SOCKS5 0)"
        );
        assert_eq!(
            summary(&config, &found, &saved, "./out", true),
            "Saved 1 proxy found before the stop to ./out (HTTP 1, SOCKS4 0, \
             SOCKS5 0)"
        );
    }

    #[tokio::test]
    async fn the_summary_drops_what_means_nothing_in_this_run() {
        let config = test_config(|raw| {
            raw.checking.check_url = None;
            raw.scraping.socks4.enabled = false;
            raw.scraping.socks5.enabled = false;
        })
        .await;
        let saved = count_by_protocol(&[typed(ProxyType::Http)]);

        assert_eq!(
            summary(&config, &saved, &saved, "./out", false),
            "Saved 1 proxy to ./out"
        );
    }

    fn sorted_naturally(mut proxies: Vec<Proxy>) -> Vec<CompactString> {
        proxies.sort_by_cached_key(natural_sort_key);
        proxies.iter().map(|p| p.to_compact_string(false)).collect()
    }

    #[test]
    fn addresses_sort_numerically_not_lexicographically() {
        assert_eq!(
            sorted_naturally(vec![
                proxy("1.2.3.10", 80, None),
                proxy("1.2.3.9", 8080, None),
                proxy("1.2.3.9", 80, None),
            ]),
            ["1.2.3.9:80", "1.2.3.9:8080", "1.2.3.10:80"]
        );
    }

    #[test]
    fn hostnames_sort_after_addresses() {
        assert_eq!(
            sorted_naturally(vec![
                proxy("1proxy.example.com", 80, None),
                proxy("255.255.255.255", 80, None),
            ]),
            ["255.255.255.255:80", "1proxy.example.com:80"]
        );
    }

    #[test]
    fn speed_order_puts_unchecked_proxies_last() {
        let mut proxies = [
            proxy("1.1.1.1", 80, None),
            proxy("2.2.2.2", 80, Some(Duration::from_millis(900))),
            proxy("3.3.3.3", 80, Some(Duration::from_millis(100))),
        ];
        proxies.sort_unstable_by(compare_timeout);

        let hosts: Vec<_> = proxies.iter().map(|p| p.host.as_str()).collect();
        assert_eq!(hosts, ["3.3.3.3", "2.2.2.2", "1.1.1.1"]);
    }

    #[test]
    fn speed_ties_are_broken_by_identity() {
        let timeout = Some(Duration::from_secs(1));
        let mut proxies = [
            proxy("2.2.2.2", 80, timeout),
            proxy("1.1.1.1", 8080, timeout),
            proxy("1.1.1.1", 80, timeout),
        ];
        proxies.sort_unstable_by(compare_timeout);

        let order: Vec<_> =
            proxies.iter().map(|p| p.to_compact_string(false)).collect();
        assert_eq!(order, ["1.1.1.1:80", "1.1.1.1:8080", "2.2.2.2:80"]);
    }

    #[test]
    fn natural_order_breaks_ties_on_credentials() {
        let mut with_auth = proxy("1.1.1.1", 80, None);
        with_auth.auth = Some(Box::new(ProxyAuth {
            username: CompactString::const_new("user"),
            password: CompactString::const_new("pass"),
        }));

        assert_eq!(
            sorted_naturally(vec![with_auth, proxy("1.1.1.1", 80, None)]),
            ["1.1.1.1:80", "user:pass@1.1.1.1:80"]
        );
    }

    async fn unchecked_config(path: PathBuf) -> Arc<Config> {
        let mut config = test_config(|raw| {
            raw.checking.check_url = None;
            raw.output.sort_by_speed = false;
            raw.scraping.http.enabled = true;
            raw.scraping.socks4.enabled = false;
            raw.scraping.socks5.enabled = false;
        })
        .await;
        config.output.path = path;
        Arc::new(config)
    }

    #[tokio::test]
    async fn saving_replaces_the_previous_output() {
        let directory = temp_directory("replace");
        let stale = directory.join("proxies").join("socks4.txt");
        std::fs::create_dir_all(directory.join("proxies")).unwrap();
        std::fs::write(&stale, "stale").unwrap();
        std::fs::write(directory.join("proxies.json"), "stale").unwrap();

        let config = unchecked_config(directory.clone()).await;
        save_proxies(config, vec![proxy("1.1.1.1", 80, None)]).await.unwrap();

        assert!(!stale.exists());
        assert_eq!(
            std::fs::read_to_string(directory.join("proxies/all.txt")).unwrap(),
            "http://1.1.1.1:80"
        );
        assert_eq!(
            std::fs::read_to_string(directory.join("proxies/http.txt"))
                .unwrap(),
            "1.1.1.1:80"
        );
        for name in ["proxies.json", "proxies_pretty.json"] {
            let json: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(directory.join(name)).unwrap(),
            )
            .unwrap();
            assert_eq!(json.pointer("/0/host").unwrap(), "1.1.1.1");
        }

        let leftovers = std::fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| name.to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0);

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_reader_holding_the_output_open_does_not_fail_the_save() {
        use std::os::windows::fs::OpenOptionsExt as _;

        const FILE_SHARE_READ_WRITE: u32 = 1 | 2;

        let directory = temp_directory("held");
        let path = directory.join("proxies.json");
        std::fs::write(&path, "stale").unwrap();
        let reader = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ_WRITE)
            .open(&path)
            .unwrap();

        let config = unchecked_config(directory.clone()).await;
        save_proxies(config, vec![proxy("1.1.1.1", 80, None)]).await.unwrap();
        drop(reader);

        assert!(std::fs::read_to_string(&path).unwrap().contains("1.1.1.1"));
        assert!(!directory.join("proxies.json.tmp").exists());

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_held_file_leaves_the_previous_directory_whole() {
        use std::os::windows::fs::OpenOptionsExt as _;

        const FILE_SHARE_READ: u32 = 1;

        let directory = temp_directory("whole");
        let proxies = directory.join("proxies");
        std::fs::create_dir_all(&proxies).unwrap();
        std::fs::write(proxies.join("all.txt"), "old").unwrap();
        std::fs::write(proxies.join("http.txt"), "old").unwrap();
        let reader = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(proxies.join("http.txt"))
            .unwrap();

        let config = unchecked_config(directory.clone()).await;
        let result =
            save_proxies(config, vec![proxy("1.1.1.1", 80, None)]).await;
        drop(reader);

        drop(result.unwrap_err());
        for name in ["all.txt", "http.txt"] {
            assert_eq!(
                std::fs::read_to_string(proxies.join(name)).unwrap(),
                "old"
            );
        }

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn one_failed_commit_does_not_hold_back_the_others() {
        let directory = temp_directory("partial");
        std::fs::create_dir_all(directory.join("proxies.json/taken")).unwrap();

        let config = unchecked_config(directory.clone()).await;
        let result =
            save_proxies(config, vec![proxy("1.1.1.1", 80, None)]).await;

        drop(result.unwrap_err());
        assert!(directory.join("proxies_pretty.json").is_file());
        assert_eq!(
            std::fs::read_to_string(directory.join("proxies/all.txt")).unwrap(),
            "http://1.1.1.1:80"
        );

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn a_failed_commit_puts_the_previous_directory_back() {
        let directory = temp_directory("restore");
        let proxies = directory.join("proxies");
        std::fs::create_dir_all(&proxies).unwrap();
        std::fs::write(proxies.join("all.txt"), "old").unwrap();

        let result =
            commit_directory(&directory.join("missing"), &proxies).await;

        drop(result.unwrap_err());
        assert_eq!(
            std::fs::read_to_string(proxies.join("all.txt")).unwrap(),
            "old"
        );
        assert!(!directory.join("proxies.old").exists());

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn a_commit_cut_short_keeps_what_it_moved_aside() {
        let directory = temp_directory("cut-short");
        let proxies = directory.join("proxies");
        let staged = directory.join("proxies.tmp");
        let previous = directory.join("proxies.old");
        let aside = previous.join("1");
        std::fs::create_dir_all(&aside).unwrap();
        std::fs::write(aside.join("all.txt"), "old").unwrap();

        let result = commit_directory(&staged, &proxies).await;

        drop(result.unwrap_err());
        assert_eq!(
            std::fs::read_to_string(aside.join("all.txt")).unwrap(),
            "old"
        );

        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(staged.join("all.txt"), "new").unwrap();
        commit_directory(&staged, &proxies).await.unwrap();

        assert_eq!(
            std::fs::read_to_string(proxies.join("all.txt")).unwrap(),
            "new"
        );
        assert!(!previous.exists());

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn a_file_where_the_directory_goes_is_replaced_and_cleared() {
        let directory = temp_directory("file-in-the-way");
        let proxies = directory.join("proxies");
        let staged = directory.join("proxies.tmp");
        std::fs::write(&proxies, "not a directory").unwrap();
        std::fs::create_dir_all(&staged).unwrap();

        commit_directory(&staged, &proxies).await.unwrap();

        assert!(proxies.is_dir());
        assert!(!directory.join("proxies.old").exists());

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn a_backup_beside_the_output_is_left_alone() {
        let directory = temp_directory("backup");
        let proxies = directory.join("proxies");
        let staged = directory.join("proxies.tmp");
        let backup = directory.join("proxies.old1");
        std::fs::create_dir_all(&proxies).unwrap();
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::create_dir_all(&backup).unwrap();
        std::fs::write(backup.join("mine.txt"), "mine").unwrap();

        commit_directory(&staged, &proxies).await.unwrap();

        assert!(backup.join("mine.txt").is_file());

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_dangling_link_where_the_directory_goes_is_replaced() {
        let directory = temp_directory("dangling");
        let proxies = directory.join("proxies");
        let staged = directory.join("proxies.tmp");
        std::os::unix::fs::symlink(directory.join("gone"), &proxies).unwrap();
        std::fs::create_dir_all(&staged).unwrap();

        commit_directory(&staged, &proxies).await.unwrap();

        assert!(proxies.is_dir());

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn every_failed_commit_is_reported() {
        let directory = temp_directory("every-failure");
        std::fs::create_dir_all(directory.join("proxies.json/taken")).unwrap();
        std::fs::create_dir_all(directory.join("proxies_pretty.json/taken"))
            .unwrap();

        let config = unchecked_config(directory.clone()).await;
        let error = save_proxies(config, vec![proxy("1.1.1.1", 80, None)])
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("proxies.json.tmp"), "{error}");
        assert!(error.contains("proxies_pretty.json.tmp"), "{error}");

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_held_previous_directory_does_not_fail_the_next_saves() {
        use std::os::windows::fs::OpenOptionsExt as _;

        const FILE_SHARE_READ: u32 = 1;

        let directory = temp_directory("held-old");
        let proxies = directory.join("proxies");
        let previous = directory.join("proxies.old");
        let held = previous.join("1");
        std::fs::create_dir_all(&proxies).unwrap();
        std::fs::create_dir_all(&held).unwrap();
        std::fs::write(proxies.join("all.txt"), "old").unwrap();
        std::fs::write(held.join("all.txt"), "older").unwrap();
        let reader = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(held.join("all.txt"))
            .unwrap();

        let config = unchecked_config(directory.clone()).await;
        save_proxies(Arc::clone(&config), vec![proxy("1.1.1.1", 80, None)])
            .await
            .unwrap();
        save_proxies(Arc::clone(&config), vec![proxy("2.2.2.2", 80, None)])
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(proxies.join("all.txt")).unwrap(),
            "http://2.2.2.2:80"
        );
        assert_eq!(std::fs::read_dir(&previous).unwrap().count(), 1);

        drop(reader);
        save_proxies(config, vec![proxy("3.3.3.3", 80, None)]).await.unwrap();
        assert!(!previous.exists());

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn failed_staging_keeps_the_previous_output() {
        let directory = temp_directory("keep");
        std::fs::create_dir_all(directory.join("proxies")).unwrap();
        std::fs::write(directory.join("proxies/all.txt"), "old").unwrap();
        std::fs::write(directory.join("proxies.json"), "old").unwrap();
        std::fs::create_dir_all(directory.join("proxies.json.tmp")).unwrap();

        let config = unchecked_config(directory.clone()).await;
        let result =
            save_proxies(config, vec![proxy("1.1.1.1", 80, None)]).await;

        drop(result.unwrap_err());
        for path in ["proxies.json", "proxies/all.txt"] {
            assert_eq!(
                std::fs::read_to_string(directory.join(path)).unwrap(),
                "old"
            );
        }

        // Failing staging leaves the other format's task still writing here.
        drop(std::fs::remove_dir_all(directory));
    }
}
