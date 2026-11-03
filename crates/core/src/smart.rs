//! Smart search over what iOS already recognised on the phone: places
//! (country, region, city, street, landmark), scenes (beach, bike, dog…),
//! seasons, holidays, text inside photos. Nothing runs a model here: the
//! iPhone's own Photos search index (`psi.sqlite`) is read and joined to
//! DCIM file names through the Photos library (`Photos.sqlite`).
//!
//! Both databases are undocumented Apple formats. Everything is best effort:
//! any schema mismatch becomes an error and the UI hides the feature.

use crate::source::afc::AfcSource;
use crate::{Error, PhotoSource, Result};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;

const PSI_DIR: &str = "/PhotoData/Caches/search";
const PHOTOS_DIR: &str = "/PhotoData";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TermKind {
    Place,
    Scene,
    Date,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Term {
    pub label: String,
    norm: String,
    pub kind: TermKind,
    /// Indexes into `SmartIndex::paths`.
    files: Vec<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SmartIndex {
    /// DCIM paths, e.g. `/DCIM/100APPLE/IMG_0014.MOV`.
    paths: Vec<String>,
    terms: Vec<Term>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Suggestion {
    pub label: String,
    pub kind: TermKind,
    pub count: usize,
}

/// Category numbers seen in psi.sqlite (iOS 17-26). Unknown ones stay searchable.
fn kind_of(category: i64) -> TermKind {
    match category {
        // 13: ISO country code ("BE"), searchable but not worth a chip.
        13 => TermKind::Other,
        1..=99 | 1000..=1099 | 1520 | 1700..=1799 => TermKind::Place,
        1500..=1519 | 1521..=1699 | 2500..=2699 | 2800 => TermKind::Scene,
        1100..=1199 => TermKind::Date,
        _ => TermKind::Other,
    }
}

/// Lowercase, no accents, single spaces: "Mer Méditerranée" -> "mer mediterranee".
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.trim_end_matches('\0').chars().flat_map(char::to_lowercase) {
        let base = match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' => 'a',
            'ç' | 'ć' | 'č' => 'c',
            'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' => 'e',
            'ì' | 'í' | 'î' | 'ï' | 'ī' => 'i',
            'ñ' | 'ń' => 'n',
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' => 'o',
            'ù' | 'ú' | 'û' | 'ü' | 'ū' => 'u',
            'ý' | 'ÿ' => 'y',
            'ś' | 'š' => 's',
            'ź' | 'ż' | 'ž' => 'z',
            'ł' => 'l',
            'ß' => 's',
            '’' | '\'' | '-' | '_' | ',' | '.' => ' ',
            c => c,
        };
        if base.is_whitespace() {
            if !out.ends_with(' ') && !out.is_empty() {
                out.push(' ');
            }
        } else {
            out.push(base);
        }
    }
    out.trim_end().to_string()
}

/// Changes whenever iOS rewrites either database: decides if a saved index is stale.
pub async fn device_version(src: &mut AfcSource) -> Result<String> {
    let mut parts = Vec::new();
    for (path, required) in [
        (format!("{PSI_DIR}/psi.sqlite"), true),
        (format!("{PSI_DIR}/psi.sqlite-wal"), false),
        (format!("{PHOTOS_DIR}/Photos.sqlite"), true),
        (format!("{PHOTOS_DIR}/Photos.sqlite-wal"), false),
    ] {
        // Recent changes live in the WAL until iOS checkpoints it.
        match src.stat(&path).await {
            Ok(e) => parts.push(format!("{}:{}", e.size, e.mtime.unwrap_or(0))),
            Err(e) if required => return Err(e),
            Err(_) => parts.push("-".into()),
        }
    }
    Ok(parts.join("|"))
}

impl SmartIndex {
    /// Copy both databases (with their WAL) into `workdir`, build, delete the copies.
    pub async fn build_from_device(src: &mut AfcSource, workdir: &Path) -> Result<Self> {
        tokio::fs::create_dir_all(workdir).await?;
        let copies = [
            (PSI_DIR, "psi.sqlite"),
            (PSI_DIR, "psi.sqlite-wal"),
            (PSI_DIR, "psi.sqlite-shm"),
            (PHOTOS_DIR, "Photos.sqlite"),
            (PHOTOS_DIR, "Photos.sqlite-wal"),
            (PHOTOS_DIR, "Photos.sqlite-shm"),
        ];
        let result = async {
            for (dir, name) in copies {
                let dst = workdir.join(name);
                let _ = tokio::fs::remove_file(&dst).await;
                let r = src.copy_file(&format!("{dir}/{name}"), &dst).await;
                // WAL and SHM may legitimately be missing; the main files may not.
                if r.is_err() && !name.contains('-') {
                    r?;
                }
            }
            let (psi, photos) = (workdir.join("psi.sqlite"), workdir.join("Photos.sqlite"));
            tokio::task::spawn_blocking(move || Self::build_from_files(&psi, &photos))
                .await
                .map_err(|e| Error::Other(e.to_string()))?
        }
        .await;
        for (_, name) in copies {
            let _ = tokio::fs::remove_file(workdir.join(name)).await;
        }
        result
    }

    pub fn build_from_files(psi: &Path, photos: &Path) -> Result<Self> {
        let photos = open(photos)?;
        let uuid_to_path = dcim_paths_by_uuid(&photos)?;
        drop(photos);

        let psi = open(psi)?;
        let mut asset_path: HashMap<i64, u32> = HashMap::new();
        let mut paths: Vec<String> = Vec::new();
        let mut path_index: HashMap<String, u32> = HashMap::new();
        {
            let mut stmt = psi.prepare("SELECT rowid, uuid_0, uuid_1 FROM assets").map_err(sq)?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))
                .map_err(sq)?;
            for row in rows {
                let (rowid, a, b) = row.map_err(sq)?;
                if let Some(p) = uuid_to_path.get(&uuid_from_halves(a, b)) {
                    let idx = *path_index.entry(p.clone()).or_insert_with(|| {
                        paths.push(p.clone());
                        (paths.len() - 1) as u32
                    });
                    asset_path.insert(rowid, idx);
                }
            }
        }

        let mut files_by_group: HashMap<i64, Vec<u32>> = HashMap::new();
        {
            let mut stmt = psi.prepare("SELECT groupid, assetid FROM ga").map_err(sq)?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))).map_err(sq)?;
            for row in rows {
                let (g, a) = row.map_err(sq)?;
                if let Some(&p) = asset_path.get(&a) {
                    files_by_group.entry(g).or_default().push(p);
                }
            }
        }

        // The same label exists once per category: merge them per (label, kind).
        let mut merged: HashMap<(String, TermKind), (String, HashSet<u32>)> = HashMap::new();
        {
            let mut stmt = psi.prepare("SELECT rowid, category, content_string FROM groups").map_err(sq)?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, Option<String>>(2)?))
                })
                .map_err(sq)?;
            for row in rows {
                let (gid, category, content) = row.map_err(sq)?;
                let (Some(files), Some(content)) = (files_by_group.get(&gid), content) else {
                    continue;
                };
                let label = content.trim_end_matches('\0').trim().to_string();
                let norm = normalize(&label);
                if norm.is_empty() {
                    continue;
                }
                let kind = kind_of(category);
                let slot = merged.entry((norm, kind)).or_insert_with(|| (label, HashSet::new()));
                slot.1.extend(files.iter().copied());
            }
        }

        let mut terms: Vec<Term> = merged
            .into_iter()
            .map(|((norm, kind), (label, files))| {
                let mut files: Vec<u32> = files.into_iter().collect();
                files.sort_unstable();
                Term { label, norm, kind, files }
            })
            .collect();
        terms.sort_by(|a, b| b.files.len().cmp(&a.files.len()).then_with(|| a.norm.cmp(&b.norm)));
        Ok(Self { paths, terms })
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// DCIM paths matching every word of the query (prefix match on any word
    /// of a label), or the whole query as a label prefix ("new york").
    pub fn search(&self, query: &str) -> Vec<String> {
        let q = normalize(query);
        if q.is_empty() {
            return Vec::new();
        }
        let mut hits: Option<HashSet<u32>> = None;
        for word in q.split(' ') {
            let mut word_hits = HashSet::new();
            for t in &self.terms {
                if t.norm.split(' ').any(|w| w.starts_with(word)) {
                    word_hits.extend(t.files.iter().copied());
                }
            }
            hits = Some(match hits {
                None => word_hits,
                Some(h) => h.intersection(&word_hits).copied().collect(),
            });
        }
        let mut hits = hits.unwrap_or_default();
        for t in self.terms.iter().filter(|t| t.norm.starts_with(&q)) {
            hits.extend(t.files.iter().copied());
        }
        let mut out: Vec<String> = hits.into_iter().map(|i| self.paths[i as usize].clone()).collect();
        out.sort();
        out
    }

    /// Most frequent places and scenes, for one-click chips.
    pub fn suggestions(&self, per_kind: usize) -> Vec<Suggestion> {
        let mut out = Vec::new();
        for kind in [TermKind::Place, TermKind::Scene] {
            out.extend(
                self.terms
                    .iter()
                    .filter(|t| t.kind == kind && t.label.chars().count() > 1)
                    .take(per_kind)
                    .map(|t| Suggestion { label: t.label.clone(), kind, count: t.files.len() }),
            );
        }
        out
    }
}

fn open(path: &Path) -> Result<Connection> {
    // Read-write on our own copy: SQLite needs the -shm to read a WAL database.
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(sq)
}

/// UUID string (as stored by Photos) -> `/DCIM/...` path.
fn dcim_paths_by_uuid(db: &Connection) -> Result<HashMap<String, String>> {
    // ZASSET since iOS 14, ZGENERICASSET before.
    let table = ["ZASSET", "ZGENERICASSET"]
        .into_iter()
        .find(|t| {
            db.query_row("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1", [t], |_| Ok(()))
                .is_ok()
        })
        .ok_or_else(|| Error::Other("Photos.sqlite: no asset table".into()))?;
    let sql = format!(
        "SELECT ZUUID, ZDIRECTORY, ZFILENAME FROM {table}
         WHERE ZDIRECTORY LIKE 'DCIM/%' AND ZUUID IS NOT NULL AND ZFILENAME IS NOT NULL"
    );
    let mut stmt = db.prepare(&sql).map_err(sq)?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))
        .map_err(sq)?;
    let mut out = HashMap::new();
    for row in rows {
        let (uuid, dir, name) = row.map_err(sq)?;
        out.insert(uuid.to_ascii_uppercase(), format!("/{}/{}", dir.trim_matches('/'), name));
    }
    Ok(out)
}

/// psi.sqlite stores a UUID as two little-endian signed 64-bit halves.
fn uuid_from_halves(a: i64, b: i64) -> String {
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&a.to_le_bytes());
    bytes[8..].copy_from_slice(&b.to_le_bytes());
    let h: String = bytes.iter().map(|x| format!("{x:02X}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

fn sq(e: rusqlite::Error) -> Error {
    Error::Other(format!("smart search: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_accents_and_case() {
        assert_eq!(normalize("Mer Méditerranée\0"), "mer mediterranee");
        assert_eq!(normalize("Vélo"), "velo");
        assert_eq!(normalize("Europe de l’Ouest"), "europe de l ouest");
        assert_eq!(normalize("  New   York "), "new york");
    }

    #[test]
    fn uuid_halves_are_little_endian() {
        // Pair observed on a real iPhone (iOS 26).
        let u = uuid_from_halves(-9204889558709347660, 2464502739989217213);
        assert_eq!(u.len(), 36);
        let back = u.replace('-', "");
        let a = i64::from_le_bytes(u64::from_str_radix(&back[..16], 16).unwrap().to_be_bytes());
        assert_eq!(a, -9204889558709347660);
    }
}
