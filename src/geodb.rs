use std::{io, path::PathBuf};

use color_eyre::eyre::WrapErr;
use futures::StreamExt;
use tokio::io::AsyncWriteExt;

use crate::{
    event::{AppEvent, Event},
    fs::get_cache_path,
    utils::is_docker,
};

const GEODB_URL: &str = "https://raw.githubusercontent.com/P3TERX/GeoLite.mmdb/download/GeoLite2-City.mmdb";

pub(crate) async fn get_geodb_path() -> color_eyre::Result<PathBuf> {
    let mut cache_path =
        get_cache_path().await.wrap_err("failed to get cache path")?;
    cache_path.push("geolocation_database.mmdb");
    Ok(cache_path)
}

async fn get_geodb_etag_path() -> color_eyre::Result<PathBuf> {
    let mut geodb_path =
        get_geodb_path().await.wrap_err("failed to get GeoDB path")?;
    geodb_path.set_extension("mmdb.etag");
    Ok(geodb_path)
}

async fn read_etag() -> color_eyre::Result<Option<reqwest::header::HeaderValue>>
{
    let etag_path = get_geodb_etag_path()
        .await
        .wrap_err("failed to get GeoDB ETag path")?;
    match tokio::fs::read_to_string(&etag_path).await {
        Ok(text) => Ok(text.parse().ok()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).wrap_err_with(move || {
            format!("failed to read file {} to string", etag_path.display())
        }),
    }
}

async fn remove_etag() -> color_eyre::Result<()> {
    let etag_path = get_geodb_etag_path()
        .await
        .wrap_err("failed to get GeoDB ETag path")?;
    match tokio::fs::remove_file(&etag_path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).wrap_err_with(move || {
            format!("failed to remove {}", etag_path.display())
        }),
    }
}

async fn save_etag(
    etag: reqwest::header::HeaderValue,
) -> color_eyre::Result<()> {
    let etag_file = get_geodb_etag_path()
        .await
        .wrap_err("failed to get GeoDB ETag path")?;
    tokio::fs::write(&etag_file, etag).await.wrap_err_with(move || {
        format!("failed to write to file {}", etag_file.display())
    })?;
    Ok(())
}

async fn save_geodb(
    response: reqwest::Response,
    tx: tokio::sync::mpsc::UnboundedSender<Event>,
) -> color_eyre::Result<()> {
    tx.send(Event::App(AppEvent::GeoDbTotal(response.content_length())))?;
    let geodb_file =
        get_geodb_path().await.wrap_err("failed to get GeoDB path")?;
    let mut file =
        tokio::fs::File::create(&geodb_file).await.wrap_err_with(|| {
            format!("failed to create file {}", geodb_file.display())
        })?;
    let mut stream = response.bytes_stream();
    while let Some(item) = stream.next().await {
        let chunk = item.wrap_err("failed to read GeoDB response chunk")?;
        file.write_all(&chunk).await.wrap_err_with(|| {
            format!("failed to write to file {}", geodb_file.display())
        })?;
        tx.send(Event::App(AppEvent::GeoDbDownloaded(chunk.len())))?;
    }
    Ok(())
}

pub(crate) async fn download_geodb(
    http_client: reqwest::Client,
    tx: tokio::sync::mpsc::UnboundedSender<Event>,
) -> color_eyre::Result<()> {
    let geodb_file =
        get_geodb_path().await.wrap_err("failed to get GeoDB path")?;

    let mut headers = reqwest::header::HeaderMap::new();
    if tokio::fs::metadata(&geodb_file).await.is_ok_and(|m| m.is_file()) {
        if let Some(etag) = read_etag().await.wrap_err("failed to read ETag")? {
            headers.insert(reqwest::header::IF_NONE_MATCH, etag);
        }
    }

    let response = http_client
        .get(GEODB_URL)
        .headers(headers)
        .send()
        .await
        .wrap_err("failed to send GeoDB download request")?
        .error_for_status()
        .wrap_err("got error HTTP status code when downloading GeoDB")?;

    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        log::info!(
            "Latest geolocation database is already cached at {}",
            geodb_file.display()
        );
        return Ok(());
    }

    let etag = response.headers().get(reqwest::header::ETAG).cloned();

    save_geodb(response, tx.clone()).await.wrap_err("failed to save GeoDB")?;

    if is_docker().await {
        log::info!(
            "Downloaded geolocation database to Docker volume ({} in \
             container)",
            geodb_file.display()
        );
    } else {
        log::info!(
            "Downloaded geolocation database to {}",
            geodb_file.display()
        );
    }

    if let Some(etag_value) = etag {
        save_etag(etag_value).await.wrap_err("failed to save GeoDB ETag")?;
    } else {
        remove_etag().await.wrap_err("failed to remove GeoDB ETag")?;
    }

    Ok(())
}
