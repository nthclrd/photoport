//! HEIC → JPEG through tools the OS already has (see docs/HEIC_STRATEGY.md):
//! macOS `sips` (built in), Linux `heif-convert` (libheif-examples).
//! Both keep EXIF. Windows: not yet, planned with bundled libheif.

use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    #[default]
    Original,
    Jpg,
    Both,
}

#[derive(Debug, Clone, Copy)]
pub struct ConvertOptions {
    pub format: OutputFormat,
    /// 1..=100
    pub quality: u8,
}

impl Default for ConvertOptions {
    fn default() -> Self {
        Self { format: OutputFormat::Original, quality: 92 }
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    Sips,
    HeifConvert,
}

/// `None` when this machine cannot convert.
pub fn backend() -> Option<Backend> {
    match std::env::consts::OS {
        "macos" => Some(Backend::Sips),
        "linux" if in_path("heif-convert") => Some(Backend::HeifConvert),
        _ => None,
    }
}

fn in_path(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
        .unwrap_or(false)
}

pub fn is_heic(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("heic") || e.eq_ignore_ascii_case("heif"))
        .unwrap_or(false)
}

pub async fn to_jpeg(src: &Path, dst: &Path, quality: u8) -> Result<()> {
    let q = quality.clamp(1, 100).to_string();
    let mut cmd = match backend() {
        Some(Backend::Sips) => {
            let mut c = Command::new("sips");
            c.args(["-s", "format", "jpeg", "-s", "formatOptions", &q])
                .arg(src)
                .arg("--out")
                .arg(dst);
            c
        }
        Some(Backend::HeifConvert) => {
            let mut c = Command::new("heif-convert");
            c.args(["-q", &q]).arg(src).arg(dst);
            c
        }
        None => return Err(Error::Other("no HEIC converter on this system".into())),
    };
    let out = cmd.output().await?;
    if !out.status.success() || !dst.is_file() {
        let _ = tokio::fs::remove_file(dst).await;
        return Err(Error::Other(format!(
            "HEIC conversion failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}
