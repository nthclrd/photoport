use photoport_core::import::{error_log_path, run_import, unique_destination, ImportOptions};
use photoport_core::source::mock::MockSource;
use photoport_core::{CancelFlag, TransferEvent};
use std::path::{Path, PathBuf};

// Noon UTC on the 15th: same month whatever the machine's time zone.
const JAN_15: i64 = 1_705_320_000; // 2024-01-15 12:00 UTC
const MAR_15: i64 = 1_710_504_000; // 2024-03-15 12:00 UTC

fn tempdir() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let d = std::env::temp_dir().join(format!(
        "photoport-import-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn phone() -> MockSource {
    MockSource::builder()
        .with_file("/DCIM/100APPLE/IMG_0001.HEIC", vec![1; 3000], JAN_15)
        .with_file("/DCIM/100APPLE/IMG_0001.MOV", vec![2; 5000], MAR_15)
        .with_file("/DCIM/100APPLE/IMG_0002.JPG", vec![3; 700], MAR_15)
        .with_file("/DCIM/101APPLE/IMG_0003.MP4", vec![4; 9000], JAN_15)
}

async fn import(src: &mut MockSource, dst: &Path, opts: ImportOptions) -> photoport_core::import::ImportReport {
    let mut sink = |_: TransferEvent| {};
    run_import(src, dst, &opts, &CancelFlag::new(), &mut sink).await.unwrap()
}

fn files_under(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.file_name().unwrap() == ".photoport" {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out.sort();
    out
}

#[tokio::test]
async fn imports_everything_into_french_month_folders() {
    let dst = tempdir();
    let mut src = phone();
    let r = import(&mut src, &dst, ImportOptions::default()).await;
    assert_eq!((r.ok, r.failed), (4, 0));
    assert_eq!(r.bytes, 3000 + 5000 + 700 + 9000);
    assert_eq!(
        files_under(&dst),
        vec![
            "2024/01 - Janvier/IMG_0001.HEIC",
            "2024/01 - Janvier/IMG_0001.MOV",
            "2024/01 - Janvier/IMG_0003.MP4",
            "2024/03 - Mars/IMG_0002.JPG",
        ]
    );
    assert_eq!(std::fs::read(dst.join("2024/03 - Mars/IMG_0002.JPG")).unwrap(), vec![3; 700]);
    assert!(!dst.join(".photoport/staging").exists(), "staging must be cleaned");
}

#[tokio::test]
async fn live_photo_video_follows_its_photo() {
    let dst = tempdir();
    let mut src = phone();
    import(&mut src, &dst, ImportOptions::default()).await;
    // MOV is dated March on the phone but belongs with the January HEIC.
    assert!(dst.join("2024/01 - Janvier/IMG_0001.MOV").exists());
    assert!(!dst.join("2024/03 - Mars/IMG_0001.MOV").exists());
}

#[tokio::test]
async fn second_run_copies_nothing() {
    let dst = tempdir();
    let mut src = phone();
    import(&mut src, &dst, ImportOptions::default()).await;
    let r = import(&mut src, &dst, ImportOptions::default()).await;
    assert_eq!((r.ok, r.failed), (0, 0));
    assert_eq!(files_under(&dst).len(), 4, "no duplicates");
}

#[tokio::test]
async fn failed_file_is_retried_next_run_and_logged() {
    let dst = tempdir();
    let mut src = phone().with_failure("/DCIM/100APPLE/IMG_0002.JPG");
    let r = import(&mut src, &dst, ImportOptions::default()).await;
    assert_eq!((r.ok, r.failed), (3, 1));
    let log = std::fs::read_to_string(error_log_path(&dst)).unwrap();
    assert!(log.contains("IMG_0002.JPG") && log.contains("simulated USB disconnect"), "log was: {log:?}");
    assert!(!files_under(&dst).iter().any(|f| f.ends_with(".part")));

    src.clear_failures();
    let r = import(&mut src, &dst, ImportOptions::default()).await;
    assert_eq!((r.ok, r.failed), (1, 0), "only the failed file is copied again");
    assert_eq!(files_under(&dst).len(), 4);
}

#[tokio::test]
async fn delete_after_never_splits_a_live_photo() {
    let dst = tempdir();
    let mut src = phone().with_failure("/DCIM/100APPLE/IMG_0001.MOV");
    let opts = ImportOptions { delete_after: true, ..Default::default() };
    let r = import(&mut src, &dst, opts).await;
    assert_eq!(r.deleted, 2, "JPG + MP4 deleted");
    assert!(src.contains("/DCIM/100APPLE/IMG_0001.HEIC"), "HEIC kept: its MOV failed");
    assert!(src.contains("/DCIM/100APPLE/IMG_0001.MOV"));
    assert!(!src.contains("/DCIM/100APPLE/IMG_0002.JPG"));
    assert!(!src.contains("/DCIM/101APPLE/IMG_0003.MP4"));
}

#[tokio::test]
async fn delete_is_off_by_default() {
    let dst = tempdir();
    let mut src = phone();
    let r = import(&mut src, &dst, ImportOptions::default()).await;
    assert_eq!(r.deleted, 0);
    assert!(src.contains("/DCIM/100APPLE/IMG_0002.JPG"));
}

#[tokio::test]
async fn cancel_before_start_imports_nothing() {
    let dst = tempdir();
    let mut src = phone();
    let cancel = CancelFlag::new();
    cancel.cancel();
    let mut sink = |_: TransferEvent| {};
    let r = run_import(&mut src, &dst, &ImportOptions::default(), &cancel, &mut sink).await.unwrap();
    assert!(r.cancelled);
    assert_eq!(r.ok, 0);
    assert!(files_under(&dst).is_empty());
}

#[tokio::test]
async fn limit_keeps_live_photo_pairs_together() {
    let dst = tempdir();
    let mut src = phone();
    let r = import(&mut src, &dst, ImportOptions { limit: 1, ..Default::default() }).await;
    assert_eq!(r.ok, 2, "HEIC + its MOV, not half a pair");
}

#[tokio::test]
async fn events_report_scan_and_progress() {
    let dst = tempdir();
    let mut src = phone();
    let mut new_files = 0;
    let mut progressed = 0u64;
    let mut sink = |ev: TransferEvent| match ev {
        TransferEvent::Scanned { new_files: n, .. } => new_files = n,
        TransferEvent::FileProgress { bytes_delta } => progressed += bytes_delta,
        _ => {}
    };
    run_import(&mut src, &dst, &ImportOptions::default(), &CancelFlag::new(), &mut sink).await.unwrap();
    assert_eq!(new_files, 4);
    assert_eq!(progressed, 3000 + 5000 + 700 + 9000);
}

#[test]
fn unique_destination_suffixes_on_collision() {
    let dst = tempdir();
    let base = dst.join("IMG_0001.HEIC");
    assert_eq!(unique_destination(&base, "/DCIM/100APPLE/IMG_0001.HEIC"), base);
    std::fs::write(&base, b"x").unwrap();
    let other = unique_destination(&base, "/DCIM/105APPLE/IMG_0001.HEIC");
    assert_ne!(other, base);
    assert!(other.to_string_lossy().ends_with(".HEIC"));
}
