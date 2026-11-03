use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
pub enum TransferEvent {
    Scanned { total_files: usize, new_files: usize, new_bytes: u64, skipped: usize },
    Start { total_files: usize, total_bytes: u64 },
    FileStart { path: String, size: u64 },
    FileProgress { bytes_delta: u64 },
    FileDone { path: String, size: u64 },
    FileError { path: String, error: String },
}

#[derive(Debug, Clone)]
pub struct CopyItem {
    pub remote: String,
    pub expected_size: u64,
    pub dst: PathBuf,
}

#[derive(Debug, Clone)]
pub struct CopyResult {
    pub remote: String,
    pub dst: PathBuf,
    pub bytes: u64,
    pub error: Option<String>,
}

#[derive(Clone, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn reset(&self) {
        self.0.store(false, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

pub type EventSink<'a> = &'a mut (dyn FnMut(TransferEvent) + Send);
