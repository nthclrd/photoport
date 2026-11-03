use photoport_core::db::{History, TransferKey, TransferRecord};
use photoport_core::layout::{folder_from_epoch, DateStyle, Lang};
use photoport_core::scan::build_plan;
use photoport_core::source::mock::MockSource;
use std::path::Path;

fn sample() -> MockSource {
    MockSource::builder()
        .with_file("/DCIM/100APPLE/IMG_0001.HEIC", vec![1u8; 2048], 1_700_000_000)
        .with_file("/DCIM/100APPLE/IMG_0001.MOV", vec![2u8; 4096], 1_700_000_000)
        .with_file("/DCIM/100APPLE/IMG_0002.JPG", vec![3u8; 512], 1_700_000_100)
}

#[test]
fn layout_fr_names_month() {
    // 2024-01-15 10:00:00 UTC => epoch 1705_312_800
    let at = |style| folder_from_epoch(1_705_312_800, style);
    assert_eq!(at(DateStyle::YearMonthNamed(Lang::Fr)), Path::new("2024").join("01 - Janvier"));
    assert_eq!(at(DateStyle::YearMonthNamed(Lang::En)), Path::new("2024").join("01 - January"));
    assert_eq!(at(DateStyle::YearDashMonth), Path::new("2024-01"));
    assert_eq!(at(DateStyle::Flat), Path::new(""));
}

#[tokio::test]
async fn fresh_db_marks_everything_new() {
    let mut src = sample();
    let db = History::in_memory().unwrap();
    let (plan, summary) = build_plan(&mut src, &db, "/DCIM").await.unwrap();
    assert_eq!(summary.total_files, 3);
    assert_eq!(summary.new_files, 3);
    assert_eq!(summary.skipped_already_done, 0);
    assert_eq!(plan.len(), 3);
}

#[tokio::test]
async fn second_scan_skips_recorded_files() {
    let mut src = sample();
    let db = History::in_memory().unwrap();
    let (plan, _) = build_plan(&mut src, &db, "/DCIM").await.unwrap();
    for it in &plan {
        db.record(&TransferRecord {
            key: it.key.clone(),
            local_path: format!("/dst/{}", it.entry.path),
            transferred_at: 42,
        })
        .unwrap();
    }
    let (plan2, summary2) = build_plan(&mut src, &db, "/DCIM").await.unwrap();
    assert_eq!(plan2.len(), 0);
    assert_eq!(summary2.skipped_already_done, 3);
    assert_eq!(summary2.new_files, 0);
}

#[tokio::test]
async fn changed_size_counts_as_new() {
    let mut src = sample();
    let db = History::in_memory().unwrap();
    // Record a stale entry: same path, different size.
    db.record(&TransferRecord {
        key: TransferKey {
            udid: "00000000-MOCKUDIDMOCK".into(),
            remote_path: "/DCIM/100APPLE/IMG_0001.HEIC".into(),
            size: 999,
            remote_mtime: Some(1_700_000_000),
        },
        local_path: "/dst/old.heic".into(),
        transferred_at: 1,
    })
    .unwrap();
    let (plan, summary) = build_plan(&mut src, &db, "/DCIM").await.unwrap();
    assert_eq!(summary.new_files, 3, "size mismatch must still show as new");
    assert!(plan.iter().any(|it| it.entry.path.ends_with("IMG_0001.HEIC")));
}

#[tokio::test]
async fn count_grows_with_records() {
    let mut src = sample();
    let db = History::in_memory().unwrap();
    assert_eq!(db.count("00000000-MOCKUDIDMOCK").unwrap(), 0);
    let (plan, _) = build_plan(&mut src, &db, "/DCIM").await.unwrap();
    for it in &plan {
        db.record(&TransferRecord {
            key: it.key.clone(),
            local_path: "/dst/x".into(),
            transferred_at: 1,
        })
        .unwrap();
    }
    assert_eq!(db.count("00000000-MOCKUDIDMOCK").unwrap(), 3);
}
