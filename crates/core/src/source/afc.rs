//! AFC-over-usbmuxd backend.
//!
//! AFC `FileDescriptor` does NOT close on drop: every `open` is paired with
//! `.close().await` on every path, including errors.

use super::{DeviceInfo, PhotoSource, SourceEntry};
use crate::transfer::{CancelFlag, CopyItem, CopyResult, EventSink, TransferEvent};
use crate::{Error, Result};
use async_trait::async_trait;
use idevice::provider::UsbmuxdProvider;
use idevice::services::afc::opcode::AfcFopenMode;
use idevice::services::afc::AfcClient;
use idevice::services::lockdown::LockdownClient;
use idevice::usbmuxd::{UsbmuxdAddr, UsbmuxdConnection};
use idevice::IdeviceService;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

const CLIENT_LABEL: &str = "photoport";
const COPY_CHUNK: usize = 1024 * 1024;
pub const DEFAULT_CONCURRENCY: usize = 4;
const THUMBS_ROOT: &str = "/PhotoData/Thumbnails/V2";
const DEFAULT_THUMB: &str = "5005.JPG";

/// What the UI needs to know before anything else.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DeviceStatus {
    /// usbmuxd not reachable: Windows without Apple Devices, Linux without usbmuxd.
    UsbmuxdMissing,
    NoDevice,
    NotPaired { device: DeviceInfo },
    Ready { device: DeviceInfo },
}

/// Cheap check used by the UI poller. Never returns an error.
pub async fn probe() -> DeviceStatus {
    let Ok(mut muxd) = UsbmuxdConnection::default().await else {
        return DeviceStatus::UsbmuxdMissing;
    };
    let Ok(devs) = muxd.get_devices().await else {
        return DeviceStatus::UsbmuxdMissing;
    };
    let Some(dev) = devs.into_iter().next() else {
        return DeviceStatus::NoDevice;
    };
    let Ok(addr) = UsbmuxdAddr::from_env_var() else {
        return DeviceStatus::UsbmuxdMissing;
    };
    let provider = dev.to_provider(addr, CLIENT_LABEL);
    let device = read_device_info(&provider, &dev.udid).await;
    match AfcClient::connect(&provider).await {
        Ok(_) => DeviceStatus::Ready { device },
        Err(_) => DeviceStatus::NotPaired { device },
    }
}

async fn read_device_info(provider: &UsbmuxdProvider, udid: &str) -> DeviceInfo {
    let mut info = DeviceInfo {
        name: "iPhone".into(),
        model: String::new(),
        ios_version: String::new(),
        udid: udid.to_string(),
    };
    // These keys are readable without a paired session.
    if let Ok(mut ld) = LockdownClient::connect(provider).await {
        if let Some(s) = lockdown_str(&mut ld, "DeviceName").await {
            info.name = s;
        }
        if let Some(s) = lockdown_str(&mut ld, "ProductType").await {
            info.model = marketing_name(&s).map(str::to_string).unwrap_or(s);
        }
        if let Some(s) = lockdown_str(&mut ld, "ProductVersion").await {
            info.ios_version = s;
        }
    }
    info
}

async fn lockdown_str(ld: &mut LockdownClient, key: &str) -> Option<String> {
    match ld.get_value(Some(key), None).await {
        Ok(v) => v.as_string().map(str::to_string),
        Err(e) => {
            tracing::debug!("lockdown {key}: {e}");
            None
        }
    }
}

/// ProductType to commercial name. Unknown identifiers fall back to the raw value.
pub fn marketing_name(product_type: &str) -> Option<&'static str> {
    Some(match product_type {
        "iPhone12,1" => "iPhone 11",
        "iPhone12,3" => "iPhone 11 Pro",
        "iPhone12,5" => "iPhone 11 Pro Max",
        "iPhone12,8" => "iPhone SE (2e génération)",
        "iPhone13,1" => "iPhone 12 mini",
        "iPhone13,2" => "iPhone 12",
        "iPhone13,3" => "iPhone 12 Pro",
        "iPhone13,4" => "iPhone 12 Pro Max",
        "iPhone14,4" => "iPhone 13 mini",
        "iPhone14,5" => "iPhone 13",
        "iPhone14,2" => "iPhone 13 Pro",
        "iPhone14,3" => "iPhone 13 Pro Max",
        "iPhone14,6" => "iPhone SE (3e génération)",
        "iPhone14,7" => "iPhone 14",
        "iPhone14,8" => "iPhone 14 Plus",
        "iPhone15,2" => "iPhone 14 Pro",
        "iPhone15,3" => "iPhone 14 Pro Max",
        "iPhone15,4" => "iPhone 15",
        "iPhone15,5" => "iPhone 15 Plus",
        "iPhone16,1" => "iPhone 15 Pro",
        "iPhone16,2" => "iPhone 15 Pro Max",
        "iPhone17,1" => "iPhone 16 Pro",
        "iPhone17,2" => "iPhone 16 Pro Max",
        "iPhone17,3" => "iPhone 16",
        "iPhone17,4" => "iPhone 16 Plus",
        "iPhone17,5" => "iPhone 16e",
        _ => return None,
    })
}

pub struct AfcSource {
    provider: UsbmuxdProvider,
    primary: AfcClient,
    info: DeviceInfo,
    concurrency: usize,
}

impl AfcSource {
    pub async fn connect_first() -> Result<Self> {
        let mut muxd = UsbmuxdConnection::default()
            .await
            .map_err(|_| Error::UsbmuxdUnavailable)?;
        let devs = muxd.get_devices().await.map_err(|_| Error::UsbmuxdUnavailable)?;
        let dev = devs.into_iter().next().ok_or(Error::NoDevice)?;
        let addr = UsbmuxdAddr::from_env_var().map_err(|_| Error::UsbmuxdUnavailable)?;
        let provider = dev.to_provider(addr, CLIENT_LABEL);
        let info = read_device_info(&provider, &dev.udid).await;
        let primary = AfcClient::connect(&provider).await.map_err(|e| match Error::from(e) {
            Error::Idevice(msg) => {
                tracing::warn!("AFC connect failed: {msg}");
                Error::NotPaired
            }
            other => other,
        })?;
        Ok(Self { provider, primary, info, concurrency: DEFAULT_CONCURRENCY })
    }

    pub fn with_concurrency(mut self, n: usize) -> Self {
        self.concurrency = n.max(1);
        self
    }

    pub fn concurrency(&self) -> usize {
        self.concurrency
    }

    async fn open_client(&self) -> Result<AfcClient> {
        Ok(AfcClient::connect(&self.provider).await?)
    }

    /// Directory names without a stat per entry.
    pub async fn list_names(&mut self, dir: &str) -> Result<Vec<String>> {
        let mut names = self.primary.list_dir(dir.to_string()).await?;
        names.retain(|n| n != "." && n != "..");
        Ok(names)
    }

    /// Whole file in memory; meant for small files such as thumbnails.
    pub async fn read_small(&mut self, path: &str) -> Result<Vec<u8>> {
        read_small_with(&mut self.primary, path).await
    }

    /// JPEG preview kept by iOS for a DCIM file (about 360x480), photos and
    /// videos alike. Live Photo videos have none: ask for the photo instead.
    pub async fn thumbnail(&mut self, dcim_path: &str) -> Result<Vec<u8>> {
        if !dcim_path.starts_with("/DCIM/") || dcim_path.contains("..") {
            return Err(Error::Other(format!("not a DCIM path: {dcim_path}")));
        }
        let dir = format!("{THUMBS_ROOT}{dcim_path}");
        if let Ok(b) = read_jpeg_with(&mut self.primary, &format!("{dir}/{DEFAULT_THUMB}")).await {
            return Ok(b);
        }
        // Other iOS versions may use another size id: take any JPEG there.
        let names = self.list_names(&dir).await?;
        let jpg = names
            .iter()
            .find(|n| n.to_ascii_uppercase().ends_with(".JPG"))
            .ok_or_else(|| Error::Other(format!("no thumbnail for {dcim_path}")))?;
        read_jpeg_with(&mut self.primary, &format!("{dir}/{jpg}")).await
    }

    pub async fn stat(&mut self, path: &str) -> Result<SourceEntry> {
        let info = self.primary.get_file_info_raw(path.to_string()).await?;
        Ok(entry_from(path, &info))
    }

    /// Bytes `[start, start + len)` of a file, fewer at end of file.
    pub async fn read_range(&mut self, path: &str, start: u64, len: usize) -> Result<Vec<u8>> {
        use tokio::io::AsyncSeekExt;
        let mut fd = self.primary.open(path.to_string(), AfcFopenMode::RdOnly).await?;
        let res: Result<Vec<u8>> = async {
            fd.seek(std::io::SeekFrom::Start(start)).await?;
            let mut buf = Vec::with_capacity(len);
            while buf.len() < len {
                let chunk = fd.read_n((len - buf.len()).min(COPY_CHUNK)).await?;
                if chunk.is_empty() {
                    break;
                }
                buf.extend_from_slice(&chunk);
            }
            Ok(buf)
        }
        .await;
        let _ = fd.close().await;
        res
    }

    /// Directory discovery on the primary client, `get_file_info` fanned out
    /// to `concurrency` extra AFC connections.
    pub async fn walk_parallel(&mut self, root: &str) -> Result<Vec<SourceEntry>> {
        self.walk_with_progress(root, &mut |_, _| {}).await
    }

    /// Same as `walk_parallel`; `on_progress(done, total)` follows the stat phase,
    /// the slow part (one round trip per file).
    pub async fn walk_with_progress(
        &mut self,
        root: &str,
        on_progress: &mut (dyn FnMut(usize, usize) + Send),
    ) -> Result<Vec<SourceEntry>> {
        let mut dirs = vec![root.to_string()];
        let mut file_paths = Vec::new();
        while let Some(cur) = dirs.pop() {
            let names = match self.primary.list_dir(cur.clone()).await {
                Ok(n) => n,
                Err(e) if cur != root => {
                    // Guessed wrong: it was a file without extension.
                    tracing::debug!("list_dir({cur}) failed ({e}), treating as file");
                    file_paths.push(cur);
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            for n in names {
                if n == "." || n == ".." || n.starts_with('.') {
                    continue;
                }
                let full = join_posix(&cur, &n);
                if n.contains('.') {
                    file_paths.push(full);
                } else {
                    dirs.push(full);
                }
            }
        }

        let total = file_paths.len();
        on_progress(0, total);
        let n = self.concurrency;
        let (work_tx, work_rx) = async_channel::bounded::<String>(n * 4);
        let (res_tx, res_rx) = async_channel::bounded::<(String, Result<SourceEntry>)>(n * 4);
        let mut handles = Vec::with_capacity(n);
        for _ in 0..n {
            let mut client = self.open_client().await?;
            let work_rx = work_rx.clone();
            let res_tx = res_tx.clone();
            handles.push(tokio::spawn(async move {
                while let Ok(path) = work_rx.recv().await {
                    let out = client
                        .get_file_info_raw(path.clone())
                        .await
                        .map(|info| entry_from(&path, &info))
                        .map_err(Error::from);
                    if res_tx.send((path, out)).await.is_err() {
                        break;
                    }
                }
            }));
        }
        drop(work_rx);
        drop(res_tx);

        let feeder = tokio::spawn(async move {
            for p in file_paths {
                if work_tx.send(p).await.is_err() {
                    break;
                }
            }
        });

        let mut out = Vec::new();
        let mut done = 0;
        while let Ok((path, r)) = res_rx.recv().await {
            done += 1;
            on_progress(done, total);
            match r {
                Ok(e) if !e.is_dir => out.push(e),
                Ok(_) => {}
                // A photo deleted on the phone between list and stat is not fatal.
                Err(e) => tracing::warn!("stat {path}: {e}"),
            }
        }
        let _ = feeder.await;
        for h in handles {
            let _ = h.await;
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }
}

/// Drives `AfcSource::read_blocks`: which block to read next, and where it goes.
#[async_trait]
pub trait BlockPlan: Send {
    /// Next block index to read, or None to stop. May wait.
    async fn next(&mut self) -> Option<u64>;
    fn got(&mut self, index: u64, data: Vec<u8>);
}

impl AfcSource {
    /// Reads `path` block by block in the order `plan` asks for, through a
    /// single open handle: no reopen per block, a seek only when the order jumps.
    pub async fn read_blocks(&mut self, path: &str, block: usize, plan: &mut dyn BlockPlan) -> Result<()> {
        use tokio::io::AsyncSeekExt;
        let mut fd = self.primary.open(path.to_string(), AfcFopenMode::RdOnly).await?;
        let res: Result<()> = async {
            let mut pos = 0u64;
            while let Some(index) = plan.next().await {
                let at = index * block as u64;
                if at != pos {
                    fd.seek(std::io::SeekFrom::Start(at)).await?;
                }
                let mut buf = Vec::with_capacity(block);
                while buf.len() < block {
                    let chunk = fd.read_n(block - buf.len()).await?;
                    if chunk.is_empty() {
                        break;
                    }
                    buf.extend_from_slice(&chunk);
                }
                pos = at + buf.len() as u64;
                plan.got(index, buf);
            }
            Ok(())
        }
        .await;
        let _ = fd.close().await;
        res
    }
}

async fn read_small_with(client: &mut AfcClient, path: &str) -> Result<Vec<u8>> {
    let mut fd = client.open(path.to_string(), AfcFopenMode::RdOnly).await?;
    let mut buf = Vec::new();
    let res: Result<()> = async {
        loop {
            let chunk = fd.read_n(COPY_CHUNK).await?;
            if chunk.is_empty() {
                return Ok(());
            }
            buf.extend_from_slice(&chunk);
        }
    }
    .await;
    let _ = fd.close().await;
    res.map(|_| buf)
}

/// Like `read_small_with`, but a short read that ends a complete JPEG is
/// taken as end of file: saves the empty read, a quarter of the round trips.
async fn read_jpeg_with(client: &mut AfcClient, path: &str) -> Result<Vec<u8>> {
    let mut fd = client.open(path.to_string(), AfcFopenMode::RdOnly).await?;
    let mut buf = Vec::new();
    let res: Result<()> = async {
        loop {
            let chunk = fd.read_n(COPY_CHUNK).await?;
            let short = chunk.len() < COPY_CHUNK;
            buf.extend_from_slice(&chunk);
            if chunk.is_empty() || (short && buf.ends_with(&[0xFF, 0xD9])) {
                return Ok(());
            }
        }
    }
    .await;
    let _ = fd.close().await;
    res.map(|_| buf)
}

async fn copy_one(
    client: &mut AfcClient,
    item: &CopyItem,
    cancel: &CancelFlag,
    prog: &async_channel::Sender<TransferEvent>,
) -> Result<u64> {
    let _ = prog
        .send(TransferEvent::FileStart { path: item.remote.clone(), size: item.expected_size })
        .await;
    if let Some(parent) = item.dst.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = tmp_path(&item.dst);
    let _ = tokio::fs::remove_file(&tmp).await;
    let mut out = File::create(&tmp).await?;

    let mut fd = match client.open(item.remote.clone(), AfcFopenMode::RdOnly).await {
        Ok(fd) => fd,
        Err(e) => {
            drop(out);
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(e.into());
        }
    };
    let res: Result<u64> = async {
        let mut total = 0u64;
        loop {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let chunk = fd.read_n(COPY_CHUNK).await?;
            if chunk.is_empty() {
                break;
            }
            out.write_all(&chunk).await?;
            total += chunk.len() as u64;
            let _ = prog.send(TransferEvent::FileProgress { bytes_delta: chunk.len() as u64 }).await;
        }
        out.flush().await?;
        Ok(total)
    }
    .await;
    let _ = fd.close().await;
    drop(out);

    let bytes = match res {
        Ok(b) if item.expected_size == 0 || b == item.expected_size => b,
        Ok(b) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(Error::Other(format!(
                "size mismatch: expected {} got {b}",
                item.expected_size
            )));
        }
        Err(e) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(e);
        }
    };
    tokio::fs::rename(&tmp, &item.dst).await?;
    Ok(bytes)
}

#[async_trait]
impl PhotoSource for AfcSource {
    async fn device_info(&mut self) -> Result<DeviceInfo> {
        Ok(self.info.clone())
    }

    async fn list(&mut self, dir: &str) -> Result<Vec<SourceEntry>> {
        let names = self.primary.list_dir(dir.to_string()).await?;
        let mut out = Vec::with_capacity(names.len());
        for n in names {
            if n == "." || n == ".." {
                continue;
            }
            let full = join_posix(dir, &n);
            let info = self.primary.get_file_info_raw(full.clone()).await?;
            out.push(entry_from(&full, &info));
        }
        Ok(out)
    }

    async fn walk(&mut self, root: &str) -> Result<Vec<SourceEntry>> {
        self.walk_parallel(root).await
    }

    async fn copy_file(&mut self, remote: &str, dst: &Path) -> Result<u64> {
        let (tx, rx) = async_channel::unbounded();
        let item = CopyItem { remote: remote.to_string(), expected_size: 0, dst: dst.to_path_buf() };
        let r = copy_one(&mut self.primary, &item, &CancelFlag::new(), &tx).await;
        drop(rx);
        r
    }

    async fn remove_verified(&mut self, remote: &str, expected_size: u64) -> Result<()> {
        let info = self.primary.get_file_info_raw(remote.to_string()).await?;
        let actual = entry_from(remote, &info).size;
        if actual != expected_size {
            return Err(Error::Other(format!(
                "size mismatch on {remote}: expected {expected_size}, iPhone reports {actual}"
            )));
        }
        self.primary.remove(remote.to_string()).await?;
        Ok(())
    }

    async fn copy_many(
        &mut self,
        items: Vec<CopyItem>,
        cancel: &CancelFlag,
        on_event: EventSink<'_>,
    ) -> Result<Vec<CopyResult>> {
        let n = self.concurrency.min(items.len().max(1));
        let (work_tx, work_rx) = async_channel::bounded::<CopyItem>(n * 4);
        let (res_tx, res_rx) = async_channel::bounded::<CopyResult>(n * 4);
        let (prog_tx, prog_rx) = async_channel::bounded::<TransferEvent>(n * 32);

        let mut handles = Vec::with_capacity(n);
        for _ in 0..n {
            let mut client = self.open_client().await?;
            let (work_rx, res_tx, prog_tx, cancel) =
                (work_rx.clone(), res_tx.clone(), prog_tx.clone(), cancel.clone());
            handles.push(tokio::spawn(async move {
                while let Ok(item) = work_rx.recv().await {
                    if cancel.is_cancelled() {
                        break;
                    }
                    let r = copy_one(&mut client, &item, &cancel, &prog_tx).await;
                    let (bytes, error) = match r {
                        Ok(b) => {
                            let _ = prog_tx
                                .send(TransferEvent::FileDone { path: item.remote.clone(), size: b })
                                .await;
                            (b, None)
                        }
                        Err(Error::Cancelled) => break,
                        Err(e) => {
                            let _ = prog_tx
                                .send(TransferEvent::FileError {
                                    path: item.remote.clone(),
                                    error: e.to_string(),
                                })
                                .await;
                            (0, Some(e.to_string()))
                        }
                    };
                    let res = CopyResult { remote: item.remote, dst: item.dst, bytes, error };
                    if res_tx.send(res).await.is_err() {
                        break;
                    }
                }
            }));
        }
        drop(work_rx);
        drop(res_tx);
        drop(prog_tx);

        let cancel_feed = cancel.clone();
        let feeder = tokio::spawn(async move {
            for it in items {
                if cancel_feed.is_cancelled() || work_tx.send(it).await.is_err() {
                    break;
                }
            }
        });

        let mut results = Vec::new();
        let (mut prog_done, mut res_done) = (false, false);
        while !(prog_done && res_done) {
            tokio::select! {
                p = prog_rx.recv(), if !prog_done => match p {
                    Ok(ev) => on_event(ev),
                    Err(_) => prog_done = true,
                },
                r = res_rx.recv(), if !res_done => match r {
                    Ok(res) => results.push(res),
                    Err(_) => res_done = true,
                },
            }
        }
        let _ = feeder.await;
        for h in handles {
            let _ = h.await;
        }
        Ok(results)
    }
}

fn entry_from(path: &str, info: &HashMap<String, String>) -> SourceEntry {
    let size = info.get("st_size").and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    let is_dir = info.get("st_ifmt").map(|v| v == "S_IFDIR").unwrap_or(false);
    // AFC reports st_mtime in nanoseconds.
    let mtime = info
        .get("st_mtime")
        .and_then(|v| v.parse::<i128>().ok())
        .map(|ns| (ns / 1_000_000_000) as i64);
    SourceEntry { path: path.to_string(), size, mtime, is_dir }
}

fn join_posix(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

pub(crate) fn tmp_path(dst: &Path) -> PathBuf {
    let mut s = dst.as_os_str().to_os_string();
    s.push(".photoport.part");
    PathBuf::from(s)
}

