//! Full incremental import pipeline, shared by the CLI and the desktop app:
//!
//! 1. scan the device and diff against history;
//! 2. copy new files into a staging dir (`.part` + rename, size verified);
//! 3. per Live Photo group, read the capture date and move into the chosen layout;
//! 4. optional HEIC to JPG conversion;
//! 5. record in SQLite;
//! 6. optional verified delete on the phone, then append errors to a log.

use crate::datemeta::capture_date;
use crate::db::{History, TransferRecord};
use crate::heic::{self, ConvertOptions, OutputFormat};
use crate::layout::{basename, folder_from_epoch, utc_to_wall_clock, DateStyle, Lang};
use crate::live_photo::{self, is_image, is_video};
use crate::scan::{classify, scan_files, PlanItem};
use crate::transfer::{CancelFlag, CopyItem, CopyResult, EventSink, TransferEvent};
use crate::{PhotoSource, Result};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;

pub const DCIM_ROOT: &str = "/DCIM";

#[derive(Debug, Clone)]
pub struct ImportOptions {
    /// Cap on new files for this run (whole Live Photo groups are kept). 0 = no cap.
    pub limit: usize,
    pub delete_after: bool,
    pub style: DateStyle,
    pub convert: ConvertOptions,
    /// Restrict to these catalog item ids (`CatalogItem::id`); any file of a
    /// Live Photo selects the whole group. None = everything new.
    pub only: Option<HashSet<String>>,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            limit: 0,
            delete_after: false,
            style: DateStyle::YearMonthNamed(Lang::Fr),
            convert: ConvertOptions::default(),
            only: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ImportReport {
    pub ok: usize,
    pub failed: usize,
    pub deleted: usize,
    pub bytes: u64,
    pub secs: f64,
    pub cancelled: bool,
    pub errors: Vec<String>,
}

pub fn app_dir(dst: &Path) -> PathBuf {
    dst.join(".photoport")
}

pub fn history_path(dst: &Path) -> PathBuf {
    app_dir(dst).join("history.sqlite")
}

pub fn error_log_path(dst: &Path) -> PathBuf {
    app_dir(dst).join("errors.log")
}

pub async fn run_import<S: PhotoSource>(
    src: &mut S,
    dst: &Path,
    opts: &ImportOptions,
    cancel: &CancelFlag,
    on_event: EventSink<'_>,
) -> Result<ImportReport> {
    let t0 = Instant::now();
    let mut report = ImportReport::default();
    tokio::fs::create_dir_all(app_dir(dst)).await?;

    let scan = scan_files(src, DCIM_ROOT).await?;
    // DB handle never lives across an .await: rusqlite::Connection is !Sync.
    let (plan, summary) = {
        let db = History::open(&history_path(dst))?;
        classify(&scan.info, scan.files, &db)?
    };
    on_event(TransferEvent::Scanned {
        total_files: summary.total_files,
        new_files: summary.new_files,
        new_bytes: summary.new_bytes,
        skipped: summary.skipped_already_done,
    });

    let mut groups = live_photo::group(&plan);
    if let Some(only) = &opts.only {
        let keys: HashSet<String> = only.iter().map(|id| live_photo::group_key(id)).collect();
        groups.retain(|g| keys.contains(&live_photo::group_key(&g[0].entry.path)));
    }
    if opts.limit > 0 {
        let mut kept = 0;
        groups.retain(|g| {
            let keep = kept < opts.limit;
            kept += g.len();
            keep
        });
    }
    let plan: Vec<PlanItem> = groups.iter().flatten().cloned().collect();
    // A Live Photo half retried alone goes next to the half imported earlier.
    let sibling_dirs: Vec<Option<PathBuf>> = {
        let db = History::open(&history_path(dst))?;
        groups
            .iter()
            .map(|g| db.sibling_dir(&g[0].key.udid, &g[0].entry.path).ok().flatten())
            .collect()
    };
    if plan.is_empty() || cancel.is_cancelled() {
        report.cancelled = cancel.is_cancelled();
        report.secs = t0.elapsed().as_secs_f64();
        return Ok(report);
    }

    // Leftovers from a crash or unplugged cable were never recorded: drop them.
    let staging = app_dir(dst).join("staging");
    let _ = tokio::fs::remove_dir_all(&staging).await;
    tokio::fs::create_dir_all(&staging).await?;

    let items: Vec<CopyItem> = plan
        .iter()
        .enumerate()
        .map(|(i, it)| CopyItem {
            remote: it.entry.path.clone(),
            expected_size: it.entry.size,
            dst: staging.join(format!("{i:06}_{}", basename(&it.entry.path))),
        })
        .collect();
    on_event(TransferEvent::Start {
        total_files: items.len(),
        total_bytes: items.iter().map(|i| i.expected_size).sum(),
    });
    let results = src.copy_many(items, cancel, &mut *on_event).await?;
    let by_remote: HashMap<&str, &CopyResult> =
        results.iter().map(|r| (r.remote.as_str(), r)).collect();

    let mut records = Vec::new();
    let mut deletable: Vec<(String, u64)> = Vec::new();
    for (gi, group) in groups.iter().enumerate() {
        let ok_members: Vec<(&PlanItem, &CopyResult)> = group
            .iter()
            .filter_map(|it| by_remote.get(it.entry.path.as_str()).map(|r| (it, *r)))
            .filter(|(_, r)| r.error.is_none())
            .collect();
        for it in group {
            if let Some(r) = by_remote.get(it.entry.path.as_str()) {
                if let Some(e) = &r.error {
                    report.failed += 1;
                    report.errors.push(format!("{}: {e}", it.entry.path));
                }
            }
        }
        if ok_members.is_empty() {
            continue;
        }

        let folder = match &sibling_dirs[gi] {
            Some(dir) => dir.clone(),
            None => dst.join(folder_from_epoch(group_date(&ok_members).await, opts.style)),
        };
        for (it, res) in &ok_members {
            // Group key, not file path: both halves of a Live Photo get the same suffix.
            let final_path = unique_destination(
                &folder.join(basename(&it.entry.path)),
                &live_photo::group_key(&it.entry.path),
            );
            if let Some(parent) = final_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            if let Err(e) = tokio::fs::rename(&res.dst, &final_path).await {
                report.failed += 1;
                report.errors.push(format!("{}: move failed: {e}", it.entry.path));
                continue;
            }
            report.ok += 1;
            report.bytes += res.bytes;
            let local = match convert_if_asked(&final_path, &it.entry.path, opts.convert).await {
                Ok(p) => p,
                Err(e) => {
                    // Original is kept: the transfer itself succeeded.
                    report.errors.push(format!("{}: {e}", it.entry.path));
                    final_path
                }
            };
            records.push(TransferRecord {
                key: it.key.clone(),
                local_path: local.to_string_lossy().into_owned(),
                transferred_at: now_secs(),
            });
        }
        // Never split a Live Photo on the phone: delete only complete groups.
        if ok_members.len() == group.len() {
            deletable.extend(group.iter().map(|it| (it.entry.path.clone(), it.entry.size)));
        }
    }

    {
        let db = History::open(&history_path(dst))?;
        for rec in &records {
            db.record(rec)?;
        }
    }
    let _ = tokio::fs::remove_dir_all(&staging).await;
    report.cancelled = cancel.is_cancelled();

    if opts.delete_after && !report.cancelled {
        for (remote, size) in &deletable {
            match src.remove_verified(remote, *size).await {
                Ok(()) => report.deleted += 1,
                Err(e) => report.errors.push(format!("{remote}: delete failed: {e}")),
            }
        }
    }

    if !report.errors.is_empty() {
        append_error_log(dst, &report.errors).await;
    }
    report.secs = t0.elapsed().as_secs_f64();
    Ok(report)
}

/// Returns the path that now represents the photo on disk.
async fn convert_if_asked(heic_path: &Path, source_key: &str, c: ConvertOptions) -> Result<PathBuf> {
    if c.format == OutputFormat::Original || !heic::is_heic(heic_path) {
        return Ok(heic_path.to_path_buf());
    }
    let jpg = unique_destination(&heic_path.with_extension("JPG"), source_key);
    heic::to_jpeg(heic_path, &jpg, c.quality).await?;
    if c.format == OutputFormat::Jpg {
        tokio::fs::remove_file(heic_path).await?;
        return Ok(jpg);
    }
    Ok(heic_path.to_path_buf())
}

/// Image EXIF, then video mvhd, then the image's file date, then any file date.
async fn group_date(members: &[(&PlanItem, &CopyResult)]) -> i64 {
    for (_, res) in members.iter().filter(|(it, _)| is_image(&it.entry.path)) {
        if let Some(d) = capture_date(&res.dst).await {
            return d;
        }
    }
    for (_, res) in members.iter().filter(|(it, _)| is_video(&it.entry.path)) {
        if let Some(d) = capture_date(&res.dst).await {
            return d;
        }
    }
    members
        .iter()
        .find(|(it, _)| is_image(&it.entry.path))
        .or(members.first())
        .and_then(|(it, _)| it.entry.mtime)
        .map(utc_to_wall_clock)
        .unwrap_or(0)
}

/// Two different phone files with the same name (DCIM counter wrapped)
/// must not overwrite each other: suffix a stable hash of the source path.
pub fn unique_destination(base: &Path, source_key: &str) -> PathBuf {
    if !base.exists() {
        return base.to_path_buf();
    }
    let stem = base.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let ext = base.extension().and_then(|s| s.to_str()).map(|e| format!(".{e}")).unwrap_or_default();
    let mut h: u32 = 2_166_136_261;
    for b in source_key.bytes() {
        h = (h ^ b as u32).wrapping_mul(16_777_619);
    }
    base.with_file_name(format!("{stem}_{h:08x}{ext}"))
}

async fn append_error_log(dst: &Path, errors: &[String]) {
    let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let mut body = String::new();
    for e in errors {
        body.push_str(&format!("{ts}\t{e}\n"));
    }
    let open = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(error_log_path(dst))
        .await;
    if let Ok(mut f) = open {
        // tokio::fs::File writes in the background: without flush the
        // line can be lost when the file is dropped.
        let _ = f.write_all(body.as_bytes()).await;
        let _ = f.flush().await;
    }
}

fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}
