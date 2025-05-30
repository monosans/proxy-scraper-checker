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
#[cfg(not(feature = "tui"))]
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

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install().wrap_err("failed to install color_eyre hooks")?;

    let raw_config_path = raw_config::get_config_path();
    let raw_config = raw_config::read_config(std::path::Path::new(
        &raw_config_path,
    ))
    .await
    .wrap_err_with(move || format!("failed to read {raw_config_path}"))?;

    let config = Arc::new(
        config::Config::from_raw_config(raw_config)
            .await
            .wrap_err("failed to create Config from RawConfig")?,
    );

    let targets_filter = tracing_subscriber::filter::Targets::new()
        .with_default(tracing::level_filters::LevelFilter::INFO)
        // TODO: remove for hickory_proto >= 0.25.0
        .with_target(
            "hickory_proto::xfer::dns_exchange",
            tracing::level_filters::LevelFilter::ERROR,
        )
        .with_target(
            "proxy_scraper_checker",
            if config.debug {
                tracing::level_filters::LevelFilter::DEBUG
            } else {
                tracing::level_filters::LevelFilter::INFO
            },
        );

    #[cfg(feature = "tui")]
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    #[cfg(feature = "tui")]
    let tui_task =
        tokio::task::spawn(tui::Tui::new(targets_filter)?.run(tx.clone(), rx));

    #[cfg(not(feature = "tui"))]
    tracing_subscriber::registry()
        .with(targets_filter)
        .with(tracing_subscriber::fmt::layer())
        .init();

    let http_client = create_reqwest_client()
        .wrap_err("failed to create reqwest HTTP client")?;

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

    let proxies = scraper::scrape_all(
        Arc::clone(&config),
        http_client.clone(),
        #[cfg(feature = "tui")]
        tx.clone(),
    )
    .await
    .wrap_err("failed to scrape proxies")?;

    drop(http_client);

    while let Some(task) = output_dependencies_tasks.join_next().await {
        task.wrap_err("failed to join output dependencies task")??;
    }
    drop(output_dependencies_tasks);

    let proxies = if config.checking.check_url.is_empty() {
        proxies.into_iter().collect()
    } else {
        #[cfg(not(feature = "tui"))]
        tracing::info!("Started checking {} proxies", proxies.len());

        checker::check_all(
            Arc::clone(&config),
            proxies,
            #[cfg(feature = "tui")]
            tx.clone(),
        )
        .await
        .wrap_err("failed to check proxies")?
    };

    output::save_proxies(config, proxies)
        .await
        .wrap_err("failed to save proxies")?;

    tracing::info!("Thank you for using proxy-scraper-checker!");

    #[cfg(feature = "tui")]
    tx.send(event::Event::App(event::AppEvent::Done))?;

    #[cfg(feature = "tui")]
    drop(tx);

    #[cfg(feature = "tui")]
    tui_task.await??;

    Ok(())
}
