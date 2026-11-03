use photoport_core::smart::{SmartIndex, TermKind};
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

fn tempdir() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let d = std::env::temp_dir().join(format!(
        "photoport-smart-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// UUID string -> the two little-endian halves psi.sqlite stores.
fn halves(uuid: &str) -> (i64, i64) {
    let hex = uuid.replace('-', "");
    let bytes: Vec<u8> = (0..16).map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap()).collect();
    (
        i64::from_le_bytes(bytes[..8].try_into().unwrap()),
        i64::from_le_bytes(bytes[8..].try_into().unwrap()),
    )
}

fn fake_phone_dbs(dir: &Path) -> (PathBuf, PathBuf) {
    let photos = dir.join("Photos.sqlite");
    let db = Connection::open(&photos).unwrap();
    db.execute_batch("CREATE TABLE ZASSET (Z_PK INTEGER PRIMARY KEY, ZUUID TEXT, ZDIRECTORY TEXT, ZFILENAME TEXT);").unwrap();
    let assets = [
        ("B5917354-E792-48BF-A0BF-8F9076FE656A", "DCIM/100APPLE", "IMG_0014.MOV"),
        ("91EF474E-3766-43E5-BCF3-50221E2E498B", "DCIM/100APPLE", "IMG_0016.HEIC"),
        ("EF37CF2C-435F-4AB4-B9A2-4C20F76CE220", "DCIM/101APPLE", "IMG_0200.HEIC"),
        // Not on the phone's DCIM (shared album): must be ignored.
        ("11111111-2222-3333-4444-555555555555", "CPLAssets/group", "X.heic"),
    ];
    for (u, d, f) in assets {
        db.execute("INSERT INTO ZASSET (ZUUID, ZDIRECTORY, ZFILENAME) VALUES (?1, ?2, ?3)", params![u, d, f]).unwrap();
    }

    let psi = dir.join("psi.sqlite");
    let db = Connection::open(&psi).unwrap();
    db.execute_batch(
        "CREATE TABLE groups(category INT2, owning_groupid INT, content_string TEXT, normalized_string TEXT, lookup_identifier TEXT, score REAL, token_ranges_0 INT8, token_ranges_1 INT8);
         CREATE TABLE assets(uuid_0 INT, uuid_1 INT, creationDate INT);
         CREATE TABLE ga(groupid INT, assetid INT);",
    )
    .unwrap();
    for (u, _, _) in assets {
        let (a, b) = halves(u);
        db.execute("INSERT INTO assets (uuid_0, uuid_1, creationDate) VALUES (?1, ?2, 0)", params![a, b]).unwrap();
    }
    // rowids: 1 = IMG_0014, 2 = IMG_0016, 3 = IMG_0200, 4 = shared X.heic
    let groups: [(i64, &str, &[i64]); 6] = [
        (12, "Guadeloupe\0", &[1, 2, 4]),
        (1010, "Guadeloupe\0", &[2]),
        (1500, "Plage\0", &[2, 3]),
        (1500, "Vélo\0", &[3]),
        (3, "New York\0", &[3]),
        (1101, "2025\0", &[1]),
    ];
    for (cat, label, ids) in groups {
        db.execute("INSERT INTO groups (category, content_string) VALUES (?1, ?2)", params![cat, label]).unwrap();
        let gid = db.last_insert_rowid();
        for id in ids {
            db.execute("INSERT INTO ga (groupid, assetid) VALUES (?1, ?2)", params![gid, id]).unwrap();
        }
    }
    (psi, photos)
}

#[test]
fn finds_places_and_scenes_by_dcim_path() {
    let dir = tempdir();
    let (psi, photos) = fake_phone_dbs(&dir);
    let idx = SmartIndex::build_from_files(&psi, &photos).unwrap();

    assert_eq!(idx.search("guadeloupe"), vec!["/DCIM/100APPLE/IMG_0014.MOV", "/DCIM/100APPLE/IMG_0016.HEIC"]);
    assert_eq!(idx.search("GUADEL"), idx.search("guadeloupe"), "prefix, any case");
    assert_eq!(idx.search("velo"), vec!["/DCIM/101APPLE/IMG_0200.HEIC"], "accents ignored");
    assert_eq!(idx.search("vélo"), idx.search("velo"));
    assert_eq!(idx.search("plage guadeloupe"), vec!["/DCIM/100APPLE/IMG_0016.HEIC"], "words are ANDed");
    assert_eq!(idx.search("new york"), vec!["/DCIM/101APPLE/IMG_0200.HEIC"]);
    assert_eq!(idx.search("york"), vec!["/DCIM/101APPLE/IMG_0200.HEIC"]);
    assert!(idx.search("tokyo").is_empty());
    assert!(idx.search("  ").is_empty());

    let sug = idx.suggestions(5);
    let place = sug.iter().find(|s| s.label == "Guadeloupe" && s.kind == TermKind::Place).unwrap();
    assert_eq!(place.count, 2, "shared-album asset not counted");
    assert!(sug.iter().any(|s| s.label == "Plage" && s.kind == TermKind::Scene));
    assert!(!sug.iter().any(|s| s.label == "2025"), "dates are not suggested");
}

#[test]
fn missing_asset_table_is_an_error() {
    let dir = tempdir();
    let (psi, _) = fake_phone_dbs(&dir);
    let empty = dir.join("empty.sqlite");
    Connection::open(&empty).unwrap().execute_batch("CREATE TABLE t(x);").unwrap();
    assert!(SmartIndex::build_from_files(&psi, &empty).is_err());
}

/// Real copies from an iPhone: PHOTOPORT_SMART_DIR=<dir with psi.sqlite and Photos.sqlite>.
#[test]
#[ignore]
fn real_phone_databases() {
    let dir = PathBuf::from(std::env::var("PHOTOPORT_SMART_DIR").expect("PHOTOPORT_SMART_DIR"));
    let t = std::time::Instant::now();
    let idx = SmartIndex::build_from_files(&dir.join("psi.sqlite"), &dir.join("Photos.sqlite")).unwrap();
    println!("built in {:?}", t.elapsed());
    for q in ["guadeloupe", "plage", "velo", "bruxelles", "chien", "new york"] {
        let r = idx.search(q);
        println!("{q}: {} {:?}", r.len(), r.iter().take(3).collect::<Vec<_>>());
    }
    for s in idx.suggestions(8) {
        println!("{:?} {} ({})", s.kind, s.label, s.count);
    }
    assert!(!idx.search("guadeloupe").is_empty());
}
