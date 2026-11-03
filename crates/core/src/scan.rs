//! Scan a `PhotoSource` against the local `History` and produce a
//! transfer plan: items that have never been copied for this device.

use crate::db::{History, TransferKey};
use crate::live_photo::{group_by, is_image, is_video};
use crate::source::{DeviceInfo, SourceEntry};
use crate::{PhotoSource, Result};
use serde::Serialize;
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct PlanItem {
    pub entry: SourceEntry,
    pub key: TransferKey,
}

#[derive(Debug, Clone, Default)]
pub struct PlanSummary {
    pub total_files: usize,
    pub total_bytes: u64,
    pub new_files: usize,
    pub new_bytes: u64,
    pub skipped_already_done: usize,
}

/// Device info + file listing, with no DB involved yet. This is the async
/// half of the plan — safe to await.
#[derive(Debug, Clone)]
pub struct ScanResult {
    pub info: DeviceInfo,
    pub files: Vec<SourceEntry>,
}

pub async fn scan_files<S: PhotoSource>(src: &mut S, root: &str) -> Result<ScanResult> {
    let info = src.device_info().await?;
    let mut files = src.walk(root).await?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(ScanResult { info, files })
}

/// Fully synchronous classification: given a snapshot of files + a DB
/// handle, decide what's new. Keeping this sync means callers never hold
/// a `&History` across an `.await`, which matters because `rusqlite::
/// Connection` is `Send` but not `Sync`.
pub fn classify(
    info: &DeviceInfo,
    files: Vec<SourceEntry>,
    history: &History,
) -> Result<(Vec<PlanItem>, PlanSummary)> {
    let mut summary = PlanSummary::default();
    let mut plan = Vec::new();
    for entry in files {
        if entry.is_dir {
            continue;
        }
        summary.total_files += 1;
        summary.total_bytes += entry.size;
        let key = TransferKey {
            udid: info.udid.clone(),
            remote_path: entry.path.clone(),
            size: entry.size,
            remote_mtime: entry.mtime,
        };
        if history.is_transferred(&key)? {
            summary.skipped_already_done += 1;
            continue;
        }
        summary.new_files += 1;
        summary.new_bytes += entry.size;
        plan.push(PlanItem { entry, key });
    }
    Ok((plan, summary))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Photo,
    Video,
    Live,
}

/// One tile of the selection grid: a Live Photo is one item with two files.
#[derive(Debug, Clone, Serialize)]
pub struct CatalogItem {
    /// Primary file (the photo of a Live Photo). Also the thumbnail key and
    /// the value sent back in `ImportOptions::only`.
    pub id: String,
    pub files: Vec<String>,
    pub kind: ItemKind,
    pub size: u64,
    pub mtime: Option<i64>,
    /// Every file of the item is already in the history.
    pub imported: bool,
}

/// Every item on the device, newest first, with its import state.
pub fn catalog(
    info: &DeviceInfo,
    files: Vec<SourceEntry>,
    history: &History,
) -> Result<(Vec<CatalogItem>, PlanSummary)> {
    let (plan, summary) = classify(info, files.clone(), history)?;
    let pending: HashSet<&str> = plan.iter().map(|p| p.entry.path.as_str()).collect();
    let files: Vec<SourceEntry> = files.into_iter().filter(|e| !e.is_dir).collect();
    let mut items: Vec<CatalogItem> = group_by(&files, |e| e.path.as_str())
        .into_iter()
        .map(|g| {
            let has_image = g.iter().any(|e| is_image(&e.path));
            let has_video = g.iter().any(|e| is_video(&e.path));
            let primary = g
                .iter()
                .find(|e| is_image(&e.path))
                .or_else(|| g.iter().find(|e| is_video(&e.path)))
                .unwrap_or(&g[0]);
            CatalogItem {
                id: primary.path.clone(),
                files: g.iter().map(|e| e.path.clone()).collect(),
                kind: match (has_image, has_video) {
                    (true, true) => ItemKind::Live,
                    (false, true) => ItemKind::Video,
                    _ => ItemKind::Photo,
                },
                size: g.iter().map(|e| e.size).sum(),
                mtime: primary.mtime,
                imported: g.iter().all(|e| !pending.contains(e.path.as_str())),
            }
        })
        .collect();
    items.sort_by(|a, b| b.mtime.cmp(&a.mtime).then_with(|| b.id.cmp(&a.id)));
    Ok((items, summary))
}

/// Convenience wrapper for serial single-threaded use (CLI, tests).
pub async fn build_plan<S: PhotoSource>(
    src: &mut S,
    history: &History,
    root: &str,
) -> Result<(Vec<PlanItem>, PlanSummary)> {
    let r = scan_files(src, root).await?;
    classify(&r.info, r.files, history)
}
