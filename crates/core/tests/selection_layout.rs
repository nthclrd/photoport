use photoport_core::db::History;
use photoport_core::import::{history_path, run_import, ImportOptions, ImportReport, DCIM_ROOT};
use photoport_core::layout::{DateStyle, Lang};
use photoport_core::scan::{catalog, scan_files, ItemKind};
use photoport_core::source::mock::MockSource;
use photoport_core::{CancelFlag, TransferEvent};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

const JAN_15: i64 = 1_705_320_000; // 2024-01-15 12:00 UTC
const MAR_15: i64 = 1_710_504_000; // 2024-03-15 12:00 UTC

fn tempdir() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let d = std::env::temp_dir().join(format!(
        "photoport-sel-{}-{}",
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

async fn import(src: &mut MockSource, dst: &Path, opts: ImportOptions) -> ImportReport {
    let mut sink = |_: TransferEvent| {};
    run_import(src, dst, &opts, &CancelFlag::new(), &mut sink).await.unwrap()
}

fn only(ids: &[&str]) -> Option<HashSet<String>> {
    Some(ids.iter().map(|s| s.to_string()).collect())
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

#[test]
fn layout_keys_map_to_styles() {
    assert_eq!(DateStyle::from_key("month", Lang::Fr), Some(DateStyle::YearMonthNamed(Lang::Fr)));
    assert_eq!(DateStyle::from_key("day", Lang::Fr), Some(DateStyle::YearDay));
    assert_eq!(DateStyle::from_key("year", Lang::Fr), Some(DateStyle::Year));
    assert_eq!(DateStyle::from_key("flat", Lang::Fr), Some(DateStyle::Flat));
    assert_eq!(DateStyle::from_key("nope", Lang::Fr), None);
}

#[tokio::test]
async fn day_layout() {
    let dst = tempdir();
    let opts = ImportOptions { style: DateStyle::YearDay, ..Default::default() };
    import(&mut phone(), &dst, opts).await;
    assert_eq!(
        files_under(&dst),
        vec![
            "2024/2024-01-15/IMG_0001.HEIC",
            "2024/2024-01-15/IMG_0001.MOV",
            "2024/2024-01-15/IMG_0003.MP4",
            "2024/2024-03-15/IMG_0002.JPG",
        ]
    );
}

#[tokio::test]
async fn year_layout() {
    let dst = tempdir();
    let opts = ImportOptions { style: DateStyle::Year, ..Default::default() };
    import(&mut phone(), &dst, opts).await;
    assert_eq!(
        files_under(&dst),
        vec!["2024/IMG_0001.HEIC", "2024/IMG_0001.MOV", "2024/IMG_0002.JPG", "2024/IMG_0003.MP4"]
    );
}

#[tokio::test]
async fn flat_layout_keeps_same_name_files_and_live_pairs_together() {
    let dst = tempdir();
    // DCIM counter wrapped: same file names in two folders.
    let mut src = phone()
        .with_file("/DCIM/102APPLE/IMG_0001.HEIC", vec![7; 100], JAN_15)
        .with_file("/DCIM/102APPLE/IMG_0001.MOV", vec![8; 200], JAN_15);
    let opts = ImportOptions { style: DateStyle::Flat, ..Default::default() };
    let r = import(&mut src, &dst, opts).await;
    assert_eq!((r.ok, r.failed), (6, 0));

    let files = files_under(&dst);
    assert_eq!(files.len(), 6, "nothing overwritten: {files:?}");
    assert!(files.iter().all(|f| !f.contains('/')), "flat: {files:?}");
    let suffixed: Vec<&String> = files.iter().filter(|f| f.starts_with("IMG_0001_")).collect();
    assert_eq!(suffixed.len(), 2, "{files:?}");
    let stem = |f: &str| f.rsplit_once('.').unwrap().0.to_string();
    assert_eq!(stem(suffixed[0]), stem(suffixed[1]), "Live pair keeps one name: {files:?}");
}

#[tokio::test]
async fn only_selected_items_are_imported() {
    let dst = tempdir();
    let mut src = phone();
    let r = import(&mut src, &dst, ImportOptions { only: only(&["/DCIM/100APPLE/IMG_0002.JPG"]), ..Default::default() }).await;
    assert_eq!(r.ok, 1);
    assert_eq!(files_under(&dst), vec!["2024/03 - Mars/IMG_0002.JPG"]);

    // Selecting the photo of a Live Photo brings its video along.
    let r = import(&mut src, &dst, ImportOptions { only: only(&["/DCIM/100APPLE/IMG_0001.HEIC"]), ..Default::default() }).await;
    assert_eq!(r.ok, 2);
    assert_eq!(
        files_under(&dst),
        vec![
            "2024/01 - Janvier/IMG_0001.HEIC",
            "2024/01 - Janvier/IMG_0001.MOV",
            "2024/03 - Mars/IMG_0002.JPG",
        ]
    );

    // An empty selection copies nothing.
    let r = import(&mut src, &dst, ImportOptions { only: only(&[]), ..Default::default() }).await;
    assert_eq!(r.ok, 0);
}

#[tokio::test]
async fn live_video_left_behind_is_found_through_its_photo() {
    let dst = tempdir();
    let mut src = phone().with_failure("/DCIM/100APPLE/IMG_0001.MOV");
    let sel = || ImportOptions { only: only(&["/DCIM/100APPLE/IMG_0001.HEIC"]), ..Default::default() };
    let r = import(&mut src, &dst, sel()).await;
    assert_eq!((r.ok, r.failed), (1, 1));

    src.clear_failures();
    let r = import(&mut src, &dst, sel()).await;
    assert_eq!((r.ok, r.failed), (1, 0), "the missing video is retried");
    assert!(dst.join("2024/01 - Janvier/IMG_0001.MOV").exists());
}

#[tokio::test]
async fn catalog_groups_kinds_and_import_state() {
    let dst = tempdir();
    let mut src = phone();
    import(&mut src, &dst, ImportOptions { only: only(&["/DCIM/100APPLE/IMG_0002.JPG"]), ..Default::default() }).await;

    let scan = scan_files(&mut src, DCIM_ROOT).await.unwrap();
    let db = History::open(&history_path(&dst)).unwrap();
    let (items, summary) = catalog(&scan.info, scan.files, &db).unwrap();

    assert_eq!(items.len(), 3, "Live Photo is one item");
    assert_eq!(summary.new_files, 3);
    let get = |id: &str| items.iter().find(|i| i.id == id).unwrap();
    let live = get("/DCIM/100APPLE/IMG_0001.HEIC");
    assert_eq!(live.kind, ItemKind::Live);
    assert_eq!(live.files.len(), 2);
    assert_eq!(live.size, 8000);
    assert!(!live.imported);
    assert_eq!(get("/DCIM/101APPLE/IMG_0003.MP4").kind, ItemKind::Video);
    let jpg = get("/DCIM/100APPLE/IMG_0002.JPG");
    assert_eq!(jpg.kind, ItemKind::Photo);
    assert!(jpg.imported);
    // Newest first.
    assert_eq!(items[0].id, "/DCIM/100APPLE/IMG_0002.JPG");
}
