//! SQLite 持久化存储：剪切板历史记录。
//!
//! 数据库 schema：
//! ```sql
//! CREATE TABLE history (
//!   hash TEXT PRIMARY KEY,
//!   payload_type TEXT NOT NULL,
//!   payload_data BLOB NOT NULL,
//!   from_peer TEXT,
//!   timestamp_secs INTEGER NOT NULL
//! );
//! CREATE INDEX idx_timestamp ON history(timestamp_secs DESC);
//! ```

use anyhow::{Context, Result};
use bytes::Bytes;
use image::GenericImageView;
use lanclip_domain::{ClipboardPayload, ContentHash, DeviceId};
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Mutex;

pub struct HistoryStore {
    conn: Mutex<Connection>,
}

impl HistoryStore {
    /// 打开或创建数据库。
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).context("open sqlite db")?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "CREATE TABLE IF NOT EXISTS history (
                hash TEXT PRIMARY KEY,
                payload_type TEXT NOT NULL,
                payload_data BLOB NOT NULL,
                from_peer TEXT,
                timestamp_secs INTEGER NOT NULL
            )",
            [],
        )
        .context("create table")?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_timestamp ON history(timestamp_secs DESC)",
            [],
        )
        .context("create index")?;

        Ok(())
    }

    /// 插入或更新一条历史记录（按 hash 去重）。
    pub fn upsert(&self, entry: &StoredEntry) -> Result<()> {
        let (payload_type, payload_data) = serialize_payload(&entry.payload);
        let from_peer = entry.from_peer.as_ref().map(|d| d.0.as_str());

        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO history (hash, payload_type, payload_data, from_peer, timestamp_secs)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(hash) DO UPDATE SET
                payload_type=excluded.payload_type,
                payload_data=excluded.payload_data,
                from_peer=excluded.from_peer,
                timestamp_secs=excluded.timestamp_secs",
            params![
                entry.hash.0.as_str(),
                payload_type,
                payload_data.as_ref(),
                from_peer,
                entry.timestamp_secs as i64,
            ],
        )
        .context("upsert history entry")?;

        Ok(())
    }

    /// 删除指定 hash 的条目。
    pub fn delete(&self, hash: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM history WHERE hash = ?1", [hash])
            .context("delete history entry")?;
        Ok(())
    }

    /// 清空所有历史记录。
    pub fn clear(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM history", [])
            .context("clear history table")?;
        Ok(())
    }

    /// 加载最近 N 条记录（按时间倒序）。
    pub fn load_recent(&self, limit: usize) -> Result<Vec<StoredEntry>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT hash, payload_type, payload_data, from_peer, timestamp_secs
             FROM history
             ORDER BY timestamp_secs DESC
             LIMIT ?1",
        )?;

        let rows = stmt
            .query_map([limit as i64], |row| {
                let hash: String = row.get(0)?;
                let payload_type: String = row.get(1)?;
                let payload_data: Vec<u8> = row.get(2)?;
                let from_peer: Option<String> = row.get(3)?;
                let timestamp_secs: i64 = row.get(4)?;

                let payload = deserialize_payload(&payload_type, Bytes::from(payload_data))
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))?;
                let from_peer = from_peer.map(DeviceId);

                Ok(StoredEntry {
                    hash: ContentHash(hash),
                    payload,
                    from_peer,
                    timestamp_secs: timestamp_secs as u64,
                })
            })
            .context("query map")?;

        let mut entries = Vec::new();
        for row in rows {
            entries.push(row.context("row parse")?);
        }
        Ok(entries)
    }

    /// 按文本内容搜索。
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<StoredEntry>> {
        let pattern = format!("%{}%", query);
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT hash, payload_type, payload_data, from_peer, timestamp_secs
             FROM history
             WHERE (payload_type = 'text' AND payload_data LIKE ?1)
                OR (payload_type = 'image' AND ('image' LIKE ?1 OR 'png' LIKE ?1 OR hash LIKE ?1))
                OR (payload_type = 'files' AND (payload_data LIKE ?1 OR 'file' LIKE ?1 OR 'folder' LIKE ?1))
             ORDER BY timestamp_secs DESC
             LIMIT ?2",
        )?;

        let rows = stmt
            .query_map(params![&pattern, limit as i64], |row| {
                let hash: String = row.get(0)?;
                let payload_type: String = row.get(1)?;
                let payload_data: Vec<u8> = row.get(2)?;
                let from_peer: Option<String> = row.get(3)?;
                let timestamp_secs: i64 = row.get(4)?;

                let payload = deserialize_payload(&payload_type, Bytes::from(payload_data))
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))?;
                let from_peer = from_peer.map(DeviceId);

                Ok(StoredEntry {
                    hash: ContentHash(hash),
                    payload,
                    from_peer,
                    timestamp_secs: timestamp_secs as u64,
                })
            })
            .context("query map")?;

        let mut entries = Vec::new();
        for row in rows {
            entries.push(row.context("row parse")?);
        }
        Ok(entries)
    }

    /// 获取总条数。
    pub fn count(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM history", [], |row| row.get(0))?;
        Ok(count as usize)
    }
}

/// 存储层使用的条目结构（与 HistoryEntry 相同，但用于 SQLite）。
#[derive(Debug, Clone)]
pub struct StoredEntry {
    pub hash: ContentHash,
    pub payload: ClipboardPayload,
    pub from_peer: Option<DeviceId>,
    pub timestamp_secs: u64,
}

impl From<StoredEntry> for crate::clipboard_history::HistoryEntry {
    fn from(e: StoredEntry) -> Self {
        Self {
            hash: e.hash,
            payload: e.payload,
            from_peer: e.from_peer,
            timestamp_secs: e.timestamp_secs,
        }
    }
}

/// 序列化 ClipboardPayload 为 (type_str, bytes)。
fn serialize_payload(payload: &ClipboardPayload) -> (&'static str, Bytes) {
    match payload {
        ClipboardPayload::Text { plain, .. } => ("text", Bytes::copy_from_slice(plain.as_bytes())),
        ClipboardPayload::ImagePng { data, .. } => ("image", data.clone()),
        ClipboardPayload::FileRefs { entries } => {
            let data = serde_json::to_vec(entries).unwrap_or_default();
            ("files", Bytes::from(data))
        }
    }
}

/// 反序列化 ClipboardPayload。
fn deserialize_payload(typ: &str, data: Bytes) -> Result<ClipboardPayload> {
    match typ {
        "text" => {
            let s = String::from_utf8(data.to_vec()).context("invalid utf8 in text payload")?;
            Ok(ClipboardPayload::plain_text(s))
        }
        "image" => {
            // 图片数据是 PNG，需要解析出宽高
            let img = image::load_from_memory(&data).context("invalid png data")?;
            let (width, height) = img.dimensions();
            Ok(ClipboardPayload::ImagePng {
                width,
                height,
                data,
            })
        }
        "files" => {
            let entries = serde_json::from_slice(&data).context("invalid files payload")?;
            Ok(ClipboardPayload::FileRefs { entries })
        }
        _ => anyhow::bail!("unknown payload type: {typ}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn upsert_and_load() {
        let tmp = NamedTempFile::new().unwrap();
        let store = HistoryStore::open(tmp.path()).unwrap();

        let entry = StoredEntry {
            hash: ContentHash("abc123".into()),
            payload: ClipboardPayload::plain_text("hello"),
            from_peer: None,
            timestamp_secs: 1000,
        };
        store.upsert(&entry).unwrap();

        let loaded = store.load_recent(10).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].hash.0, "abc123");
    }

    #[test]
    fn search_text() {
        let tmp = NamedTempFile::new().unwrap();
        let store = HistoryStore::open(tmp.path()).unwrap();

        store
            .upsert(&StoredEntry {
                hash: ContentHash("1".into()),
                payload: ClipboardPayload::plain_text("hello world"),
                from_peer: None,
                timestamp_secs: 1000,
            })
            .unwrap();

        store
            .upsert(&StoredEntry {
                hash: ContentHash("2".into()),
                payload: ClipboardPayload::plain_text("foo bar"),
                from_peer: None,
                timestamp_secs: 2000,
            })
            .unwrap();

        let results = store.search("hello", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].hash.0, "1");
    }

    #[test]
    fn file_refs_roundtrip() {
        let tmp = NamedTempFile::new().unwrap();
        let store = HistoryStore::open(tmp.path()).unwrap();

        store
            .upsert(&StoredEntry {
                hash: ContentHash("files123".into()),
                payload: ClipboardPayload::FileRefs {
                    entries: vec![lanclip_domain::FileClipboardEntry {
                        path: std::path::PathBuf::from("/tmp/a.txt"),
                        name: "a.txt".into(),
                        is_dir: false,
                        size: Some(12),
                        child_count: None,
                    }],
                },
                from_peer: None,
                timestamp_secs: 3000,
            })
            .unwrap();

        let loaded = store.load_recent(10).unwrap();
        assert_eq!(loaded.len(), 1);
        match &loaded[0].payload {
            ClipboardPayload::FileRefs { entries } => assert_eq!(entries[0].name, "a.txt"),
            _ => panic!("expected files"),
        }
    }
}
