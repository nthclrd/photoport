//! SQLite history of what has already been transferred.
//!
//! Identity key: (udid, remote_path, size, remote_mtime). If a future
//! collision is observed, step 4 adds a partial-hash column.

use crate::{Error, Result};
use rusqlite::{params, Connection};
use std::path::Path;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS schema_version (
    v INTEGER NOT NULL PRIMARY KEY
);

CREATE TABLE IF NOT EXISTS transferred (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    udid          TEXT    NOT NULL,
    remote_path   TEXT    NOT NULL,
    size          INTEGER NOT NULL,
    remote_mtime  INTEGER,
    local_path    TEXT    NOT NULL,
    transferred_at INTEGER NOT NULL,
    UNIQUE(udid, remote_path, size, remote_mtime)
);
CREATE INDEX IF NOT EXISTS idx_transferred_udid_path
    ON transferred(udid, remote_path);
"#;

const CURRENT_VERSION: i64 = 1;

pub struct History {
    conn: Connection,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TransferKey {
    pub udid: String,
    pub remote_path: String,
    pub size: u64,
    pub remote_mtime: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct TransferRecord {
    pub key: TransferKey,
    pub local_path: String,
    pub transferred_at: i64,
}

impl History {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path).map_err(sqerr)?;
        Self::init(conn)
    }

    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(sqerr)?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(SCHEMA).map_err(sqerr)?;
        let v: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(v), 0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .map_err(sqerr)?;
        if v < CURRENT_VERSION {
            conn.execute(
                "INSERT INTO schema_version (v) VALUES (?1)",
                params![CURRENT_VERSION],
            )
            .map_err(sqerr)?;
        }
        Ok(Self { conn })
    }

    pub fn is_transferred(&self, key: &TransferKey) -> Result<bool> {
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT 1 FROM transferred
                 WHERE udid = ?1 AND remote_path = ?2 AND size = ?3
                   AND (remote_mtime IS ?4 OR remote_mtime = ?4)
                 LIMIT 1",
            )
            .map_err(sqerr)?;
        let found = stmt
            .exists(params![
                key.udid,
                key.remote_path,
                key.size as i64,
                key.remote_mtime
            ])
            .map_err(sqerr)?;
        Ok(found)
    }

    pub fn record(&self, rec: &TransferRecord) -> Result<()> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO transferred
                 (udid, remote_path, size, remote_mtime, local_path, transferred_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    rec.key.udid,
                    rec.key.remote_path,
                    rec.key.size as i64,
                    rec.key.remote_mtime,
                    rec.local_path,
                    rec.transferred_at
                ],
            )
            .map_err(sqerr)?;
        Ok(())
    }

    /// Local folder of an already imported file with the same stem in the
    /// same DCIM folder (the other half of a Live Photo).
    pub fn sibling_dir(&self, udid: &str, remote_path: &str) -> Result<Option<std::path::PathBuf>> {
        let Some((stem, _)) = remote_path.rsplit_once('.') else {
            return Ok(None);
        };
        let pattern = format!("{}.%", stem.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT local_path FROM transferred
                 WHERE udid = ?1 AND remote_path LIKE ?2 ESCAPE '\\' AND remote_path <> ?3
                 ORDER BY transferred_at DESC LIMIT 1",
            )
            .map_err(sqerr)?;
        let mut rows = stmt.query(params![udid, pattern, remote_path]).map_err(sqerr)?;
        let Some(row) = rows.next().map_err(sqerr)? else {
            return Ok(None);
        };
        let local: String = row.get(0).map_err(sqerr)?;
        Ok(std::path::Path::new(&local).parent().map(|p| p.to_path_buf()))
    }

    pub fn count(&self, udid: &str) -> Result<i64> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM transferred WHERE udid = ?1",
                params![udid],
                |r| r.get(0),
            )
            .map_err(sqerr)
    }
}

fn sqerr(e: rusqlite::Error) -> Error {
    Error::Other(format!("sqlite: {e}"))
}
