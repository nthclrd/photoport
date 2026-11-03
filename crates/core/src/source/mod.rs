use crate::transfer::{CancelFlag, CopyItem, CopyResult, EventSink, TransferEvent};
use crate::{Error, Result};
use async_trait::async_trait;
use serde::Serialize;
use std::path::Path;

pub mod afc;
pub mod mock;

#[derive(Debug, Clone)]
pub struct SourceEntry {
    pub path: String,
    pub size: u64,
    pub mtime: Option<i64>,
    pub is_dir: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceInfo {
    pub name: String,
    pub model: String,
    pub ios_version: String,
    pub udid: String,
}

/// Device-side photo store. Albums, edited photos, iCloud-only items and
/// Wi-Fi sources would be further implementations of this trait.
#[async_trait]
pub trait PhotoSource: Send {
    async fn device_info(&mut self) -> Result<DeviceInfo>;

    async fn list(&mut self, dir: &str) -> Result<Vec<SourceEntry>>;

    /// Every file (not directory) below `root`.
    async fn walk(&mut self, root: &str) -> Result<Vec<SourceEntry>>;

    /// Stream one file into `dst` through a `.part` temp file. Returns bytes written.
    async fn copy_file(&mut self, remote: &str, dst: &Path) -> Result<u64>;

    /// Delete `remote` only if the device still reports `expected_size`.
    async fn remove_verified(&mut self, remote: &str, expected_size: u64) -> Result<()>;

    /// Copy many files. Default is serial; backends override to parallelize.
    async fn copy_many(
        &mut self,
        items: Vec<CopyItem>,
        cancel: &CancelFlag,
        on_event: EventSink<'_>,
    ) -> Result<Vec<CopyResult>> {
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            if cancel.is_cancelled() {
                break;
            }
            on_event(TransferEvent::FileStart { path: item.remote.clone(), size: item.expected_size });
            let res = match self.copy_file(&item.remote, &item.dst).await {
                Ok(n) if item.expected_size != 0 && n != item.expected_size => {
                    let _ = tokio::fs::remove_file(&item.dst).await;
                    Err(Error::Other(format!(
                        "size mismatch: expected {} got {n}",
                        item.expected_size
                    )))
                }
                other => other,
            };
            match res {
                Ok(n) => {
                    on_event(TransferEvent::FileProgress { bytes_delta: n });
                    on_event(TransferEvent::FileDone { path: item.remote.clone(), size: n });
                    out.push(CopyResult { remote: item.remote, dst: item.dst, bytes: n, error: None });
                }
                Err(e) => {
                    on_event(TransferEvent::FileError { path: item.remote.clone(), error: e.to_string() });
                    out.push(CopyResult { remote: item.remote, dst: item.dst, bytes: 0, error: Some(e.to_string()) });
                }
            }
        }
        Ok(out)
    }
}
