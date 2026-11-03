//! Groups files that belong together on the phone: a Live Photo is
//! `IMG_0001.HEIC` + `IMG_0001.MOV` (+ optional `IMG_0001.AAE` edit sidecar)
//! in the same DCIM folder.

use crate::layout::basename;
use crate::scan::PlanItem;
use std::collections::HashMap;

pub fn is_image(path: &str) -> bool {
    has_ext(path, &["heic", "heif", "jpg", "jpeg", "png", "dng", "tif", "tiff", "gif"])
}

pub fn is_video(path: &str) -> bool {
    has_ext(path, &["mov", "mp4", "m4v", "avi", "qt"])
}

fn has_ext(path: &str, exts: &[&str]) -> bool {
    path.rsplit_once('.')
        .map(|(_, e)| exts.iter().any(|x| e.eq_ignore_ascii_case(x)))
        .unwrap_or(false)
}

/// `/DCIM/100APPLE/IMG_0001.HEIC` -> `/DCIM/100APPLE/IMG_0001`, case-folded.
pub fn group_key(path: &str) -> String {
    let parent = path.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
    let name = basename(path);
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
    format!("{parent}/{}", stem.to_ascii_uppercase())
}

/// Groups in first-seen order; every item lands in exactly one group.
pub fn group_by<T: Clone>(items: &[T], path: impl Fn(&T) -> &str) -> Vec<Vec<T>> {
    let mut groups: Vec<Vec<T>> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for it in items {
        let key = group_key(path(it));
        match index.get(&key) {
            Some(&i) => groups[i].push(it.clone()),
            None => {
                index.insert(key, groups.len());
                groups.push(vec![it.clone()]);
            }
        }
    }
    groups
}

pub fn group(plan: &[PlanItem]) -> Vec<Vec<PlanItem>> {
    group_by(plan, |it| it.entry.path.as_str())
}
