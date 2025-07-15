#![warn(
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

fn create_reqwest_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(config::USER_AGENT)
        .timeout(tokio::time::Duration::from_secs(60))
        .connect_timeout(tokio::time::Duration::from_secs(5))
        .use_rustls_tls()
        .build()
}

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

fn spawn_ip_database_tasks(
    config: &Arc<config::Config>,
    http_client: &reqwest::Client,
    #[cfg(feature = "tui")] tx: &tokio::sync::mpsc::UnboundedSender<
        event::Event,
    >,
) -> tokio::task::JoinSet<color_eyre::Result<()>> {
    let mut output_dependencies_tasks = tokio::task::JoinSet::new();

    if config.asn_enabled() {
        let http_client = http_client.clone();
        #[cfg(feature = "tui")]
        let tx = tx.clone();

        output_dependencies_tasks.spawn(async move {
            ipdb::DbType::Asn
                .download(
                    http_client,
                    #[cfg(feature = "tui")]
                    tx,
                )
                .await
        });
    }

    if config.geolocation_enabled() {
        let http_client = http_client.clone();
        #[cfg(feature = "tui")]
        let tx = tx.clone();

        output_dependencies_tasks.spawn(async move {
            ipdb::DbType::Geo
                .download(
                    http_client,
                    #[cfg(feature = "tui")]
                    tx,
                )
                .await
        });
    }

    output_dependencies_tasks
}

async fn wait_for_ip_database_tasks(
    mut tasks: tokio::task::JoinSet<color_eyre::Result<()>>,
) -> color_eyre::Result<()> {
    while let Some(task) = tasks.join_next().await {
        task.wrap_err("output dependencies task panicked or was cancelled")??;
    }
    Ok(())
}

async fn process_proxies(
    config: Arc<config::Config>,
    proxies: std::collections::HashSet<crate::proxy::Proxy>,
    #[cfg(feature = "tui")] tx: &tokio::sync::mpsc::UnboundedSender<
        event::Event,
    >,
) -> color_eyre::Result<Vec<crate::proxy::Proxy>> {
    if config.checking.check_url.is_empty() {
        Ok(proxies.into_iter().collect())
    } else {
        #[cfg(not(feature = "tui"))]
        tracing::info!("Started checking {} proxies", proxies.len());

        checker::check_all(
            config,
            proxies,
            #[cfg(feature = "tui")]
            tx.clone(),
        )
        .await
        .wrap_err("failed to check proxies")
    }
}

async fn main_task(
    config: Arc<config::Config>,
    #[cfg(feature = "tui")] tx: tokio::sync::mpsc::UnboundedSender<
        event::Event,
    >,
) -> color_eyre::Result<()> {
    let http_client = create_reqwest_client()
        .wrap_err("failed to create reqwest HTTP client")?;

    let ip_db_tasks = spawn_ip_database_tasks(
        &config,
        &http_client,
        #[cfg(feature = "tui")]
        &tx,
    );

    let proxies = scraper::scrape_all(
        Arc::clone(&config),
        http_client.clone(),
        #[cfg(feature = "tui")]
        tx.clone(),
    )
    .await
    .wrap_err("failed to scrape proxies")?;

    drop(http_client);

    wait_for_ip_database_tasks(ip_db_tasks).await?;

    let proxies = process_proxies(
        Arc::clone(&config),
        proxies,
        #[cfg(feature = "tui")]
        &tx,
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

#[cfg(feature = "tui")]
async fn run_with_tui(
    config: Arc<config::Config>,
    logging_filter: tracing_subscriber::filter::Targets,
) -> color_eyre::Result<()> {
    tui_logger::init_logger(tui_logger::LevelFilter::Debug)
        .wrap_err("failed to initialize tui_logger")?;
    tracing_subscriber::registry()
        .with(logging_filter)
        .with(tui_logger::TuiTracingSubscriberLayer)
        .init();

    let terminal =
        ratatui::try_init().wrap_err("failed to initialize ratatui")?;
    let terminal_guard = tui::RatatuiRestoreGuard;

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let main_task = tokio::task::spawn(main_task(config, tx.clone()));
    let main_task_handle = main_task.abort_handle();

    tokio::try_join!(async move { main_task.await? }, async move {
        let result = tui::run(terminal, tx, rx).await;
        drop(terminal_guard);
        main_task_handle.abort();
        result
    })?;

    Ok(())
}

#[cfg(not(feature = "tui"))]
async fn run_without_tui(
    config: Arc<config::Config>,
    logging_filter: tracing_subscriber::filter::Targets,
) -> color_eyre::Result<()> {
    tracing_subscriber::registry()
        .with(logging_filter)
        .with(tracing_subscriber::fmt::layer())
        .init();

    main_task(config).await
}

async fn load_config() -> color_eyre::Result<Arc<config::Config>> {
    let raw_config_path = raw_config::get_config_path();
    let raw_config = raw_config::read_config(std::path::Path::new(
        &raw_config_path,
    ))
    .await
    .wrap_err_with(move || format!("failed to read {raw_config_path}"))?;

    let config = config::Config::from_raw_config(raw_config)
        .await
        .wrap_err("failed to create Config from RawConfig")?;

    Ok(Arc::new(config))
}

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install().wrap_err("failed to install color_eyre hooks")?;

    let config = load_config().await?;
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
