use photoport_core::datemeta::capture_date;
use photoport_core::layout::utc_to_wall_clock;

fn mp4_with_mvhd(creation_unix: i64) -> Vec<u8> {
    let qt = (creation_unix + 2_082_844_800) as u32;
    let mut mvhd_payload = vec![0u8, 0, 0, 0]; // version 0 + flags
    mvhd_payload.extend_from_slice(&qt.to_be_bytes());
    mvhd_payload.extend_from_slice(&[0u8; 16]);
    let mvhd = [((8 + mvhd_payload.len()) as u32).to_be_bytes().to_vec(), b"mvhd".to_vec(), mvhd_payload].concat();
    let moov = [((8 + mvhd.len()) as u32).to_be_bytes().to_vec(), b"moov".to_vec(), mvhd].concat();
    let ftyp = [20u32.to_be_bytes().to_vec(), b"ftypqt  ".to_vec(), vec![0; 8]].concat();
    [ftyp, moov].concat()
}

fn tmp(name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("photoport-dm-{}-{name}", std::process::id()));
    std::fs::write(&p, bytes).unwrap();
    p
}

#[tokio::test]
async fn reads_quicktime_creation_time() {
    let p = tmp("a.MOV", &mp4_with_mvhd(1_705_320_000));
    assert_eq!(capture_date(&p).await, Some(utc_to_wall_clock(1_705_320_000)));
}

#[tokio::test]
async fn garbage_files_yield_none() {
    assert_eq!(capture_date(&tmp("b.JPG", &[0xFF; 64])).await, None);
    assert_eq!(capture_date(&tmp("c.MOV", &[0x00; 3])).await, None);
    assert_eq!(capture_date(&tmp("d.AAE", b"<plist/>")).await, None);
}

/// `PHOTOPORT_SAMPLE_VIDEO=/path/to/real.MOV cargo test -- --ignored real_video`
#[tokio::test]
#[ignore]
async fn real_video_has_capture_date() {
    let p = std::path::PathBuf::from(std::env::var("PHOTOPORT_SAMPLE_VIDEO").expect("set PHOTOPORT_SAMPLE_VIDEO"));
    let d = capture_date(&p).await.expect("video date");
    println!("{} -> {}", p.display(), chrono::DateTime::from_timestamp(d, 0).unwrap());
}
