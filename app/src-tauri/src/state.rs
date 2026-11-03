use photoport_core::smart::SmartIndex;
use photoport_core::source::afc::{AfcSource, DeviceStatus};
use photoport_core::CancelFlag;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use tokio::sync::{Mutex, Semaphore, SemaphorePermit};

/// Measured on an iPhone 16 Pro: 8 connections ~25 thumbnails/s, 16 is slower.
const THUMB_CONNECTIONS: usize = 8;
const MEDIA_CONNECTIONS: usize = 2;

pub struct AppState {
    pub cancel: CancelFlag,
    pub busy: Arc<AtomicBool>,
    pub last_status: Mutex<Option<DeviceStatus>>,
    pub thumbs: AfcPool,
    pub media: AfcPool,
    pub media_sizes: std::sync::Mutex<HashMap<String, u64>>,
    /// The video being previewed; replaced (and stopped) when another one opens.
    pub video: std::sync::Mutex<Option<Arc<crate::video::VideoStream>>>,
    pub cache_dir: OnceLock<PathBuf>,
    /// (udid, index). Building holds the lock so two clicks build once.
    pub smart: Mutex<Option<(String, Arc<SmartIndex>)>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            cancel: CancelFlag::new(),
            busy: Arc::default(),
            last_status: Mutex::default(),
            thumbs: AfcPool::new(THUMB_CONNECTIONS),
            media: AfcPool::new(MEDIA_CONNECTIONS),
            media_sizes: std::sync::Mutex::default(),
            video: std::sync::Mutex::default(),
            cache_dir: OnceLock::new(),
            smart: Mutex::default(),
        }
    }
}

impl AppState {
    pub fn forget_device(&self) {
        self.thumbs.clear();
        self.media.clear();
        self.media_sizes.lock().unwrap().clear();
        if let Some(v) = self.video.lock().unwrap().take() {
            v.close();
        }
    }
}

pub struct BusyGuard(pub Arc<AtomicBool>);

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// A few AFC connections reused across requests, so scrolling the grid or
/// seeking in a video does not reconnect every time.
pub struct AfcPool {
    permits: Semaphore,
    idle: std::sync::Mutex<Vec<AfcSource>>,
}

/// One pooled connection. Dropped without `keep()`, the connection is
/// discarded (it may be broken: iPhone unplugged or swapped).
pub struct Lease<'a> {
    pool: &'a AfcPool,
    _permit: SemaphorePermit<'a>,
    src: Option<AfcSource>,
}

impl AfcPool {
    fn new(n: usize) -> Self {
        Self { permits: Semaphore::new(n), idle: std::sync::Mutex::new(Vec::new()) }
    }

    pub async fn lease(&self) -> Result<Lease<'_>, String> {
        let permit = self.permits.acquire().await.map_err(|e| e.to_string())?;
        let reused = self.idle.lock().unwrap().pop();
        let src = match reused {
            Some(s) => s,
            None => AfcSource::connect_first().await.map_err(|e| e.to_string())?,
        };
        Ok(Lease { pool: self, _permit: permit, src: Some(src) })
    }

    pub fn clear(&self) {
        self.idle.lock().unwrap().clear();
    }
}

impl Lease<'_> {
    pub fn src(&mut self) -> &mut AfcSource {
        self.src.as_mut().expect("present until keep()")
    }

    pub fn keep(mut self) {
        if let Some(s) = self.src.take() {
            self.pool.idle.lock().unwrap().push(s);
        }
    }
}
