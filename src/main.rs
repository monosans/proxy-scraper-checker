pub(crate) mod checker;
pub(crate) mod config;
pub(crate) mod event;
pub(crate) mod fs;
pub(crate) mod geodb;
pub(crate) mod output;
pub(crate) mod parsers;
pub(crate) mod proxy;
pub(crate) mod raw_config;
pub(crate) mod scraper;
pub(crate) mod storage;
pub(crate) mod tui;
pub(crate) mod utils;

pub(crate) const APP_DIRECTORY_NAME: &str = "proxy_scraper_checker";
pub(crate) const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; \
                                     x64) AppleWebKit/537.36 (KHTML, like \
                                     Gecko) Chrome/135.0.0.0 Safari/537.36";
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

async fn run(terminal: ratatui::DefaultTerminal) -> color_eyre::Result<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let ui_task = tokio::task::spawn(tui::run(terminal, tx.clone(), rx));

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

    let maybe_geodb_task = if config.enable_geolocation {
        let http_client = http_client.clone();
        let tx = tx.clone();
        Some(tokio::spawn(async move {
            geodb::download_geodb(http_client, tx).await
        }))
    } else {
        None
    };

    let mut storage =
        scraper::scrape_all(config.clone(), http_client.clone(), tx.clone())
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
            checker::check_all(config.clone(), storage, tx.clone()).await,
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

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::eyre::Context::wrap_err(
        color_eyre::install(),
        "failed to install color_eyre hooks",
    )?;
    color_eyre::eyre::Context::wrap_err(
        tui_logger::init_logger(log::LevelFilter::Trace),
        "failed to initialize logging",
    )?;
    tui_logger::set_default_level(log::LevelFilter::Info);
    let terminal = ratatui::init();
    let result = run(terminal).await;
    ratatui::restore();
    result
}
