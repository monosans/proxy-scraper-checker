//! Scrapes proxies from the configured sources, checks them concurrently and
//! saves the working ones.

#![deny(
    warnings,
    ambiguous_negative_literals,
    dead_code_pub_in_binary,
    deprecated_in_future,
    deprecated_safe,
    deref_into_dyn_supertrait,
    ffi_unwind_calls,
    future_incompatible,
    impl_trait_redundant_captures,
    keyword_idents,
    let_underscore,
    macro_use_extern_crate,
    meta_variable_misuse,
    missing_copy_implementations,
    missing_docs,
    non_ascii_idents,
    nonstandard_style,
    raw_borrows_via_references,
    redundant_imports,
    redundant_lifetimes,
    refining_impl_trait,
    rust_2018_compatibility,
    rust_2018_idioms,
    rust_2021_compatibility,
    rust_2024_compatibility,
    single_use_lifetimes,
    trivial_casts,
    trivial_numeric_casts,
    unit_bindings,
    unsafe_code,
    unstable_features,
    unused,
    unused_crate_dependencies,
    unused_import_braces,
    unused_lifetimes,
    unused_qualifications,
    variant_size_differences,
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

// Without the TUI, only its features are used: they leave the trace logs
// of the dependencies out of the build.
#[cfg(not(feature = "tui"))]
use log as _;
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
        utils::log_panic("IP database task", res);
    }
}

async fn main_task(
    config: Arc<config::Config>,
    token: tokio_util::sync::CancellationToken,
    #[cfg(feature = "tui")] tx: tokio::sync::mpsc::UnboundedSender<
        event::Event,
    >,
) -> Result<compact_str::CompactString> {
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
        drop(token);
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
        if let Some(_receiver) = signal.await {
            tracing::info!("Received {signal_name} signal, exiting...");
            stop();
            // With nothing listening, Windows would let a second Ctrl-C kill
            // the process, which by then is saving what it found.
            std::future::pending::<()>().await;
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
    run: impl Future<Output = Result<compact_str::CompactString>>,
    tui: impl Future<Output = Result<()>> + Send + 'static,
    token: tokio_util::sync::CancellationToken,
    tx: tokio::sync::mpsc::UnboundedSender<event::Event>,
) -> (Result<compact_str::CompactString>, Result<()>) {
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
) -> Result<()> {
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
        let input = crossterm::event::EventStream::new();
        let result =
            tui::app::run(terminal, input, config, tui_token, tx, rx).await;
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
) -> Result<()> {
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
async fn main() -> Result<()> {
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
    use std::{io, pin::Pin, sync::Arc, time::Duration};

    use color_eyre::eyre::eyre;
    use compact_str::CompactString;
    use tokio::sync::{mpsc::unbounded_channel, oneshot};
    use tokio_util::sync::CancellationToken;
    use tracing::Level;

    use crate::{
        config::tests::{temp_directory, test_config},
        create_logging_filter, download_ipdb,
        event::{AppEvent, Event},
        http::{self, tests::serve},
        ipdb::{self, tests::cache},
        join_run_and_tui, main_task,
        raw_config::SourceConfig,
        stop_on_signal,
        utils::tests::capture_logs,
    };

    const NO_TUNNEL: &str = "HTTP/1.1 502 Bad Gateway\r\nContent-Length: \
                             0\r\nConnection: close\r\n\r\n";

    #[test]
    fn debug_mode_shows_the_debug_messages_of_this_program_only() {
        let ours = "proxy_scraper_checker::checker";
        let theirs = "hyper_util::client::legacy";
        let verifier = "rustls_platform_verifier::verification";

        let normal = create_logging_filter(false);
        assert!(normal.would_enable(ours, &Level::INFO));
        assert!(!normal.would_enable(ours, &Level::DEBUG));
        assert!(normal.would_enable(theirs, &Level::INFO));

        let debug = create_logging_filter(true);
        assert!(debug.would_enable(ours, &Level::DEBUG));
        assert!(!debug.would_enable(theirs, &Level::DEBUG));

        for filter in [normal, debug] {
            assert!(!filter.would_enable(verifier, &Level::ERROR));
        }
    }

    // The same types for every test, so that they all run the same code.
    type Signal = std::future::Ready<Option<()>>;
    type Stop = Box<dyn FnOnce() + Send>;
    type Run = Pin<Box<dyn Future<Output = crate::Result<CompactString>>>>;
    type Tui = Pin<Box<dyn Future<Output = crate::Result<()>> + Send>>;

    fn stop_with(stopped: oneshot::Sender<()>) -> Stop {
        Box::new(move || stopped.send(()).unwrap())
    }

    #[tokio::test]
    async fn a_signal_that_cannot_be_listened_for_is_only_a_warning() {
        let logs = capture_logs();
        let (stopped, not_stopped) = oneshot::channel();
        let signal: io::Result<Signal> = Err(io::Error::other("not supported"));

        stop_on_signal("TEST", signal, stop_with(stopped));

        not_stopped.await.unwrap_err();
        let text = logs.text();
        assert!(
            text.contains("Failed to listen for TEST signal: not supported"),
            "{text}"
        );
    }

    #[tokio::test]
    async fn a_signal_stops_the_run() {
        let logs = capture_logs();
        let (stopped, stop_seen) = oneshot::channel();

        let signal: Signal = std::future::ready(Some(()));

        stop_on_signal("TEST", Ok(signal), stop_with(stopped));

        within_a_test(stop_seen).await.unwrap();
        let text = logs.text();
        assert!(text.contains("Received TEST signal, exiting..."), "{text}");
    }

    #[tokio::test]
    async fn signals_that_stop_coming_stop_nothing() {
        let (stopped, not_stopped) = oneshot::channel();
        let signal: Signal = std::future::ready(None);

        stop_on_signal("TEST", Ok(signal), stop_with(stopped));

        // The listener ends, and the stop goes with it unused.
        within_a_test(not_stopped).await.unwrap_err();
    }

    fn finished_downloads(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<Event>,
    ) -> Vec<(&'static str, bool)> {
        let mut finished = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let Event::App(AppEvent::IpDbFinished(db_type, ok)) = event {
                finished.push((db_type.name(), ok));
            }
        }
        finished.sort_unstable();
        finished
    }

    #[tokio::test]
    async fn only_the_enabled_databases_are_downloaded() {
        let cache = cache::take().await;
        let logs = capture_logs();
        // Three tries for the one database.
        let (download_proxy, tries) = serve(vec![NO_TUNNEL; 3]).await;
        let config = Arc::new(
            test_config(|raw| {
                raw.output.json.include_asn = false;
                raw.scraping.proxy =
                    Some(format!("http://{download_proxy}").parse().unwrap());
            })
            .await,
        );
        let client = http::create_reqwest_client(&config).await.unwrap();

        let (tx, mut rx) = unbounded_channel();
        download_ipdb(
            Arc::clone(&config),
            client.clone(),
            CancellationToken::new(),
            tx,
        )
        .await;

        assert_eq!(finished_downloads(&mut rx), [("geolocation", false)]);
        assert_eq!(tries.lock().len(), 3);
        let text = logs.text();
        assert!(
            text.contains("failed to download geolocation database: "),
            "{text}"
        );

        // Nothing is left to tell about a stopped run.
        let token = CancellationToken::new();
        token.cancel();
        let (tx, mut rx) = unbounded_channel();
        download_ipdb(config, client, token, tx).await;
        assert_eq!(finished_downloads(&mut rx), []);
        assert_eq!(tries.lock().len(), 3);
        drop(cache);
    }

    #[tokio::test]
    async fn a_run_that_finds_nothing_leaves_the_previous_results() {
        let directory = temp_directory("nothing-found");
        let source = directory.join("empty.txt");
        std::fs::write(&source, "no proxies here").unwrap();
        let mut config = test_config(|raw| {
            raw.output.json.enabled = false;
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

        assert_eq!(
            summary,
            "No proxies to save - previous results left untouched"
        );
        let events: Vec<_> =
            std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(matches!(events.last(), Some(Event::App(AppEvent::Done))));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Event::App(AppEvent::Saved(_))))
        );
        assert!(!directory.join("proxies").exists());
        std::fs::remove_dir_all(directory).unwrap();
    }

    // A side that never tells the other would hang the tests rather than fail.
    async fn within_a_test<T>(future: impl Future<Output = T>) -> T {
        tokio::time::timeout(Duration::from_secs(5), future).await.unwrap()
    }

    #[tokio::test]
    async fn a_failed_terminal_stops_the_run_and_keeps_its_result() {
        let token = CancellationToken::new();
        let stopped = token.clone();
        let run: Run = Box::pin(async move {
            stopped.cancelled().await;
            Ok(CompactString::const_new("saved"))
        });
        let tui: Tui = Box::pin(async { Err(eyre!("the terminal is gone")) });
        let tx = unbounded_channel().0;

        let (summary, tui_result) =
            within_a_test(join_run_and_tui(run, tui, token, tx)).await;

        assert_eq!(summary.unwrap(), "saved");
        drop(tui_result.unwrap_err());
    }

    #[tokio::test]
    async fn a_panic_in_the_terminal_stops_the_run_and_keeps_its_result() {
        let token = CancellationToken::new();
        let stopped = token.clone();
        let run: Run = Box::pin(async move {
            stopped.cancelled().await;
            Ok(CompactString::const_new("saved"))
        });

        let tui: Tui = Box::pin(async {
            std::hint::black_box(None::<()>).unwrap();
            Ok(())
        });
        let tx = unbounded_channel().0;

        let (summary, tui_result) =
            within_a_test(join_run_and_tui(run, tui, token, tx)).await;

        assert_eq!(summary.unwrap(), "saved");
        drop(tui_result.unwrap_err());
    }

    #[tokio::test]
    async fn a_failed_run_closes_the_terminal() {
        let (tx, mut rx) = unbounded_channel();
        let run: Run = Box::pin(async { Err(eyre!("the save failed")) });
        let tui: Tui = Box::pin(async move {
            let event = rx.recv().await;
            assert!(matches!(event, Some(Event::App(AppEvent::Quit))));
            Ok(())
        });

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
        const EXIT_IP: &str = "HTTP/1.1 200 OK\r\nContent-Length: \
                               9\r\nConnection: close\r\n\r\n192.0.2.1";

        let directory = temp_directory("failing-downloads");
        let (working_proxy, _) = serve(vec![EXIT_IP]).await;
        let source = directory.join("proxies.txt");
        std::fs::write(&source, working_proxy.to_string()).unwrap();

        // Each database gets three tries. The source is a file and the check
        // goes through the proxy under test, so only the downloads use this.
        let (download_proxy, tries) = serve(vec![NO_TUNNEL; 6]).await;

        let mut config = test_config(move |raw| {
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
        drop(summary);
        assert_eq!(tries.lock().len(), 6);
        drop(tries);

        assert_eq!(
            finished_downloads(&mut rx),
            [("ASN", false), ("geolocation", false)]
        );
        drop(rx);

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

    #[tokio::test]
    async fn a_failed_download_falls_back_on_the_cached_ip_databases() {
        let cache = cache::take().await;
        let proxy = run_with_failing_downloads().await;
        assert_eq!(proxy.pointer("/exit_ip").unwrap(), "192.0.2.1");
        assert!(proxy.pointer("/asn").unwrap().is_null());
        assert!(proxy.pointer("/geolocation").unwrap().is_null());
        drop(proxy);

        let paths = [
            cache::database_path(ipdb::DbType::Asn).await,
            cache::database_path(ipdb::DbType::Geo).await,
        ];
        let cached = cache::database(7);
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
        drop(proxy);
        for path in paths {
            assert_eq!(std::fs::read(path).unwrap(), cached);
        }
        drop(cached);
        drop(cache);
    }
}
