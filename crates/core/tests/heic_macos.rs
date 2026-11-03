#![cfg(target_os = "macos")]
//! Real conversions through `sips`, macOS only.

use photoport_core::datemeta::capture_date;
use photoport_core::heic::{to_jpeg, ConvertOptions, OutputFormat};
use photoport_core::import::{run_import, ImportOptions};
use photoport_core::source::mock::MockSource;
use photoport_core::{CancelFlag, TransferEvent};
use std::path::{Path, PathBuf};
use std::process::Command;

fn workdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("photoport-heic-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Builds a small HEIC with sips from a generated PNG.
fn sample_heic(dir: &Path) -> Vec<u8> {
    let png = dir.join("in.png");
    let heic = dir.join("in.heic");
    // 8x8 solid RGB PNG
    let raw: Vec<u8> = (0..8).flat_map(|_| std::iter::once(0u8).chain([200u8, 40, 40].repeat(8))).collect();
    let mut z = flate_store(&raw);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    out.extend(chunk(b"IHDR", &[0, 0, 0, 8, 0, 0, 0, 8, 8, 2, 0, 0, 0]));
    out.extend(chunk(b"IDAT", &z.split_off(0)));
    out.extend(chunk(b"IEND", &[]));
    std::fs::write(&png, out).unwrap();
    let ok = Command::new("sips")
        .args(["-s", "format", "heic"])
        .arg(&png)
        .arg("--out")
        .arg(&heic)
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "sips could not create a HEIC");
    std::fs::read(heic).unwrap()
}

fn chunk(t: &[u8; 4], d: &[u8]) -> Vec<u8> {
    let mut c = (d.len() as u32).to_be_bytes().to_vec();
    c.extend_from_slice(t);
    c.extend_from_slice(d);
    c.extend(crc32([&t[..], d].concat().as_slice()).to_be_bytes());
    c
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

/// zlib stream with one stored (uncompressed) deflate block.
fn flate_store(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01, 0x01];
    let len = data.len() as u16;
    out.extend(len.to_le_bytes());
    out.extend((!len).to_le_bytes());
    out.extend_from_slice(data);
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    out.extend(((b << 16) | a).to_be_bytes());
    out
}

#[tokio::test]
async fn sips_converts_heic_to_jpeg() {
    let dir = workdir("convert");
    let heic = dir.join("a.HEIC");
    std::fs::write(&heic, sample_heic(&dir)).unwrap();
    let jpg = dir.join("a.JPG");
    to_jpeg(&heic, &jpg, 80).await.unwrap();
    assert_eq!(&std::fs::read(&jpg).unwrap()[..2], &[0xFF, 0xD8], "JPEG magic");
}

async fn import_with(format: OutputFormat, tag: &str) -> Vec<String> {
    let dir = workdir(tag);
    let mut src = MockSource::builder().with_file("/DCIM/100APPLE/IMG_0001.HEIC", sample_heic(&dir), 1_705_320_000);
    let dst = dir.join("out");
    let opts = ImportOptions { convert: ConvertOptions { format, quality: 85 }, ..Default::default() };
    let mut sink = |_: TransferEvent| {};
    let r = run_import(&mut src, &dst, &opts, &CancelFlag::new(), &mut sink).await.unwrap();
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    let mut names: Vec<String> = std::fs::read_dir(dst.join("2024/01 - Janvier"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn format_jpg_only_replaces_heic() {
    assert_eq!(import_with(OutputFormat::Jpg, "jpg").await, vec!["IMG_0001.JPG"]);
}

#[tokio::test]
async fn format_both_keeps_both() {
    assert_eq!(import_with(OutputFormat::Both, "both").await, vec!["IMG_0001.HEIC", "IMG_0001.JPG"]);
}

#[tokio::test]
async fn format_original_untouched() {
    assert_eq!(import_with(OutputFormat::Original, "orig").await, vec!["IMG_0001.HEIC"]);
}

/// `PHOTOPORT_SAMPLE_HEIC=/path/to/real.HEIC cargo test -- --ignored exif`
#[tokio::test]
#[ignore]
async fn exif_date_survives_conversion() {
    let src = PathBuf::from(std::env::var("PHOTOPORT_SAMPLE_HEIC").expect("set PHOTOPORT_SAMPLE_HEIC"));
    let before = capture_date(&src).await.expect("sample has EXIF date");
    let dir = workdir("exif");
    let heic = dir.join("x.HEIC");
    std::fs::copy(&src, &heic).unwrap();
    let jpg = dir.join("x.JPG");
    to_jpeg(&heic, &jpg, 90).await.unwrap();
    assert_eq!(capture_date(&jpg).await, Some(before));
}
