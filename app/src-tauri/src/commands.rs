use crate::state::{AppState, BusyGuard};
use photoport_core::db::History;
use photoport_core::heic::{self, ConvertOptions, OutputFormat};
use photoport_core::import::{history_path, run_import, ImportOptions, DCIM_ROOT};
use photoport_core::prereq::{self, Help};
use photoport_core::layout::{DateStyle, Lang};
use photoport_core::scan::{catalog, CatalogItem, ScanResult};
use photoport_core::PhotoSource;
use photoport_core::smart::{self, SmartIndex, Suggestion};
use photoport_core::source::afc::{probe, AfcSource, DeviceStatus};
use photoport_core::TransferEvent;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_opener::OpenerExt;

pub const EV_PROGRESS: &str = "photoport://progress";
pub const EV_DONE: &str = "photoport://done";
pub const EV_FAILED: &str = "photoport://failed";
pub const EV_SCAN: &str = "photoport://scan";

#[derive(Serialize)]
pub struct StatusJs {
    #[serde(flatten)]
    status: DeviceStatus,
    help: Help,
    importing: bool,
    /// None: this OS cannot convert HEIC yet.
    heic_converter: Option<heic::Backend>,
}

#[derive(Serialize)]
pub struct ScanSummaryJs {
    total_files: usize,
    total_bytes: u64,
    new_files: usize,
    new_bytes: u64,
    skipped: usize,
    items: Vec<CatalogItem>,
}

#[derive(Deserialize)]
pub struct ImportOpts {
    dst: String,
    delete_after: bool,
    format: OutputFormat,
    quality: u8,
    /// `month`, `day`, `year` or `flat`.
    layout: String,
    /// Catalog item ids picked in the grid. None = every new item.
    selection: Option<Vec<String>>,
    lang: Option<String>,
}

#[tauri::command]
pub async fn device_status(state: State<'_, AppState>) -> Result<StatusJs, String> {
    let importing = state.busy.load(Ordering::SeqCst);
    // While importing, AFC connections are busy: report the last known state.
    let status = if importing {
        state.last_status.lock().await.clone().unwrap_or(DeviceStatus::NoDevice)
    } else {
        let s = probe().await;
        let prev = state.last_status.lock().await.replace(s.clone());
        if udid_of(prev.as_ref()) != udid_of(Some(&s)) {
            state.forget_device();
        }
        s
    };
    Ok(StatusJs { status, help: prereq::help(), importing, heic_converter: heic::backend() })
}

#[derive(Clone, Serialize)]
struct ScanProgress {
    done: usize,
    total: usize,
}

#[tauri::command]
pub async fn scan_device(dst: String, app: AppHandle) -> Result<ScanSummaryJs, String> {
    let dst = PathBuf::from(dst);
    let mut src = AfcSource::connect_first().await.map_err(|e| e.to_string())?;
    let info = src.device_info().await.map_err(|e| e.to_string())?;
    let mut last = std::time::Instant::now();
    let mut on_progress = move |done: usize, total: usize| {
        // ~10 updates per second is plenty for a progress bar.
        if done == 0 || done == total || last.elapsed().as_millis() >= 100 {
            last = std::time::Instant::now();
            let _ = app.emit(EV_SCAN, ScanProgress { done, total });
        }
    };
    let files = src.walk_with_progress(DCIM_ROOT, &mut on_progress).await.map_err(|e| e.to_string())?;
    let scan = ScanResult { info, files };
    std::fs::create_dir_all(&dst).map_err(|e| e.to_string())?;
    let db = History::open(&history_path(&dst)).map_err(|e| e.to_string())?;
    let (items, s) = catalog(&scan.info, scan.files, &db).map_err(|e| e.to_string())?;
    Ok(ScanSummaryJs {
        total_files: s.total_files,
        total_bytes: s.total_bytes,
        new_files: s.new_files,
        new_bytes: s.new_bytes,
        skipped: s.skipped_already_done,
        items,
    })
}

fn udid_of(s: Option<&DeviceStatus>) -> Option<&str> {
    match s? {
        DeviceStatus::Ready { device } | DeviceStatus::NotPaired { device } => Some(&device.udid),
        _ => None,
    }
}

#[tauri::command]
pub async fn start_import(opts: ImportOpts, app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    if state.busy.swap(true, Ordering::SeqCst) {
        return Err("import already running".into());
    }
    let guard = BusyGuard(state.busy.clone());
    let cancel = state.cancel.clone();
    cancel.reset();

    tokio::spawn(async move {
        let _guard = guard;
        let dst = PathBuf::from(&opts.dst);
        let result = async {
            let mut src = AfcSource::connect_first().await?;
            let emitter = app.clone();
            let mut sink = move |ev: TransferEvent| {
                let _ = emitter.emit(EV_PROGRESS, ev);
            };
            let format = if heic::backend().is_some() { opts.format } else { OutputFormat::Original };
            let lang = if opts.lang.as_deref() == Some("en") { Lang::En } else { Lang::Fr };
            let o = ImportOptions {
                delete_after: opts.delete_after,
                convert: ConvertOptions { format, quality: opts.quality.clamp(1, 100) },
                style: DateStyle::from_key(&opts.layout, lang).unwrap_or(DateStyle::YearMonthNamed(lang)),
                only: opts.selection.map(|v| v.into_iter().collect()),
                ..Default::default()
            };
            run_import(&mut src, &dst, &o, &cancel, &mut sink).await
        }
        .await;
        match result {
            Ok(report) => {
                let _ = app.emit(EV_DONE, report);
            }
            Err(e) => {
                let _ = app.emit(EV_FAILED, e.to_string());
            }
        }
    });
    Ok(())
}

#[tauri::command]
pub async fn cancel_import(state: State<'_, AppState>) -> Result<(), String> {
    state.cancel.cancel();
    Ok(())
}

/// Fixed target only: the front end cannot make us open arbitrary URIs.
#[tauri::command]
pub async fn open_apple_devices(app: AppHandle) -> Result<(), String> {
    let opener = app.opener();
    if opener.open_url(prereq::APPLE_DEVICES_STORE_URI, None::<&str>).is_ok() {
        return Ok(());
    }
    opener
        .open_url(prereq::APPLE_DEVICES_WEB_URL, None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn open_folder(app: AppHandle, path: String) -> Result<(), String> {
    app.opener().open_path(path, None::<&str>).map_err(|e| e.to_string())
}

#[derive(Serialize)]
pub struct SmartReady {
    suggestions: Vec<Suggestion>,
}

#[derive(Serialize, Deserialize)]
struct SavedIndex {
    version: String,
    index: SmartIndex,
}

/// Loads (or builds, the first time: copies about 120 MB from the iPhone)
/// the index of places and scenes iOS recognised in the photos.
#[tauri::command]
pub async fn smart_prepare(state: State<'_, AppState>) -> Result<SmartReady, String> {
    let udid = match state.last_status.lock().await.as_ref() {
        Some(DeviceStatus::Ready { device }) => device.udid.clone(),
        _ => return Err("no device".into()),
    };
    let mut slot = state.smart.lock().await;
    if let Some((u, idx)) = slot.as_ref() {
        if *u == udid {
            return Ok(SmartReady { suggestions: idx.suggestions(10) });
        }
    }
    let cache = state.cache_dir.get().cloned().ok_or("no cache dir")?.join("smart");
    let saved_path = cache.join(format!("{udid}.json"));
    let mut src = AfcSource::connect_first().await.map_err(|e| e.to_string())?;
    let version = smart::device_version(&mut src).await.map_err(|e| e.to_string())?;

    let saved = tokio::fs::read(&saved_path)
        .await
        .ok()
        .and_then(|b| serde_json::from_slice::<SavedIndex>(&b).ok())
        .filter(|s| s.version == version);
    let index = match saved {
        Some(s) => s.index,
        None => {
            let index = SmartIndex::build_from_device(&mut src, &cache.join("work"))
                .await
                .map_err(|e| e.to_string())?;
            let saved = SavedIndex { version, index };
            if let Ok(json) = serde_json::to_vec(&saved) {
                let _ = tokio::fs::write(&saved_path, &json).await;
            }
            saved.index
        }
    };
    let suggestions = index.suggestions(10);
    *slot = Some((udid, Arc::new(index)));
    Ok(SmartReady { suggestions })
}

/// DCIM paths whose iOS labels match the query. Empty until `smart_prepare` ran.
#[tauri::command]
pub async fn smart_search(query: String, state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let index = state.smart.lock().await.as_ref().map(|(_, i)| i.clone()).ok_or("not ready")?;
    Ok(index.search(&query))
}
