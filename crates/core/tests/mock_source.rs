use photoport_core::source::mock::MockSource;
use photoport_core::PhotoSource;

fn sample() -> MockSource {
    MockSource::builder()
        .with_file("/DCIM/100APPLE/IMG_0001.HEIC", vec![1u8; 2048], 1_700_000_000)
        .with_file("/DCIM/100APPLE/IMG_0001.MOV", vec![2u8; 4096], 1_700_000_000)
        .with_file("/DCIM/100APPLE/IMG_0002.JPG", vec![3u8; 512], 1_700_000_100)
        .with_file("/DCIM/101APPLE/VID_0001.MP4", vec![4u8; 8192], 1_700_001_000)
}

#[tokio::test]
async fn device_info_round_trip() {
    let mut src = sample();
    let info = src.device_info().await.unwrap();
    assert_eq!(info.name, "Mock iPhone");
    assert_eq!(info.udid, "00000000-MOCKUDIDMOCK");
}

#[tokio::test]
async fn list_top_level_dcim() {
    let mut src = sample();
    let items = src.list("/DCIM").await.unwrap();
    let names: Vec<_> = items.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(names, vec!["/DCIM/100APPLE", "/DCIM/101APPLE"]);
    assert!(items.iter().all(|e| e.is_dir));
}

#[tokio::test]
async fn walk_returns_only_files_recursively() {
    let mut src = sample();
    let mut files = src.walk("/DCIM").await.unwrap();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let paths: Vec<_> = files.iter().map(|e| e.path.clone()).collect();
    assert_eq!(
        paths,
        vec![
            "/DCIM/100APPLE/IMG_0001.HEIC",
            "/DCIM/100APPLE/IMG_0001.MOV",
            "/DCIM/100APPLE/IMG_0002.JPG",
            "/DCIM/101APPLE/VID_0001.MP4",
        ]
    );
    assert!(files.iter().all(|e| !e.is_dir));
    assert_eq!(files.iter().map(|e| e.size).sum::<u64>(), 2048 + 4096 + 512 + 8192);
}

#[tokio::test]
async fn copy_file_writes_exact_bytes_and_atomic_rename() {
    let mut src = sample();
    let tmp = tempdir_in_target();
    let dst = tmp.join("out/IMG_0001.HEIC");

    let n = src
        .copy_file("/DCIM/100APPLE/IMG_0001.HEIC", &dst)
        .await
        .unwrap();
    assert_eq!(n, 2048);
    let on_disk = std::fs::read(&dst).unwrap();
    assert_eq!(on_disk.len(), 2048);
    assert!(on_disk.iter().all(|&b| b == 1));

    // The .part file must not survive a successful copy.
    let part = {
        let mut s = dst.as_os_str().to_os_string();
        s.push(".photoport.part");
        std::path::PathBuf::from(s)
    };
    assert!(!part.exists(), "temp .part file must be renamed away");
}

#[tokio::test]
async fn copy_missing_file_errors() {
    let mut src = sample();
    let tmp = tempdir_in_target();
    let err = src
        .copy_file("/DCIM/does_not_exist.HEIC", &tmp.join("x.heic"))
        .await
        .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("no such file"), "got: {msg}");
}

#[tokio::test]
async fn copy_dir_errors() {
    let mut src = sample();
    let tmp = tempdir_in_target();
    let err = src
        .copy_file("/DCIM/100APPLE", &tmp.join("x"))
        .await
        .unwrap_err();
    assert!(format!("{err}").contains("is a directory"));
}

/// Returns a fresh temp directory under `target/` so parallel test runs do
/// not race on filesystem state. Not using the `tempfile` crate to keep the
/// core dep tree minimal in step 2.
fn tempdir_in_target() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static CTR: AtomicU64 = AtomicU64::new(0);
    let n = CTR.fetch_add(1, Ordering::SeqCst);
    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("photoport-test-{pid}-{n}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
