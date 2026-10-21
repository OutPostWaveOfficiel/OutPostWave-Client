//! `owfile://` protocol: lets the chat show images and play videos and audio from files that
//! are complete on disk (downloads, or our own sent files), without giving the webview access to
//! the file system.
//!
//! URL: `owfile://localhost/<file id hex>` (`http://owfile.localhost/<file id hex>` on Windows and
//! Android). Only media types listed here are served (never SVG or HTML: no script can run), with
//! `Range` support so that videos can seek.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use tauri::http::{Request, Response, StatusCode, header};
use tauri::{AppHandle, Manager};

use crate::AppState;

/// Largest response body (a range request gets at most this much).
const MAX_BODY: u64 = 8 * 1024 * 1024;
/// Without a `Range` header, files up to this size are sent whole.
const MAX_WHOLE: u64 = 64 * 1024 * 1024;

/// Kind of media and MIME type for a file name, if it can be shown in the chat.
#[must_use]
pub fn media_type(name: &str) -> Option<(&'static str, &'static str)> {
    let ext = Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => ("image", "image/png"),
        "jpg" | "jpeg" | "jfif" => ("image", "image/jpeg"),
        "gif" => ("image", "image/gif"),
        "webp" => ("image", "image/webp"),
        "avif" => ("image", "image/avif"),
        "bmp" => ("image", "image/bmp"),
        "mp4" | "m4v" | "mov" => ("video", "video/mp4"),
        "webm" | "mkv" => ("video", "video/webm"),
        "ogv" => ("video", "video/ogg"),
        "mp3" => ("audio", "audio/mpeg"),
        "ogg" | "oga" | "opus" => ("audio", "audio/ogg"),
        "wav" => ("audio", "audio/wav"),
        "flac" => ("audio", "audio/flac"),
        "m4a" | "aac" => ("audio", "audio/mp4"),
        "weba" => ("audio", "audio/webm"),
        _ => return None,
    })
}

fn error(status: StatusCode) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .body(Vec::new())
        .unwrap_or_default()
}

/// Parses `bytes=start-end` / `bytes=start-` / `bytes=-suffix` (single range only).
fn parse_range(value: &str, size: u64) -> Option<(u64, u64)> {
    let spec = value.trim().strip_prefix("bytes=")?;
    if spec.contains(',') || size == 0 {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let (start, end) = if a.is_empty() {
        let suffix: u64 = b.parse().ok()?;
        (size.saturating_sub(suffix), size - 1)
    } else {
        let start: u64 = a.parse().ok()?;
        let end = if b.is_empty() {
            size - 1
        } else {
            b.parse::<u64>().ok()?.min(size - 1)
        };
        (start, end)
    };
    (start <= end && start < size).then_some((start, end))
}

/// Answers an `owfile://` request.
pub async fn handle(app: AppHandle, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
    let Some(id) = request
        .uri()
        .path()
        .trim_matches('/')
        .split('/')
        .next()
        .and_then(crate::dto::unhex32)
    else {
        return error(StatusCode::BAD_REQUEST);
    };
    let Some(messenger) = app.state::<AppState>().messenger.lock().await.clone() else {
        return error(StatusCode::SERVICE_UNAVAILABLE);
    };
    let Ok(info) = messenger.file(&id) else {
        return error(StatusCode::NOT_FOUND);
    };
    // `path` is set only for our own files and complete downloads.
    let Some(path) = info.path.filter(|p| p.is_file()) else {
        return error(StatusCode::NOT_FOUND);
    };
    let Some((_, mime)) = media_type(&info.name) else {
        return error(StatusCode::FORBIDDEN);
    };
    let range = request
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);

    let result =
        tauri::async_runtime::spawn_blocking(move || -> std::io::Result<Response<Vec<u8>>> {
            let mut file = std::fs::File::open(&path)?;
            let size = file.metadata()?.len();
            let (status, start, end) = match range.as_deref().map(|r| parse_range(r, size)) {
                Some(Some((s, e))) => (StatusCode::PARTIAL_CONTENT, s, e.min(s + MAX_BODY - 1)),
                Some(None) => {
                    return Ok(Response::builder()
                        .status(StatusCode::RANGE_NOT_SATISFIABLE)
                        .header(header::CONTENT_RANGE, format!("bytes */{size}"))
                        .body(Vec::new())
                        .unwrap_or_default());
                }
                None if size <= MAX_WHOLE => (StatusCode::OK, 0, size.saturating_sub(1)),
                None => (StatusCode::PARTIAL_CONTENT, 0, MAX_BODY - 1),
            };
            let len = if size == 0 { 0 } else { end - start + 1 };
            let mut body = vec![0u8; usize::try_from(len).unwrap_or(0)];
            file.seek(SeekFrom::Start(start))?;
            file.read_exact(&mut body)?;
            let mut res = Response::builder()
                .status(status)
                .header(header::CONTENT_TYPE, mime)
                .header(header::ACCEPT_RANGES, "bytes")
                .header(header::CONTENT_LENGTH, len)
                .header("X-Content-Type-Options", "nosniff")
                .header(header::CACHE_CONTROL, "private, max-age=3600");
            if status == StatusCode::PARTIAL_CONTENT {
                res = res.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}"));
            }
            Ok(res.body(body).unwrap_or_default())
        })
        .await;
    match result {
        Ok(Ok(r)) => r,
        _ => error(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_and_types() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some((0, 99)));
        assert_eq!(parse_range("bytes=900-", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=-100", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=990-5000", 1000), Some((990, 999)));
        assert_eq!(parse_range("bytes=1000-", 1000), None);
        assert_eq!(parse_range("bytes=0-1,5-6", 1000), None);
        assert_eq!(media_type("Clip.MP4"), Some(("video", "video/mp4")));
        assert_eq!(media_type("a.svg"), None, "no SVG: it can carry scripts");
        assert_eq!(media_type("page.html"), None);
        assert_eq!(media_type("noext"), None);
    }
}
