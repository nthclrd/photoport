//! In-memory `PhotoSource` for tests: no iPhone needed.

use super::{DeviceInfo, PhotoSource, SourceEntry};
use crate::{Error, Result};
use async_trait::async_trait;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone)]
enum Node {
    Dir,
    File { bytes: Vec<u8>, mtime: i64 },
}

#[derive(Debug, Clone)]
pub struct MockSource {
    info: DeviceInfo,
    tree: BTreeMap<String, Node>,
    failing: HashSet<String>,
}

impl Default for MockSource {
    fn default() -> Self {
        let mut s = Self {
            info: DeviceInfo {
                name: "Mock iPhone".into(),
                model: "iPhone-Mock".into(),
                ios_version: "99.0".into(),
                udid: "00000000-MOCKUDIDMOCK".into(),
            },
            tree: BTreeMap::new(),
            failing: HashSet::new(),
        };
        s.insert_dir("/DCIM");
        s.insert_dir("/DCIM/100APPLE");
        s
    }
}

impl MockSource {
    pub fn builder() -> Self {
        Self::default()
    }

    pub fn insert_dir(&mut self, path: &str) -> &mut Self {
        self.tree.insert(path.to_string(), Node::Dir);
        self
    }

    pub fn insert_file(&mut self, path: &str, bytes: Vec<u8>, mtime: i64) -> &mut Self {
        if let Some(parent) = parent_of(path) {
            if !self.tree.contains_key(&parent) {
                self.insert_dir(&parent);
            }
        }
        self.tree.insert(path.to_string(), Node::File { bytes, mtime });
        self
    }

    pub fn with_file(mut self, path: &str, bytes: Vec<u8>, mtime: i64) -> Self {
        self.insert_file(path, bytes, mtime);
        self
    }

    /// Make every copy of `path` fail, like a cable pulled mid-transfer.
    pub fn with_failure(mut self, path: &str) -> Self {
        self.failing.insert(path.to_string());
        self
    }

    pub fn clear_failures(&mut self) {
        self.failing.clear();
    }

    pub fn contains(&self, path: &str) -> bool {
        self.tree.contains_key(path)
    }
}

#[async_trait]
impl PhotoSource for MockSource {
    async fn device_info(&mut self) -> Result<DeviceInfo> {
        Ok(self.info.clone())
    }

    async fn list(&mut self, dir: &str) -> Result<Vec<SourceEntry>> {
        let dir = normalize(dir);
        if dir != "/" && !matches!(self.tree.get(&dir), Some(Node::Dir)) {
            return Err(Error::Other(format!("not a directory: {dir}")));
        }
        let prefix = if dir == "/" { "/".to_string() } else { format!("{dir}/") };
        let mut out = Vec::new();
        for (path, node) in self.tree.range(prefix.clone()..) {
            if !path.starts_with(&prefix) {
                break;
            }
            let rest = &path[prefix.len()..];
            if rest.is_empty() || rest.contains('/') {
                continue;
            }
            out.push(match node {
                Node::Dir => SourceEntry { path: path.clone(), size: 0, mtime: None, is_dir: true },
                Node::File { bytes, mtime } => SourceEntry {
                    path: path.clone(),
                    size: bytes.len() as u64,
                    mtime: Some(*mtime),
                    is_dir: false,
                },
            });
        }
        Ok(out)
    }

    async fn walk(&mut self, root: &str) -> Result<Vec<SourceEntry>> {
        let mut out = Vec::new();
        let mut stack = vec![normalize(root)];
        while let Some(cur) = stack.pop() {
            for it in self.list(&cur).await? {
                if it.is_dir {
                    stack.push(it.path.clone());
                } else {
                    out.push(it);
                }
            }
        }
        Ok(out)
    }

    async fn copy_file(&mut self, remote: &str, dst: &Path) -> Result<u64> {
        let remote = normalize(remote);
        if self.failing.contains(&remote) {
            return Err(Error::Idevice("simulated USB disconnect".into()));
        }
        let bytes = match self.tree.get(&remote) {
            Some(Node::File { bytes, .. }) => bytes.clone(),
            Some(Node::Dir) => return Err(Error::Other(format!("is a directory: {remote}"))),
            None => return Err(Error::Other(format!("no such file: {remote}"))),
        };
        if let Some(parent) = dst.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let tmp = crate::source::afc::tmp_path(dst);
        let mut f = tokio::fs::File::create(&tmp).await?;
        f.write_all(&bytes).await?;
        f.flush().await?;
        drop(f);
        tokio::fs::rename(&tmp, dst).await?;
        Ok(bytes.len() as u64)
    }

    async fn remove_verified(&mut self, remote: &str, expected_size: u64) -> Result<()> {
        let remote = normalize(remote);
        match self.tree.get(&remote) {
            Some(Node::File { bytes, .. }) if bytes.len() as u64 == expected_size => {
                self.tree.remove(&remote);
                Ok(())
            }
            Some(Node::File { bytes, .. }) => Err(Error::Other(format!(
                "size mismatch on {remote}: expected {expected_size}, device has {}",
                bytes.len()
            ))),
            _ => Err(Error::Other(format!("no such file: {remote}"))),
        }
    }
}

fn normalize(p: &str) -> String {
    if p.is_empty() || p == "/" {
        "/".into()
    } else {
        p.trim_end_matches('/').to_string()
    }
}

fn parent_of(path: &str) -> Option<String> {
    let path = normalize(path);
    if path == "/" {
        return None;
    }
    let idx = path.rfind('/')?;
    Some(if idx == 0 { "/".into() } else { path[..idx].to_string() })
}
