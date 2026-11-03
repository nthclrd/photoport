//! `thumb://localhost/DCIM/...?v=<size>-<mtime>`: iOS thumbnail, cached on disk.
//! `media://localhost/DCIM/...`: original file with HTTP Range, for previews.
//! On Windows the same URLs are `http://thumb.localhost/...` and `http://media.localhost/...`.

use crate::state::AppState;
use crate::video::VideoStream;
use std::path::{Path, PathBuf};
use tauri::http::{header, Request, Response, StatusCode};
use tauri::{AppHandle, Manager};

/// Largest slice sent per request; the player asks for the next one.
const MEDIA_CHUNK: u64 = 4 * 1024 * 1024;
/// Below this, a request without Range gets the whole file (photos).
const WHOLE_FILE_MAX: u64 = 64 * 1024 * 1024;
const THUMB_CACHE_MAX: u64 = 300 * 1024 * 1024;
const THUMB_CACHE_TARGET: u64 = 200 * 1024 * 1024;

type Resp = Response<Vec<u8>>;

fn dcim_path(req: &Request<Vec<u8>>) -> Option<String> {
    let p = percent_encoding::percent_decode_str(req.uri().path()).decode_utf8_lossy().into_owned();
    (p.starts_with("/DCIM/") && !p.contains("..")).then_some(p)
}

fn status(code: StatusCode) -> Resp {
    Response::builder().status(code).body(Vec::new()).expect("static response")
}

pub fn thumbs_dir(app: &AppHandle) -> Option<PathBuf> {
    app.state::<AppState>().cache_dir.get().map(|d| d.join("thumbs"))
}

pub async fn serve_thumb(app: AppHandle, req: Request<Vec<u8>>) -> Resp {
    let Some(path) = dcim_path(&req) else { return status(StatusCode::BAD_REQUEST) };
    let version = req.uri().query().unwrap_or("").to_string();
    let cached = thumbs_dir(&app).map(|d| d.join(format!("{:016x}.jpg", fnv64(&format!("{path}?{version}")))));
    if let Some(file) = &cached {
        if let Ok(bytes) = tokio::fs::read(file).await {
            return jpeg(bytes);
        }
    }
    let state = app.state::<AppState>();
    let mut lease = match state.thumbs.lease().await {
        Ok(l) => l,
        Err(_) => return status(StatusCode::SERVICE_UNAVAILABLE),
    };
    match lease.src().thumbnail(&path).await {
        Ok(bytes) => {
            lease.keep();
            if let Some(file) = cached {
                let _ = write_atomic(&file, &bytes).await;
            }
            jpeg(bytes)
        }
        Err(e) => {
            tracing::debug!("thumbnail {path}: {e}");
            status(StatusCode::NOT_FOUND)
        }
    }
}

fn jpeg(bytes: Vec<u8>) -> Resp {
    Response::builder()
        .header(header::CONTENT_TYPE, "image/jpeg")
        .header(header::CACHE_CONTROL, "max-age=86400")
        .body(bytes)
        .expect("static response")
}

pub async fn serve_media(app: AppHandle, req: Request<Vec<u8>>) -> Resp {
    let Some(path) = dcim_path(&req) else { return status(StatusCode::BAD_REQUEST) };
    let range = req.headers().get(header::RANGE).and_then(|v| v.to_str().ok()).map(str::to_string);
    let state = app.state::<AppState>();
    let mut lease = match state.media.lease().await {
        Ok(l) => l,
        Err(_) => return status(StatusCode::SERVICE_UNAVAILABLE),
    };

    let known = state.media_sizes.lock().unwrap().get(&path).copied();
    let size = match known {
        Some(s) => s,
        None => match lease.src().stat(&path).await {
            Ok(e) => {
                state.media_sizes.lock().unwrap().insert(path.clone(), e.size);
                e.size
            }
            Err(_) => return status(StatusCode::NOT_FOUND),
        },
    };
    if size == 0 {
        lease.keep();
        return status(StatusCode::NO_CONTENT);
    }
    if photoport_core::live_photo::is_video(&path) {
        lease.keep();
        return serve_video(&state, path, size, range.as_deref()).await;
    }

    let (start, end, partial) = match range.as_deref().and_then(|r| parse_range(r, size)) {
        Some((s, e)) => (s, e.min(s + MEDIA_CHUNK - 1), true),
        None if range.is_some() => {
            lease.keep();
            return Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{size}"))
                .body(Vec::new())
                .expect("static response");
        }
        None if size <= WHOLE_FILE_MAX => (0, size - 1, false),
        None => (0, MEDIA_CHUNK - 1, true),
    };

    let bytes = match lease.src().read_range(&path, start, (end - start + 1) as usize).await {
        Ok(b) => {
            lease.keep();
            b
        }
        Err(e) => {
            tracing::debug!("media {path}: {e}");
            return status(StatusCode::BAD_GATEWAY);
        }
    };
    let last = start + bytes.len().max(1) as u64 - 1;
    let mut b = Response::builder()
        .header(header::CONTENT_TYPE, mime_for(&path))
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, bytes.len().to_string());
    if partial {
        b = b
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_RANGE, format!("bytes {start}-{last}/{size}"));
    }
    b.body(bytes).expect("static response")
}

async fn serve_video(state: &AppState, path: String, size: u64, range: Option<&str>) -> Resp {
    let (start, end) = match range {
        Some(r) => match parse_range(r, size) {
            Some(x) => x,
            None => {
                return Response::builder()
                    .status(StatusCode::RANGE_NOT_SATISFIABLE)
                    .header(header::CONTENT_RANGE, format!("bytes */{size}"))
                    .body(Vec::new())
                    .expect("static response")
            }
        },
        None => (0, size - 1),
    };
    let stream = {
        let mut current = state.video.lock().unwrap();
        match current.as_ref() {
            Some(s) if s.path == path && !s.is_closed() => s.clone(),
            _ => {
                if let Some(old) = current.take() {
                    old.close();
                }
                let s = VideoStream::start(path.clone(), size);
                *current = Some(s.clone());
                s
            }
        }
    };
    match stream.read(start, end).await {
        Ok(bytes) if !bytes.is_empty() => {
            let last = start + bytes.len() as u64 - 1;
            Response::builder()
                .status(StatusCode::PARTIAL_CONTENT)
                .header(header::CONTENT_TYPE, mime_for(&path))
                .header(header::ACCEPT_RANGES, "bytes")
                .header(header::CONTENT_LENGTH, bytes.len().to_string())
                .header(header::CONTENT_RANGE, format!("bytes {start}-{last}/{size}"))
                .body(bytes)
                .expect("static response")
        }
        Ok(_) => status(StatusCode::RANGE_NOT_SATISFIABLE),
        Err(e) => {
            tracing::debug!("video {path}: {e}");
            status(StatusCode::BAD_GATEWAY)
        }
    }
}

/// `bytes=a-b`, `bytes=a-`, `bytes=-n` (one range only). Inclusive end.
fn parse_range(h: &str, size: u64) -> Option<(u64, u64)> {
    let spec = h.strip_prefix("bytes=")?.split(',').next()?.trim();
    let (a, b) = spec.split_once('-')?;
    let (start, end) = if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        (size.saturating_sub(n), size - 1)
    } else {
        let start: u64 = a.parse().ok()?;
        let end = if b.is_empty() { size - 1 } else { b.parse::<u64>().ok()?.min(size - 1) };
        (start, end)
    };
    (start <= end && start < size).then_some((start, end))
}

fn mime_for(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).as_deref() {
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("heic" | "heif") => "image/heic",
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("dng") => "image/x-adobe-dng",
        Some("mov" | "qt") => "video/quicktime",
        Some("mp4" | "m4v") => "video/mp4",
        _ => "application/octet-stream",
    }
}

fn fnv64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
    }
    h
}

async fn write_atomic(file: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let tmp = file.with_extension("part");
    tokio::fs::write(&tmp, bytes).await?;
    tokio::fs::rename(&tmp, file).await
}

/// Oldest thumbnails go first once the cache passes its cap.
pub fn prune_thumb_cache(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    if total <= THUMB_CACHE_MAX {
        return;
    }
    files.sort_by_key(|f| f.0);
    for (_, len, path) in files {
        if total <= THUMB_CACHE_TARGET {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_range;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-", 100), Some((0, 99)));
        assert_eq!(parse_range("bytes=0-1", 100), Some((0, 1)));
        assert_eq!(parse_range("bytes=10-500", 100), Some((10, 99)));
        assert_eq!(parse_range("bytes=-10", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=100-", 100), None);
        assert_eq!(parse_range("bytes=5-2", 100), None);
        assert_eq!(parse_range("items=0-1", 100), None);
    }
}
