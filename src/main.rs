#![deny(
    warnings,
    deprecated_safe,
    future_incompatible,
    keyword_idents,
    let_underscore,
    nonstandard_style,
    refining_impl_trait,
    rust_2018_compatibility,
    rust_2018_idioms,
    rust_2021_compatibility,
    rust_2024_compatibility,
    unused,
    clippy::all,
    clippy::pedantic,
    clippy::restriction,
    clippy::nursery,
    clippy::cargo
)]
#![allow(
    clippy::absolute_paths,
    clippy::allow_attributes_without_reason,
    clippy::arbitrary_source_item_ordering,
    clippy::as_conversions,
    clippy::blanket_clippy_restriction_lints,
    clippy::cast_precision_loss,
    clippy::cognitive_complexity,
    clippy::else_if_without_else,
    clippy::float_arithmetic,
    clippy::implicit_return,
    clippy::integer_division_remainder_used,
    clippy::iter_over_hash_type,
    clippy::min_ident_chars,
    clippy::missing_docs_in_private_items,
    clippy::mod_module_files,
    clippy::multiple_crate_versions,
    clippy::pattern_type_mismatch,
    clippy::question_mark_used,
    clippy::separated_literal_suffix,
    clippy::shadow_reuse,
    clippy::shadow_unrelated,
    clippy::single_call_fn,
    clippy::single_char_lifetime_names,
    clippy::std_instead_of_alloc,
    clippy::std_instead_of_core,
    clippy::too_many_lines,
    clippy::unwrap_used
)]

mod checker;
mod config;
#[cfg(feature = "tui")]
mod event;
mod http;
mod ipdb;
mod output;
mod parsers;
mod proxy;
mod raw_config;
mod scraper;
#[cfg(feature = "tui")]
mod tui;
mod utils;

#[cfg(feature = "tui")]
use std::io::Write as _;
use std::{io::IsTerminal as _, sync::Arc};

use tracing_subscriber::{
    layer::SubscriberExt as _, util::SubscriberInitExt as _,
};

#[cfg(feature = "dhat")]
#[global_allocator]
static GLOBAL: dhat::Alloc = dhat::Alloc;

#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(feature = "__mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

// auto-allocator replaces the system allocator only where that measurably
// wins, and both replacements also take over malloc for the C dependencies
// (aws-lc). On glibc and Windows the system allocator had the lowest peak RSS
// and mimalloc/jemalloc saved no significant CPU. The BSDs ship jemalloc and
// Android ships Scudo as their libc allocator already.

// macOS: 12-58% lower peak RSS than libmalloc.
#[cfg(all(feature = "auto-allocator", target_os = "macos"))]
#[global_allocator]
static GLOBAL: tikv_jemallocator_auto::Jemalloc =
    tikv_jemallocator_auto::Jemalloc;

// musl: 8-18% less CPU than mallocng when checking. mimalloc rather than
// jemalloc because jemalloc fixes the page size at build time, and a
// cross-compiled build assumes 4 KiB pages and aborts on 16/64 KiB kernels
// (Raspberry Pi 5, Asahi, ppc64le). v2 with no_thp costs 5-25% peak RSS over
// mallocng; v3 or letting it madvise(MADV_HUGEPAGE) costs up to 60%.
// mimalloc needs 64-bit atomics, which armv5te lacks.
#[cfg(all(
    feature = "auto-allocator",
    target_env = "musl",
    target_has_atomic = "64"
))]
#[global_allocator]
static GLOBAL: mimalloc_auto::MiMalloc = mimalloc_auto::MiMalloc;

type Error = color_eyre::Report;
type Result<T> = color_eyre::Result<T>;

type HashMap<K, V> = rapidhash::RapidHashMap<K, V>;
type HashSet<T> = rapidhash::RapidHashSet<T>;

fn create_logging_filter(debug: bool) -> tracing_subscriber::filter::Targets {
    let base = tracing_subscriber::filter::Targets::new()
        .with_default(tracing::level_filters::LevelFilter::INFO)
        .with_target(
            "rustls_platform_verifier::verification",
            tracing::level_filters::LevelFilter::OFF,
        );

    if debug {
        base.with_target(
            "proxy_scraper_checker",
            tracing::level_filters::LevelFilter::DEBUG,
        )
    } else {
        base
    }
}

async fn download_ipdb(
    config: Arc<config::Config>,
    http_client: reqwest_middleware::ClientWithMiddleware,
    token: tokio_util::sync::CancellationToken,
    #[cfg(feature = "tui")] tx: tokio::sync::mpsc::UnboundedSender<
        event::Event,
    >,
) {
    let databases = [
        (ipdb::DbType::Asn, config.asn_enabled()),
        (ipdb::DbType::Geo, config.geolocation_enabled()),
    ];
    drop(config);

    let mut join_set = tokio::task::JoinSet::new();

    for (db_type, enabled) in databases {
        if !enabled {
            continue;
        }

        let http_client = http_client.clone();
        let token = token.clone();
        #[cfg(feature = "tui")]
        let tx = tx.clone();

        join_set.spawn(async move {
            let download = db_type.download(
                http_client,
                #[cfg(feature = "tui")]
                tx.clone(),
            );

            let result = tokio::select! {
                biased;
                res = download => res,
                () = token.cancelled() => Ok(()),
            };

            if let Err(e) = &result {
                tracing::warn!(
                    "failed to download {} database: {}",
                    db_type.name(),
                    utils::pretty_error(e),
                );
            }

            #[cfg(feature = "tui")]
            if !token.is_cancelled() {
                drop(tx.send(event::Event::App(
                    event::AppEvent::IpDbFinished(db_type, result.is_ok()),
                )));
            }
        });
    }

    drop(http_client);
    drop(token);
    #[cfg(feature = "tui")]
    drop(tx);

    // A panic here should cost the ASN and geolocation data, not the run:
    // every real download failure is already logged inside the task, and
    // save_proxies degrades gracefully when a database is unavailable.
    while let Some(res) = join_set.join_next().await {
        if let Err(e) = res {
            tracing::error!(
                "IP database task panicked: {}",
                utils::pretty_error(&e.into())
            );
        }
    }
}

async fn main_task(
    config: Arc<config::Config>,
    token: tokio_util::sync::CancellationToken,
    #[cfg(feature = "tui")] tx: tokio::sync::mpsc::UnboundedSender<
        event::Event,
    >,
) -> crate::Result<compact_str::CompactString> {
    let http_client = http::create_reqwest_client(&config).await?;

    // The databases download while scraping rather than while checking, so
    // the download does not compete with the checks for bandwidth and skew
    // their timeouts.
    let ((), scraped) = tokio::join!(
        download_ipdb(
            Arc::clone(&config),
            http_client.clone(),
            token.clone(),
            #[cfg(feature = "tui")]
            tx.clone(),
        ),
        scraper::scrape_all(
            Arc::clone(&config),
            http_client,
            token.clone(),
            #[cfg(feature = "tui")]
            tx.clone(),
        ),
    );

    let scraped = scraped?;
    let found = output::count_by_protocol(&scraped);

    let proxies = checker::check_all(
        Arc::clone(&config),
        scraped,
        token.clone(),
        #[cfg(feature = "tui")]
        tx.clone(),
    )
    .await?;

    let summary = if proxies.is_empty() {
        let summary = compact_str::CompactString::const_new(
            "No proxies to save - previous results left untouched",
        );
        tracing::warn!("{summary}");
        summary
    } else {
        // A stop during the save comes after everything was checked.
        let stopped = token.is_cancelled();
        let saved = output::count_by_protocol(&proxies);
        #[cfg(feature = "tui")]
        let saved_count = proxies.len();
        #[cfg(feature = "tui")]
        drop(tx.send(event::Event::App(event::AppEvent::Stage(
            event::Stage::Saving,
        ))));

        let location =
            output::save_proxies(Arc::clone(&config), proxies).await?;

        #[cfg(feature = "tui")]
        drop(tx.send(event::Event::App(event::AppEvent::Saved(saved_count))));

        let summary =
            output::summary(&config, &found, &saved, &location, stopped);
        tracing::info!("{summary}");
        summary
    };

    tracing::info!("Thank you for using proxy-scraper-checker!");

    #[cfg(feature = "tui")]
    drop(tx.send(event::Event::App(event::AppEvent::Done)));

    Ok(summary)
}

#[cfg(any(unix, windows))]
fn stop_on_signal<S, R>(
    signal_name: &'static str,
    signal: std::io::Result<S>,
    stop: impl FnOnce() + Send + 'static,
) where
    S: Future<Output = Option<R>> + Send + 'static,
    R: Send + 'static,
{
    let signal = match signal {
        Ok(signal) => signal,
        Err(e) => {
            tracing::warn!(
                "Failed to listen for {} signal: {}",
                signal_name,
                utils::pretty_error(&e.into())
            );
            return;
        }
    };
    // Not tied to the token: after ESC cancels the run, the TUI stays open
    // until it is told to quit.
    tokio::spawn(async move {
        if let Some(receiver) = signal.await {
            tracing::info!("Received {signal_name} signal, exiting...");
            stop();
            // With nothing listening, Windows would let a second Ctrl-C kill
            // the process, which by then is saving what it found.
            std::future::pending::<()>().await;
            drop(receiver);
        }
    });
}

#[cfg(any(unix, windows))]
fn watch_signals(
    token: &tokio_util::sync::CancellationToken,
    #[cfg(feature = "tui")] tx: &tokio::sync::mpsc::UnboundedSender<
        event::Event,
    >,
) {
    let token = token.clone();
    #[cfg(feature = "tui")]
    let tx = tx.clone();
    let stop = move || {
        token.cancel();
        #[cfg(feature = "tui")]
        drop(tx.send(event::Event::App(event::AppEvent::Quit)));
    };

    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        // Listening would undo nohup, which ignores SIGHUP to keep a run going
        // and takes it off the terminal to do so.
        let on_terminal = std::io::stdin().is_terminal()
            || std::io::stdout().is_terminal()
            || std::io::stderr().is_terminal();
        let signals = [
            ("SIGINT", SignalKind::interrupt(), true),
            ("SIGTERM", SignalKind::terminate(), true),
            ("SIGHUP", SignalKind::hangup(), on_terminal),
        ];
        for (signal_name, kind, wanted) in signals {
            if wanted {
                stop_on_signal(
                    signal_name,
                    signal(kind).map(|mut signal| async move {
                        signal.recv().await.map(move |()| signal)
                    }),
                    stop.clone(),
                );
            }
        }
    }

    #[cfg(windows)]
    {
        use tokio::signal::windows::{ctrl_c, ctrl_close};

        stop_on_signal(
            "Ctrl-C",
            ctrl_c().map(|mut signal| async move {
                signal.recv().await.map(move |()| signal)
            }),
            stop.clone(),
        );
        stop_on_signal(
            "console close",
            ctrl_close().map(|mut signal| async move {
                signal.recv().await.map(move |()| signal)
            }),
            stop,
        );
    }
}

// Neither side may drop the other mid-way: a run that fails closes the
// terminal, and a terminal that fails stops the run, which still saves what it
// found. The TUI gets a task of its own so that a panic in it fails the same
// way.
#[cfg(feature = "tui")]
async fn join_run_and_tui(
    run: impl Future<Output = crate::Result<compact_str::CompactString>>,
    tui: impl Future<Output = crate::Result<()>> + Send + 'static,
    token: tokio_util::sync::CancellationToken,
    tx: tokio::sync::mpsc::UnboundedSender<event::Event>,
) -> (crate::Result<compact_str::CompactString>, crate::Result<()>) {
    let tui = tokio::spawn(tui);
    tokio::join!(
        async move {
            let result = run.await;
            if result.is_err() {
                drop(tx.send(event::Event::App(event::AppEvent::Quit)));
            }
            result
        },
        async move {
            let result =
                tui.await.map_err(Into::into).and_then(|result| result);
            if result.is_err() {
                token.cancel();
            }
            result
        }
    )
}

#[cfg(feature = "tui")]
async fn run_with_tui(
    raw_config: raw_config::RawConfig,
    logging_filter: tracing_subscriber::filter::Targets,
) -> crate::Result<()> {
    tui_logger::init_logger(tui_logger::LevelFilter::Debug)?;
    tracing_subscriber::registry()
        .with(logging_filter)
        .with(tui_logger::TuiTracingSubscriberLayer)
        .init();

    let config = Arc::new(config::Config::from_raw_config(raw_config).await?);

    let terminal = ratatui::try_init()?;
    let terminal_guard = tui::app::RatatuiRestoreGuard;

    let token = tokio_util::sync::CancellationToken::new();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    #[cfg(any(unix, windows))]
    watch_signals(&token, &tx);

    let run = main_task(Arc::clone(&config), token.clone(), tx.clone());
    let quit = tx.clone();
    let tui_token = token.clone();
    let tui = async move {
        let result = tui::app::run(terminal, config, tui_token, tx, rx).await;
        drop(terminal_guard);
        result
    };

    let (summary, tui_result) = join_run_and_tui(run, tui, token, quit).await;

    match summary {
        Ok(summary) => {
            // The alternate screen takes everything with it when it closes, so
            // this is the only part of the run left in the scrollback.
            drop(writeln!(std::io::stdout(), "{summary}"));
            tui_result
        }
        Err(e) => utils::keep_both_errors(Err(e), tui_result),
    }
}

async fn run_without_tui(
    raw_config: raw_config::RawConfig,
    logging_filter: tracing_subscriber::filter::Targets,
) -> crate::Result<()> {
    // Colors are for people: escape codes only litter log files.
    let colored = std::io::stdout().is_terminal()
        && std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty());
    tracing_subscriber::registry()
        .with(logging_filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(colored)
                .with_target(false),
        )
        .init();

    let config = Arc::new(config::Config::from_raw_config(raw_config).await?);

    let token = tokio_util::sync::CancellationToken::new();

    // The events have no reader here, so sending them just fails.
    #[cfg(feature = "tui")]
    let tx = tokio::sync::mpsc::unbounded_channel().0;

    #[cfg(any(unix, windows))]
    watch_signals(
        &token,
        #[cfg(feature = "tui")]
        &tx,
    );

    main_task(
        config,
        token,
        #[cfg(feature = "tui")]
        tx,
    )
    .await?;

    Ok(())
}

#[cfg_attr(
    feature = "tokio-multi-thread",
    tokio::main(flavor = "multi_thread")
)]
#[cfg_attr(
    not(feature = "tokio-multi-thread"),
    tokio::main(flavor = "current_thread")
)]
async fn main() -> crate::Result<()> {
    #[cfg(feature = "dhat")]
    let _profiler = dhat::Profiler::new_heap();

    color_eyre::install()?;

    // Only parsed here: building the Config can log, which needs the subscriber
    // the debug flag configures.
    let raw_config =
        raw_config::read_config(&raw_config::get_config_path()).await?;
    let logging_filter = create_logging_filter(raw_config.debug);

    // Under cron or with the output redirected there is nothing to draw on.
    #[cfg(feature = "tui")]
    if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        return run_with_tui(raw_config, logging_filter).await;
    }

    run_without_tui(raw_config, logging_filter).await
}

#[cfg(test)]
#[cfg(feature = "tui")]
mod tests {
    use std::{
        sync::{Arc, atomic::Ordering},
        time::Duration,
    };

    use color_eyre::eyre::eyre;
    use compact_str::CompactString;
    use tokio::sync::mpsc::unbounded_channel;
    use tokio_util::sync::CancellationToken;

    use crate::{
        config::tests::{temp_directory, test_config},
        event::{AppEvent, Event},
        http::tests::serve,
        ipdb::{self, tests::cache},
        join_run_and_tui, main_task,
        raw_config::SourceConfig,
    };

    // A side that never tells the other would hang the tests rather than fail.
    async fn within_a_test<T>(future: impl Future<Output = T>) -> T {
        tokio::time::timeout(Duration::from_secs(5), future).await.unwrap()
    }

    #[tokio::test]
    async fn a_failed_terminal_stops_the_run_and_keeps_its_result() {
        let token = CancellationToken::new();
        let stopped = token.clone();
        let run = async move {
            stopped.cancelled().await;
            Ok(CompactString::const_new("saved"))
        };
        let tui = async { Err(eyre!("the terminal is gone")) };

        let (summary, tui_result) = within_a_test(join_run_and_tui(
            run,
            tui,
            token,
            unbounded_channel().0,
        ))
        .await;

        assert_eq!(summary.unwrap(), "saved");
        drop(tui_result.unwrap_err());
    }

    #[tokio::test]
    async fn a_panic_in_the_terminal_stops_the_run_and_keeps_its_result() {
        let token = CancellationToken::new();
        let stopped = token.clone();
        let run = async move {
            stopped.cancelled().await;
            Ok(CompactString::const_new("saved"))
        };

        let tui = async {
            std::hint::black_box(None::<()>).unwrap();
            Ok(())
        };

        let (summary, tui_result) = within_a_test(join_run_and_tui(
            run,
            tui,
            token,
            unbounded_channel().0,
        ))
        .await;

        assert_eq!(summary.unwrap(), "saved");
        drop(tui_result.unwrap_err());
    }

    #[tokio::test]
    async fn a_failed_run_closes_the_terminal() {
        let (tx, mut rx) = unbounded_channel();
        let run = async { Err(eyre!("the save failed")) };
        let tui = async move {
            let event = rx.recv().await;
            assert!(matches!(event, Some(Event::App(AppEvent::Quit))));
            Ok(())
        };

        let (summary, tui_result) = within_a_test(join_run_and_tui(
            run,
            tui,
            CancellationToken::new(),
            tx,
        ))
        .await;

        drop(summary.unwrap_err());
        tui_result.unwrap();
    }

    // Returns the proxy that was saved, as it appears in the JSON output.
    async fn run_with_failing_downloads() -> serde_json::Value {
        const NO_TUNNEL: &str = "HTTP/1.1 502 Bad Gateway\r\nContent-Length: \
                                 0\r\nConnection: close\r\n\r\n";
        const EXIT_IP: &str = "HTTP/1.1 200 OK\r\nContent-Length: \
                               9\r\nConnection: close\r\n\r\n192.0.2.1";

        let directory = temp_directory("failing-downloads");
        let source = directory.join("proxies.txt");
        let (working_proxy, _) = serve(vec![EXIT_IP]).await;
        std::fs::write(&source, working_proxy.to_string()).unwrap();

        // Each database gets three tries. The source is a file and the check
        // goes through the proxy under test, so only the downloads use this.
        let (download_proxy, tries) = serve(vec![NO_TUNNEL; 6]).await;

        let mut config = test_config(|raw| {
            raw.checking.check_url =
                Some("http://127.0.0.1:9/".parse().unwrap());
            raw.output.json.enabled = true;
            raw.output.json.include_asn = true;
            raw.output.json.include_geolocation = true;
            raw.scraping.proxy =
                Some(format!("http://{download_proxy}").parse().unwrap());
            raw.scraping.http.enabled = true;
            raw.scraping.http.urls =
                vec![SourceConfig::Simple(source.to_str().unwrap().into())];
            raw.scraping.socks4.enabled = false;
            raw.scraping.socks5.enabled = false;
        })
        .await;
        config.output.path.clone_from(&directory);

        let (tx, mut rx) = unbounded_channel();
        let summary = main_task(Arc::new(config), CancellationToken::new(), tx)
            .await
            .unwrap();

        assert!(summary.starts_with("Saved 1 of 1 proxy"), "{summary}");
        assert_eq!(tries.load(Ordering::Relaxed), 6);

        let mut finished = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let Event::App(AppEvent::IpDbFinished(db_type, ok)) = event {
                finished.push((db_type.name(), ok));
            }
        }
        finished.sort_unstable();
        assert_eq!(finished, [("ASN", false), ("geolocation", false)]);

        assert_eq!(
            std::fs::read_to_string(directory.join("proxies/all.txt")).unwrap(),
            format!("http://{working_proxy}")
        );
        let read_json = |name: &str| -> serde_json::Value {
            let text = std::fs::read_to_string(directory.join(name)).unwrap();
            serde_json::from_str(&text).unwrap()
        };
        let json = read_json("proxies.json");
        assert_eq!(json, read_json("proxies_pretty.json"));

        std::fs::remove_dir_all(directory).unwrap();
        json.pointer("/0").unwrap().clone()
    }

    // A single test, since a test process has only the one cache.
    #[tokio::test]
    async fn a_failed_download_falls_back_on_the_cached_ip_databases() {
        let proxy = run_with_failing_downloads().await;
        assert_eq!(proxy.pointer("/exit_ip").unwrap(), "192.0.2.1");
        assert!(proxy.pointer("/asn").unwrap().is_null());
        assert!(proxy.pointer("/geolocation").unwrap().is_null());

        let cached = cache::database(7);
        let paths = [
            cache::database_path(ipdb::DbType::Asn).await,
            cache::database_path(ipdb::DbType::Geo).await,
        ];
        for path in &paths {
            std::fs::write(path, &cached).unwrap();
        }

        let proxy = run_with_failing_downloads().await;
        assert_eq!(proxy.pointer("/exit_ip").unwrap(), "192.0.2.1");
        assert_eq!(
            proxy.pointer("/asn/autonomous_system_number").unwrap(),
            7_u32
        );
        assert!(proxy.pointer("/geolocation").unwrap().is_object());
        for path in &paths {
            assert_eq!(std::fs::read(path).unwrap(), cached);
        }

        cache::remove().await;
    }
}
