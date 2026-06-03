//! 剪切板历史：最近 N 条（去重 by hash），用于菜单栏/UI 展示。
//!
//! 设计：
//! - 内部用 `VecDeque<HistoryEntry>`，最新在前；
//! - 推入新条目时按 `ContentHash` 去重（已存在则移到队首）；
//! - 用 `tokio::sync::watch<u64>` 广播版本号；订阅者收到变化后调 `snapshot()` 重新拉。

use std::collections::VecDeque;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::history_store::HistoryStore;
use lanclip_domain::{ClipboardPayload, ContentHash, DeviceId};
use tokio::sync::watch;

/// 默认保留条数。
pub const DEFAULT_MAX_ENTRIES: usize = 50;

/// 单条历史记录。
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub hash: ContentHash,
    pub payload: ClipboardPayload,
    /// 来源：`None` 表示本机复制；`Some(peer_id)` 表示从某 peer 同步来。
    pub from_peer: Option<DeviceId>,
    /// Unix epoch 秒。
    pub timestamp_secs: u64,
}

impl HistoryEntry {
    pub fn new_local(payload: ClipboardPayload) -> Self {
        Self {
            hash: payload.hash(),
            payload,
            from_peer: None,
            timestamp_secs: now_secs(),
        }
    }

    pub fn new_remote(peer: DeviceId, payload: ClipboardPayload) -> Self {
        Self {
            hash: payload.hash(),
            payload,
            from_peer: Some(peer),
            timestamp_secs: now_secs(),
        }
    }

    /// 一行摘要（用于菜单 title），最长 `max_chars`。
    pub fn summary(&self, max_chars: usize) -> String {
        match &self.payload {
            ClipboardPayload::Text { plain, .. } => {
                let one_line: String = plain.replace(['\n', '\r', '\t'], " ");
                let len = plain.chars().count();
                format!("{} ({} chars)", truncate_chars(&one_line, max_chars), len)
            }
            ClipboardPayload::ImagePng {
                width,
                height,
                data,
            } => {
                let kb = data.len() as f64 / 1024.0;
                format!("[image {}x{} ({:.1} KB)]", width, height, kb)
            }
            ClipboardPayload::FileRefs { entries } => {
                let first = entries
                    .first()
                    .map(|entry| entry.name.as_str())
                    .unwrap_or("file");
                if entries.len() > 1 {
                    format!(
                        "[{} items: {}]",
                        entries.len(),
                        truncate_chars(first, max_chars)
                    )
                } else {
                    format!("[file: {}]", truncate_chars(first, max_chars))
                }
            }
        }
    }
}

// ============================================================================
// 服务
// ============================================================================

pub struct ClipboardHistory {
    max_entries: usize,
    state: RwLock<HistoryState>,
    watch_tx: watch::Sender<u64>,
    store: Option<HistoryStore>,
}

struct HistoryState {
    entries: VecDeque<HistoryEntry>,
    version: u64,
}

impl ClipboardHistory {
    pub fn new(max_entries: usize, store: Option<HistoryStore>) -> Arc<Self> {
        let (watch_tx, _) = watch::channel(0u64);
        let mut entries = VecDeque::with_capacity(max_entries);
        if let Some(store) = &store {
            if let Ok(loaded) = store.load_recent(max_entries) {
                for stored in loaded {
                    entries.push_back(stored.into());
                }
            }
        }
        Arc::new(Self {
            max_entries,
            state: RwLock::new(HistoryState {
                entries,
                version: 0,
            }),
            watch_tx,
            store,
        })
    }

    /// 推入一条新记录。已存在相同 hash 的旧条目会被移除（保留最新时间戳并置顶）。
    pub fn push(&self, entry: HistoryEntry) {
        if let Some(store) = &self.store {
            let stored = crate::history_store::StoredEntry {
                hash: entry.hash.clone(),
                payload: entry.payload.clone(),
                from_peer: entry.from_peer.clone(),
                timestamp_secs: entry.timestamp_secs,
            };
            if let Err(e) = store.upsert(&stored) {
                tracing::error!("failed to upsert history to sqlite: {e}");
            }
        }

        let v = {
            let mut st = self.state.write().expect("history rwlock poisoned");
            st.entries.retain(|e| e.hash != entry.hash);
            st.entries.push_front(entry);
            while st.entries.len() > self.max_entries {
                st.entries.pop_back();
            }
            st.version = st.version.wrapping_add(1);
            st.version
        };
        let _ = self.watch_tx.send(v);
    }

    /// 删除指定 hash 的条目。
    pub fn delete(&self, hash: &ContentHash) {
        if let Some(store) = &self.store {
            if let Err(e) = store.delete(&hash.0) {
                tracing::error!("failed to delete history from sqlite: {e}");
            }
        }

        let v = {
            let mut st = self.state.write().expect("history rwlock poisoned");
            st.entries.retain(|e| e.hash != *hash);
            st.version = st.version.wrapping_add(1);
            st.version
        };
        let _ = self.watch_tx.send(v);
    }

    /// 清空所有历史。
    pub fn clear(&self) {
        if let Some(store) = &self.store {
            if let Err(e) = store.clear() {
                tracing::error!("failed to clear history from sqlite: {e}");
            }
        }

        let v = {
            let mut st = self.state.write().expect("history rwlock poisoned");
            st.entries.clear();
            st.version = st.version.wrapping_add(1);
            st.version
        };
        let _ = self.watch_tx.send(v);
    }

    /// 搜索历史记录（优先从 SQLite 搜索，如果没有则在内存搜索）。
    pub fn search(&self, query: &str) -> Vec<HistoryEntry> {
        if let Some(store) = &self.store {
            match store.search(query, self.max_entries) {
                Ok(results) => results.into_iter().map(HistoryEntry::from).collect(),
                Err(e) => {
                    tracing::error!("failed to search sqlite history: {e}");
                    self.search_memory(query)
                }
            }
        } else {
            self.search_memory(query)
        }
    }

    fn search_memory(&self, query: &str) -> Vec<HistoryEntry> {
        let st = self.state.read().expect("history rwlock poisoned");
        let query_lower = query.to_lowercase();
        st.entries
            .iter()
            .filter(|e| match &e.payload {
                ClipboardPayload::Text { plain, .. } => plain.to_lowercase().contains(&query_lower),
                ClipboardPayload::ImagePng { .. } => {
                    "image".contains(&query_lower) || "png".contains(&query_lower)
                }
                ClipboardPayload::FileRefs { entries } => entries.iter().any(|entry| {
                    entry.name.to_lowercase().contains(&query_lower)
                        || entry
                            .path
                            .to_string_lossy()
                            .to_lowercase()
                            .contains(&query_lower)
                        || if entry.is_dir { "folder" } else { "file" }.contains(&query_lower)
                }),
            })
            .cloned()
            .collect()
    }

    /// 获取总数。
    pub fn total_count(&self) -> usize {
        if let Some(store) = &self.store {
            store.count().unwrap_or_else(|_| self.len())
        } else {
            self.len()
        }
    }

    pub fn snapshot(&self) -> Vec<HistoryEntry> {
        self.state
            .read()
            .expect("history rwlock poisoned")
            .entries
            .iter()
            .cloned()
            .collect()
    }

    pub fn len(&self) -> usize {
        self.state
            .read()
            .expect("history rwlock poisoned")
            .entries
            .len()
    }

    /// 订阅变化版本号；初始值 = 当前版本。
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.watch_tx.subscribe()
    }

    /// 按 hash 查找一条记录。
    pub fn find_by_hash(&self, hash: &ContentHash) -> Option<HistoryEntry> {
        self.state
            .read()
            .expect("history rwlock poisoned")
            .entries
            .iter()
            .find(|e| &e.hash == hash)
            .cloned()
    }
}

// ============================================================================
// helpers
// ============================================================================

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 按"字符"截断（而不是字节），保证 utf-8 安全。
fn truncate_chars(s: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut count = 0;
    for c in s.chars() {
        if count >= max_chars {
            out.push('…');
            break;
        }
        out.push(c);
        count += 1;
    }
    out
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn payload_text(s: &str) -> ClipboardPayload {
        ClipboardPayload::plain_text(s)
    }

    #[test]
    fn push_and_snapshot_basic() {
        let h = ClipboardHistory::new(3, None);
        h.push(HistoryEntry::new_local(payload_text("a")));
        h.push(HistoryEntry::new_local(payload_text("b")));
        let snap = h.snapshot();
        assert_eq!(snap.len(), 2);
        match &snap[0].payload {
            ClipboardPayload::Text { plain, .. } => assert_eq!(plain, "b"),
            _ => panic!(),
        }
    }

    #[test]
    fn dedup_by_hash_promotes() {
        let h = ClipboardHistory::new(5, None);
        h.push(HistoryEntry::new_local(payload_text("a")));
        h.push(HistoryEntry::new_local(payload_text("b")));
        h.push(HistoryEntry::new_local(payload_text("a")));
        let snap = h.snapshot();
        assert_eq!(snap.len(), 2, "dedup should keep len = 2");
        match &snap[0].payload {
            ClipboardPayload::Text { plain, .. } => assert_eq!(plain, "a"),
            _ => panic!(),
        }
    }

    #[test]
    fn cap_at_max_entries() {
        let h = ClipboardHistory::new(2, None);
        h.push(HistoryEntry::new_local(payload_text("a")));
        h.push(HistoryEntry::new_local(payload_text("b")));
        h.push(HistoryEntry::new_local(payload_text("c")));
        let snap = h.snapshot();
        assert_eq!(snap.len(), 2);
        let texts: Vec<&str> = snap
            .iter()
            .filter_map(|e| match &e.payload {
                ClipboardPayload::Text { plain, .. } => Some(plain.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["c", "b"]);
    }

    #[tokio::test]
    async fn watch_notifies_on_push() {
        let h = ClipboardHistory::new(5, None);
        let mut rx = h.subscribe();
        let initial = *rx.borrow();
        h.push(HistoryEntry::new_local(payload_text("x")));
        rx.changed().await.expect("watch should fire");
        assert_ne!(*rx.borrow(), initial);
    }

    #[test]
    fn summary_truncates() {
        let entry = HistoryEntry::new_local(payload_text("hello world this is long"));
        let s = entry.summary(10);
        assert!(s.starts_with("hello worl…"));
        assert!(s.contains("24 chars"));
    }

    #[test]
    fn summary_image_shows_dimensions() {
        let payload = ClipboardPayload::ImagePng {
            width: 100,
            height: 50,
            data: bytes::Bytes::from(vec![0u8; 1024]),
        };
        let entry = HistoryEntry::new_local(payload);
        assert_eq!(entry.summary(80), "[image 100x50 (1.0 KB)]");
    }
}
