//! Capture-date extraction for images (EXIF) and videos (QuickTime mvhd).
//!
//! All functions return a Unix epoch (seconds, UTC-ish — iPhone writes
//! local time without zone for EXIF; we accept it as-is). On failure the
//! caller falls back to the file's mtime.

use chrono::{NaiveDateTime, TimeZone, Utc};
use std::path::Path;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};

/// Dispatch by file extension. Returns `None` on any error or missing tag.
pub async fn capture_date(path: &Path) -> Option<i64> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())?;
    match ext.as_str() {
        "jpg" | "jpeg" | "heic" | "heif" | "png" | "tif" | "tiff" | "dng" => {
            capture_date_image(path).await
        }
        "mov" | "mp4" | "m4v" | "qt" => capture_date_video(path).await,
        _ => None,
    }
}

pub async fn capture_date_image(path: &Path) -> Option<i64> {
    // kamadak-exif is sync and expects a `BufReader`. Run on blocking pool.
    let p = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let file = std::fs::File::open(&p).ok()?;
        let mut reader = std::io::BufReader::new(file);
        let exif = exif::Reader::new().read_from_container(&mut reader).ok()?;
        // Prefer DateTimeOriginal, fall back to DateTimeDigitized, then DateTime.
        for &tag in &[
            exif::Tag::DateTimeOriginal,
            exif::Tag::DateTimeDigitized,
            exif::Tag::DateTime,
        ] {
            if let Some(f) = exif.get_field(tag, exif::In::PRIMARY) {
                if let Some(ts) = parse_exif_datetime(&f.display_value().to_string()) {
                    return Some(ts);
                }
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

/// Wall-clock seconds. Raw EXIF is "2024:03:12 14:35:07" but kamadak-exif's
/// `display_value` renders "2024-03-12 14:35:07": accept both.
fn parse_exif_datetime(s: &str) -> Option<i64> {
    let s = s.trim().trim_matches('"');
    let ndt = NaiveDateTime::parse_from_str(s, "%Y:%m:%d %H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S"))
        .ok()?;
    Some(Utc.from_utc_datetime(&ndt).timestamp())
}

/// Quicktime/MP4 `mvhd` box walker. Returns `creation_time` as Unix epoch.
///
/// Box format (ISO/IEC 14496-12):
///   size:u32 (box size incl. header)
///   type:[u8;4]
///   if size == 1: largesize:u64 (full size including this field)
///   if size == 0: box extends to EOF (not supported here)
///   payload follows
///
/// `mvhd` payload v0: version:u8, flags:[u8;3], creation_time:u32, ...
/// `mvhd` payload v1: version:u8, flags:[u8;3], creation_time:u64, ...
///
/// QuickTime epoch starts 1904-01-01 UTC; Unix = QT - 2_082_844_800.
pub async fn capture_date_video(path: &Path) -> Option<i64> {
    let mut f = File::open(path).await.ok()?;
    // Dig for `moov` → `mvhd`. Hard cap on walked bytes to avoid pathological files.
    const MAX_WALK: u64 = 64 * 1024 * 1024;
    let moov_off = find_box(&mut f, b"moov", 0, MAX_WALK).await?;
    // moov payload starts right after its header. find_box returned the
    // offset of moov's PAYLOAD (just past type+size), with its size known.
    let moov_payload_end = moov_off.payload_end;
    let mvhd = find_box(
        &mut f,
        b"mvhd",
        moov_off.payload_start,
        moov_payload_end,
    )
    .await?;

    f.seek(SeekFrom::Start(mvhd.payload_start)).await.ok()?;
    let mut hdr = [0u8; 4];
    f.read_exact(&mut hdr).await.ok()?;
    let version = hdr[0];
    let qt_time: u64 = if version == 1 {
        let mut b = [0u8; 8];
        f.read_exact(&mut b).await.ok()?;
        u64::from_be_bytes(b)
    } else {
        let mut b = [0u8; 4];
        f.read_exact(&mut b).await.ok()?;
        u32::from_be_bytes(b) as u64
    };
    const QT_EPOCH: i64 = 2_082_844_800;
    let unix = qt_time as i64 - QT_EPOCH;
    (unix > 0).then(|| crate::layout::utc_to_wall_clock(unix))
}

struct FoundBox {
    payload_start: u64,
    payload_end: u64,
}

async fn find_box(
    f: &mut File,
    want_type: &[u8; 4],
    start: u64,
    end: u64,
) -> Option<FoundBox> {
    let mut pos = start;
    while pos + 8 <= end {
        f.seek(SeekFrom::Start(pos)).await.ok()?;
        let mut head = [0u8; 8];
        f.read_exact(&mut head).await.ok()?;
        let size32 = u32::from_be_bytes([head[0], head[1], head[2], head[3]]);
        let mut box_type = [0u8; 4];
        box_type.copy_from_slice(&head[4..8]);

        let (hdr_len, box_size) = if size32 == 1 {
            let mut ext = [0u8; 8];
            f.read_exact(&mut ext).await.ok()?;
            (16, u64::from_be_bytes(ext))
        } else if size32 == 0 {
            (8, end - pos)
        } else {
            (8, size32 as u64)
        };
        if box_size < hdr_len as u64 {
            return None;
        }
        let payload_start = pos + hdr_len as u64;
        let payload_end = pos + box_size;
        if &box_type == want_type {
            return Some(FoundBox {
                payload_start,
                payload_end,
            });
        }
        pos = payload_end;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::parse_exif_datetime;

    #[test]
    fn parses_raw_and_displayed_exif_dates() {
        let expected = Some(1_751_540_795); // 2025-07-03 11:06:35
        assert_eq!(parse_exif_datetime("2025:07:03 11:06:35"), expected);
        assert_eq!(parse_exif_datetime("2025-07-03 11:06:35"), expected);
        assert_eq!(parse_exif_datetime("\"2025:07:03 11:06:35\""), expected);
        assert_eq!(parse_exif_datetime("garbage"), None);
    }
}
