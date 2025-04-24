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
    clippy::allow_attributes,
    clippy::arbitrary_source_item_ordering,
    clippy::blanket_clippy_restriction_lints,
    clippy::default_numeric_fallback,
    clippy::else_if_without_else,
    clippy::float_arithmetic,
    clippy::implicit_return,
    clippy::iter_over_hash_type,
    clippy::min_ident_chars,
    clippy::missing_docs_in_private_items,
    clippy::mod_module_files,
    clippy::multiple_crate_versions,
    clippy::pattern_type_mismatch,
    clippy::pub_with_shorthand,
    clippy::too_many_lines,
    clippy::question_mark_used,
    clippy::shadow_reuse,
    clippy::shadow_unrelated,
    clippy::single_call_fn,
    clippy::single_char_lifetime_names,
    clippy::std_instead_of_alloc,
    clippy::std_instead_of_core,
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

use ui::UI as _;

pub const APP_DIRECTORY_NAME: &str = "proxy_scraper_checker";
pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
                              AppleWebKit/537.36 (KHTML, like Gecko) \
                              Chrome/135.0.0.0 Safari/537.36";
const CONFIG_ENV_VAR: &str = "PROXY_SCRAPER_CHECKER_CONFIG";

fn get_config_path() -> String {
    std::env::var(CONFIG_ENV_VAR).unwrap_or_else(|_| "config.toml".to_owned())
}

fn create_reqwest_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(tokio::time::Duration::from_secs(60))
        .connect_timeout(tokio::time::Duration::from_secs(5))
        .use_rustls_tls()
        .build()
}

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::eyre::Context::wrap_err(
        color_eyre::install(),
        "failed to install color_eyre hooks",
    )?;
    let ui_impl = ui::UIImpl::new()?;

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let ui_task = tokio::task::spawn(ui_impl.run(tx.clone(), rx));

    let http_client = color_eyre::eyre::Context::wrap_err(
        create_reqwest_client(),
        "failed to create reqwest HTTP client",
    )?;

    let raw_config_path = get_config_path();
    let raw_config = color_eyre::eyre::Context::wrap_err_with(
        raw_config::read_config(&raw_config_path).await,
        move || format!("failed to read {raw_config_path}"),
    )?;

    let config = std::sync::Arc::new(color_eyre::eyre::Context::wrap_err(
        config::Config::from_raw_config(raw_config, http_client.clone()).await,
        "failed to create Config from RawConfig",
    )?);

    if config.debug {
        ui::UIImpl::set_log_level(log::LevelFilter::Debug);
    }

    let maybe_geodb_task = config.enable_geolocation.then(|| {
        let http_client = http_client.clone();
        let tx = tx.clone();
        tokio::spawn(
            async move { geodb::download_geodb(http_client, tx).await },
        )
    });

    let mut storage = scraper::scrape_all(
        std::sync::Arc::clone(&config),
        http_client.clone(),
        tx.clone(),
    )
    .await?;

    drop(http_client);

    if let Some(geodb_task) = maybe_geodb_task {
        color_eyre::eyre::Context::wrap_err(
            color_eyre::eyre::Context::wrap_err(
                geodb_task.await,
                "failed to join GeoDB download task",
            )?,
            "failed to download GeoDB",
        )?;
    }

    if !config.check_website.is_empty() {
        storage = color_eyre::eyre::Context::wrap_err(
            checker::check_all(
                std::sync::Arc::clone(&config),
                storage,
                tx.clone(),
            )
            .await,
            "failed to check proxies",
        )?;
    }

    output::save_proxies(config, storage).await?;

    log::info!("Thank you for using proxy-scraper-checker!");
    tx.send(event::Event::App(event::AppEvent::Done))?;
    drop(tx);
    ui_task.await??;
    Ok(())
}
