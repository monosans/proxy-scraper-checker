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
    clippy::blanket_clippy_restriction_lints,
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
mod event;
mod fs;
mod geodb;
mod output;
mod parsers;
mod proxy;
mod raw_config;
mod scraper;
mod storage;
mod ui;
mod utils;

use std::sync::Arc;

use color_eyre::eyre::WrapErr as _;
use ui::UI as _;

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
    let ui_impl = ui::UIImpl::new()?;

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let ui_task = tokio::task::spawn(ui_impl.run(tx.clone(), rx));

    let http_client = create_reqwest_client()
        .wrap_err("failed to create reqwest HTTP client")?;

    let raw_config_path = raw_config::get_config_path();
    let raw_config = raw_config::read_config(&raw_config_path)
        .await
        .wrap_err_with(move || format!("failed to read {raw_config_path}"))?;

    let config = Arc::new(
        config::Config::from_raw_config(raw_config, http_client.clone())
            .await
            .wrap_err("failed to create Config from RawConfig")?,
    );

    if config.debug {
        ui::UIImpl::set_log_level(log::LevelFilter::Debug);
    }

    let maybe_geodb_task = config.enable_geolocation.then(|| {
        let http_client = http_client.clone();
        #[cfg(feature = "tui")]
        let tx = tx.clone();
        tokio::spawn(async move {
            geodb::download_geodb(
                http_client,
                #[cfg(feature = "tui")]
                tx,
            )
            .await
        })
    });

    let mut storage = scraper::scrape_all(
        Arc::clone(&config),
        http_client.clone(),
        #[cfg(feature = "tui")]
        tx.clone(),
    )
    .await?;

    drop(http_client);

    if let Some(geodb_task) = maybe_geodb_task {
        geodb_task
            .await
            .wrap_err("failed to join GeoDB download task")?
            .wrap_err("failed to download GeoDB")?;
    }

    if !config.check_website.is_empty() {
        storage = checker::check_all(
            Arc::clone(&config),
            storage,
            #[cfg(feature = "tui")]
            tx.clone(),
        )
        .await
        .wrap_err("failed to check proxies")?;
    }

    output::save_proxies(config, storage).await?;

    log::info!("Thank you for using proxy-scraper-checker!");
    #[cfg(feature = "tui")]
    tx.send(event::Event::App(event::AppEvent::Done))?;
    drop(tx);
    ui_task.await?
}
