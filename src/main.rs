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
mod fs;
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
use std::sync::Arc;

use color_eyre::eyre::WrapErr as _;
use tracing_subscriber::{
    layer::SubscriberExt as _, util::SubscriberInitExt as _,
};

#[cfg(all(
    feature = "mimalloc",
    any(target_arch = "aarch64", target_arch = "x86_64"),
    any(target_os = "linux", target_os = "macos", target_os = "windows"),
))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

type Error = color_eyre::Report;
type Result<T> = color_eyre::Result<T>;

fn create_logging_filter(
    config: &config::Config,
) -> tracing_subscriber::filter::Targets {
    let base = tracing_subscriber::filter::Targets::new()
        .with_default(tracing::level_filters::LevelFilter::INFO)
        .with_target(
            "hickory_proto::udp::udp_client_stream",
            tracing::level_filters::LevelFilter::ERROR,
        )
        .with_target(
            // TODO: remove for hickory_proto >= 0.25.0
            "hickory_proto::xfer::dns_exchange",
            tracing::level_filters::LevelFilter::ERROR,
        );

    if config.debug {
        base.with_target(
            "proxy_scraper_checker::checker",
            tracing::level_filters::LevelFilter::DEBUG,
        )
    } else {
        base
    }
}

async fn download_output_dependencies(
    config: &config::Config,
    http_client: reqwest::Client,
    token: tokio_util::sync::CancellationToken,
    #[cfg(feature = "tui")] tx: tokio::sync::mpsc::UnboundedSender<
        event::Event,
    >,
) -> crate::Result<()> {
    let mut output_dependencies_tasks = tokio::task::JoinSet::new();

    if config.asn_enabled() {
        let http_client = http_client.clone();
        let token = token.clone();
        #[cfg(feature = "tui")]
        let tx = tx.clone();

        output_dependencies_tasks.spawn(async move {
            tokio::select! {
                biased;
                res = ipdb::DbType::Asn.download(
                    http_client,
                    #[cfg(feature = "tui")]
                    tx,
                ) => res,
                () = token.cancelled() => Ok(()),
            }
        });
    }

    if config.geolocation_enabled() {
        output_dependencies_tasks.spawn(async move {
            tokio::select! {
                biased;
                res = ipdb::DbType::Geo.download(
                    http_client,
                    #[cfg(feature = "tui")]
                    tx,
                ) => res,
                () = token.cancelled() => Ok(()),
            }
        });
    }

    while let Some(task) = output_dependencies_tasks.join_next().await {
        task.wrap_err("output dependencies task panicked or was cancelled")??;
    }
    Ok(())
}

async fn main_task(
    config: Arc<config::Config>,
    token: tokio_util::sync::CancellationToken,
    #[cfg(feature = "tui")] tx: tokio::sync::mpsc::UnboundedSender<
        event::Event,
    >,
) -> crate::Result<()> {
    let http_client = http::create_reqwest_client(&config)
        .wrap_err("failed to create reqwest HTTP client")?;

    let ((), mut proxies) = tokio::try_join!(
        download_output_dependencies(
            &config,
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
    )?;

    proxies = checker::check_all(
        Arc::clone(&config),
        proxies,
        token,
        #[cfg(feature = "tui")]
        tx.clone(),
    )
    .await?;

    output::save_proxies(config, proxies)
        .await
        .wrap_err("failed to save proxies")?;

    tracing::info!("Thank you for using proxy-scraper-checker!");

    #[cfg(feature = "tui")]
    drop(tx.send(event::Event::App(event::AppEvent::Done)));

    Ok(())
}

#[cfg(any(unix, windows))]
fn watch_signals(
    token: &tokio_util::sync::CancellationToken,
    #[cfg(feature = "tui")] tx: &tokio::sync::mpsc::UnboundedSender<
        event::Event,
    >,
) {
    #[cfg(unix)]
    let signals = [
        (
            "SIGINT",
            tokio::signal::unix::signal(
                tokio::signal::unix::SignalKind::interrupt(),
            ),
        ),
        (
            "SIGTERM",
            tokio::signal::unix::signal(
                tokio::signal::unix::SignalKind::terminate(),
            ),
        ),
    ];

    #[cfg(windows)]
    let signals = [("Ctrl-C", tokio::signal::windows::ctrl_c())];

    for (signal_name, stream) in signals {
        let mut stream = match stream {
            Ok(signal) => signal,
            Err(e) => {
                tracing::warn!(
                    "Failed to listen for {} signal: {}",
                    signal_name,
                    e
                );
                continue;
            }
        };
        let token = token.clone();
        #[cfg(feature = "tui")]
        let tx = tx.clone();
        tokio::spawn(async move {
            tokio::select! {
                biased;
                () = token.cancelled() => {},
                _ = stream.recv() => {
                    tracing::info!("Received {} signal, exiting...", signal_name);
                    token.cancel();
                    #[cfg(feature = "tui")]
                    drop(tx.send(event::Event::App(event::AppEvent::Quit)));
                },
            }
        });
    }
}

#[cfg(feature = "tui")]
async fn run_with_tui(
    config: Arc<config::Config>,
    logging_filter: tracing_subscriber::filter::Targets,
) -> crate::Result<()> {
    tui_logger::init_logger(tui_logger::LevelFilter::Debug)
        .wrap_err("failed to initialize tui_logger")?;
    tracing_subscriber::registry()
        .with(logging_filter)
        .with(tui_logger::TuiTracingSubscriberLayer)
        .init();

    let terminal =
        ratatui::try_init().wrap_err("failed to initialize ratatui")?;
    let terminal_guard = tui::RatatuiRestoreGuard;

    let token = tokio_util::sync::CancellationToken::new();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    #[cfg(any(unix, windows))]
    watch_signals(&token, &tx);

    tokio::try_join!(
        main_task(config, token.clone(), tx.clone()),
        async move {
            let result = tui::run(terminal, token, tx, rx).await;
            drop(terminal_guard);
            result
        }
    )?;

    Ok(())
}

#[cfg(not(feature = "tui"))]
async fn run_without_tui(
    config: Arc<config::Config>,
    logging_filter: tracing_subscriber::filter::Targets,
) -> crate::Result<()> {
    tracing_subscriber::registry()
        .with(logging_filter)
        .with(tracing_subscriber::fmt::layer())
        .init();

    let token = tokio_util::sync::CancellationToken::new();

    #[cfg(any(unix, windows))]
    watch_signals(&token);

    main_task(config, token).await
}

#[tokio::main]
async fn main() -> crate::Result<()> {
    color_eyre::install().wrap_err("failed to install color_eyre hooks")?;

    let config = config::load_config().await?;
    let logging_filter = create_logging_filter(&config);

    #[cfg(feature = "tui")]
    {
        run_with_tui(config, logging_filter).await
    }
    #[cfg(not(feature = "tui"))]
    {
        run_without_tui(config, logging_filter).await
    }
}
