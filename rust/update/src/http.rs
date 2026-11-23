use crate::{
    invalid,
    package::{Release, LATEST_URL, LIMIT},
    Version,
};
use reqwest::{redirect::Policy, Client, Url};
use std::{
    io,
    time::{Duration, Instant},
};

pub fn fetch_github(
    uri: &str,
    maximum: usize,
    cancel: &dyn Fn() -> io::Result<()>,
) -> io::Result<Vec<u8>> {
    fetch_until(
        uri,
        maximum,
        cancel,
        Instant::now() + Duration::from_secs(180),
    )
}

fn fetch_until(
    uri: &str,
    maximum: usize,
    cancel: &dyn Fn() -> io::Result<()>,
    deadline: Instant,
) -> io::Result<Vec<u8>> {
    cancel()?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        tokio::select! {
            answer = fetch_async(uri, maximum, deadline) => answer,
            error = async {
                loop {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    if let Err(error) = cancel() { return error; }
                }
            } => Err(error)
        }
    })
}

async fn fetch_async(uri: &str, maximum: usize, deadline: Instant) -> io::Result<Vec<u8>> {
    let client = Client::builder()
        .no_proxy()
        .user_agent("Svyaz")
        .redirect(Policy::none())
        .build()
        .map_err(io::Error::other)?;
    let mut url = Url::parse(uri).map_err(invalid)?;
    for _ in 0..5 {
        if url.scheme() != "https"
            || url.port_or_known_default() != Some(443)
            || !url.username().is_empty()
            || url.password().is_some()
            || !matches!(
                url.host_str(),
                Some("api.github.com" | "github.com" | "release-assets.githubusercontent.com")
            )
        {
            return Err(invalid("Недопустимое перенаправление GitHub."));
        }
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "Превышено время загрузки."))?;
        let mut response = client
            .get(url.clone())
            .timeout(remaining)
            .send()
            .await
            .map_err(io::Error::other)?;
        if response.status().is_redirection() {
            let target = response
                .headers()
                .get("location")
                .and_then(|h| h.to_str().ok())
                .ok_or_else(|| invalid("Редирект без адреса."))?;
            url = url.join(target).map_err(invalid)?;
            continue;
        }
        response.error_for_status_ref().map_err(io::Error::other)?;
        if !response.status().is_success()
            || response
                .content_length()
                .is_some_and(|n| n > maximum as u64)
        {
            return Err(invalid("Недопустимый ответ GitHub."));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(io::Error::other)? {
            if bytes.len().saturating_add(chunk.len()) > maximum {
                return Err(invalid("Ответ GitHub слишком большой."));
            }
            bytes.extend_from_slice(&chunk);
        }
        return Ok(bytes);
    }
    Err(invalid("Слишком много перенаправлений GitHub."))
}

pub(crate) fn download(
    current: Version,
    cancel: &dyn Fn() -> io::Result<()>,
) -> io::Result<Option<(Release, Vec<u8>)>> {
    let deadline = Instant::now() + Duration::from_secs(180);
    let fetch = |url: &str, limit| fetch_until(url, limit, cancel, deadline);
    let release = Release::parse(&fetch(LATEST_URL, 2 * 1024 * 1024)?)?;
    if release.version <= current {
        return Ok(None);
    }
    let bytes = fetch(&release.url, LIMIT)?;
    Ok(Some((release, bytes)))
}
