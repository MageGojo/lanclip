//! 文件传输 —— 多连接并发（1 控制 + N 数据，详见设计文档 5.5）。
//!
//! 这一层做的事：
//! - **发送**：把 `Vec<FileEntry>` 分发给 N 条已握手的 data 连接，并发流式推送。
//! - **接收**：每条 data 连接独立消费 `FileBegin → Binary… → FileEnd`，按 `entry_id` 落盘。
//!
//! 控制连接上的 `TransferOffer / Accept / Done / Cancel / Progress` 由 app 层处理；
//! 本 crate 只负责数据通道的并发执行。

// TransferError 透传了较大的 `NetError`（内含 tungstenite::Error），large-err 提示可忽略。
#![allow(clippy::result_large_err)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use lanclip_domain::{EntryId, FileEntry, TaskId};
use lanclip_network::{ConnHandle, InFrame, NetError};
use lanclip_proto::{Msg, MAX_BINARY_CHUNK};
use thiserror::Error;
use tokio::fs::{self, File};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// 默认并发度。
pub const DEFAULT_PARALLELISM: usize = 4;

// ============================================================================
// 错误
// ============================================================================

#[derive(Debug, Error)]
pub enum TransferError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("network: {0}")]
    Net(#[from] NetError),

    #[error("entry missing source_path (this is a send-side bug)")]
    MissingSource,

    #[error("path escapes download dir: {0:?}")]
    PathEscape(PathBuf),

    #[error("unexpected frame on data connection: {0}")]
    UnexpectedFrame(String),

    #[error("entry {0} not declared in current task")]
    UnknownEntry(EntryId),

    #[error("task cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, TransferError>;

// ============================================================================
// 进度
// ============================================================================

/// 跨 task / 跨 worker 共享的进度计数器。
#[derive(Debug, Clone, Default)]
pub struct ProgressMeter {
    bytes_done: Arc<AtomicU64>,
    total_bytes: u64,
}

impl ProgressMeter {
    pub fn new(total_bytes: u64) -> Self {
        Self {
            bytes_done: Arc::new(AtomicU64::new(0)),
            total_bytes,
        }
    }

    pub fn add(&self, n: u64) {
        self.bytes_done.fetch_add(n, Ordering::Relaxed);
    }

    pub fn bytes_done(&self) -> u64 {
        self.bytes_done.load(Ordering::Relaxed)
    }

    pub fn total(&self) -> u64 {
        self.total_bytes
    }
}

// ============================================================================
// 发送
// ============================================================================

/// 发送一个任务的句柄。所有 worker join 后通过 `wait` 拿最终结果。
pub struct SendTaskHandle {
    pub task_id: TaskId,
    pub progress: ProgressMeter,
    workers: Vec<tokio::task::JoinHandle<Result<()>>>,
}

impl SendTaskHandle {
    /// 等待所有 worker 结束；任一 worker 失败则整体返回该错误。
    pub async fn wait(self) -> Result<()> {
        let mut first_err: Option<TransferError> = None;
        for w in self.workers {
            match w.await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    warn!("send worker failed: {e}");
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                }
                Err(join_err) => {
                    warn!("send worker panicked: {join_err}");
                    if first_err.is_none() {
                        first_err = Some(TransferError::Io(std::io::Error::other(format!(
                            "join: {join_err}"
                        ))));
                    }
                }
            }
        }
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

/// 启动 **多连接并发发送**：把 `entries` 分发给 `data_conns` 上的 N 个 worker。
///
/// 调用方负责：
/// - 在控制连接上完成 `TransferOffer / Accept` 协商；
/// - 把 N 条已握手的 data `ConnHandle` 传进来。
pub fn spawn_send(
    task_id: TaskId,
    entries: Vec<FileEntry>,
    data_conns: Vec<ConnHandle>,
) -> SendTaskHandle {
    let total_bytes: u64 = entries.iter().map(|e| e.size).sum();
    let progress = ProgressMeter::new(total_bytes);

    // 多消费者队列：N 个 worker 抢任务
    let (job_tx, job_rx) = async_channel::unbounded::<FileEntry>();
    for e in entries {
        // unbounded 不会阻塞；启动前一次性入队
        let _ = job_tx.send_blocking(e);
    }
    job_tx.close();

    let worker_count = data_conns.len().max(1);
    info!(
        "send task {task_id} starting: total={} bytes, workers={}",
        total_bytes, worker_count
    );

    let mut workers = Vec::with_capacity(worker_count);
    for (idx, conn) in data_conns.into_iter().enumerate() {
        let job_rx = job_rx.clone();
        let progress = progress.clone();
        let handle = tokio::spawn(async move {
            debug!("send worker {idx} started");
            while let Ok(entry) = job_rx.recv().await {
                send_one_entry(task_id, &entry, &conn, &progress).await?;
            }
            debug!("send worker {idx} finished");
            Ok(())
        });
        workers.push(handle);
    }

    SendTaskHandle {
        task_id,
        progress,
        workers,
    }
}

async fn send_one_entry(
    task_id: TaskId,
    entry: &FileEntry,
    conn: &ConnHandle,
    progress: &ProgressMeter,
) -> Result<()> {
    let source = entry
        .source_path
        .as_ref()
        .ok_or(TransferError::MissingSource)?;

    let mut file = File::open(source).await?;

    conn.send_msg(&Msg::FileBegin {
        task_id,
        entry_id: entry.entry_id,
        rel_path: entry.rel_path.clone(),
        size: entry.size,
    })
    .await?;

    let mut buf = vec![0u8; MAX_BINARY_CHUNK];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        let chunk = Bytes::copy_from_slice(&buf[..n]);
        conn.send_binary(chunk).await?;
        progress.add(n as u64);
    }

    conn.send_msg(&Msg::FileEnd {
        task_id,
        entry_id: entry.entry_id,
    })
    .await?;

    debug!(
        "sent entry {} ({} bytes) on conn for task {task_id}",
        entry.rel_path, entry.size
    );
    Ok(())
}

// ============================================================================
// 接收
// ============================================================================

/// 接收端单个 entry 的元信息（控制连接收到 `TransferOffer` 后由 app 层登记）。
#[derive(Debug, Clone)]
pub struct ExpectedEntry {
    pub entry_id: EntryId,
    /// 已规范化的相对路径（剔除 `..`、绝对前缀）。
    pub rel_path: String,
    pub size: u64,
}

/// 启动 **一条 data 连接上的接收 worker**。N 条连接 = N 个 worker，并发。
///
/// - `download_root`：根目录（已含 sender 子目录），所有 entry 都落在它下面。
/// - `inbound`：这条 data 连接的 inbound 帧流。
/// - `expected`：本任务期望的 entry 集合（用 `entry_id` 索引；只读，多个 worker 共享）。
pub fn spawn_recv_worker(
    task_id: TaskId,
    download_root: PathBuf,
    expected: Arc<RecvTaskExpect>,
    mut inbound: mpsc::Receiver<InFrame>,
    progress: ProgressMeter,
) -> tokio::task::JoinHandle<Result<()>> {
    tokio::spawn(async move {
        let mut state = WorkerState::Idle;

        while let Some(frame) = inbound.recv().await {
            match frame {
                InFrame::Msg(Msg::FileBegin {
                    task_id: t,
                    entry_id,
                    rel_path,
                    size,
                }) => {
                    if t != task_id {
                        return Err(TransferError::UnexpectedFrame(format!(
                            "FileBegin task mismatch: got {t}, expected {task_id}"
                        )));
                    }
                    let meta = expected
                        .get(entry_id)
                        .ok_or(TransferError::UnknownEntry(entry_id))?;
                    if meta.rel_path != rel_path || meta.size != size {
                        return Err(TransferError::UnexpectedFrame(format!(
                            "FileBegin meta mismatch for entry {entry_id}"
                        )));
                    }
                    let dest = safe_join(&download_root, &rel_path)?;
                    let part = with_part_suffix(&dest);
                    if let Some(parent) = part.parent() {
                        fs::create_dir_all(parent).await?;
                    }
                    let file = File::create(&part).await?;
                    state = WorkerState::Receiving {
                        entry_id,
                        dest,
                        part,
                        file,
                        bytes_left: size,
                    };
                }

                InFrame::Binary(bytes) => {
                    let WorkerState::Receiving {
                        file,
                        bytes_left,
                        entry_id,
                        ..
                    } = &mut state
                    else {
                        return Err(TransferError::UnexpectedFrame(
                            "Binary without active FileBegin".into(),
                        ));
                    };
                    if bytes.len() as u64 > *bytes_left {
                        return Err(TransferError::UnexpectedFrame(format!(
                            "entry {entry_id}: binary chunk exceeds remaining size",
                        )));
                    }
                    file.write_all(&bytes).await?;
                    *bytes_left -= bytes.len() as u64;
                    progress.add(bytes.len() as u64);
                }

                InFrame::Msg(Msg::FileEnd {
                    task_id: t,
                    entry_id,
                }) => {
                    if t != task_id {
                        return Err(TransferError::UnexpectedFrame(format!(
                            "FileEnd task mismatch: got {t}, expected {task_id}"
                        )));
                    }
                    let prev = std::mem::replace(&mut state, WorkerState::Idle);
                    let WorkerState::Receiving {
                        entry_id: cur_id,
                        dest,
                        part,
                        mut file,
                        bytes_left,
                    } = prev
                    else {
                        return Err(TransferError::UnexpectedFrame(
                            "FileEnd without active FileBegin".into(),
                        ));
                    };
                    if cur_id != entry_id {
                        return Err(TransferError::UnexpectedFrame(format!(
                            "FileEnd entry mismatch: got {entry_id}, current {cur_id}",
                        )));
                    }
                    if bytes_left != 0 {
                        return Err(TransferError::UnexpectedFrame(format!(
                            "FileEnd for entry {entry_id} but {bytes_left} bytes missing",
                        )));
                    }
                    file.flush().await?;
                    drop(file);
                    fs::rename(&part, &dest).await?;
                    expected.mark_done(entry_id);
                    debug!("recv worker: entry {entry_id} done -> {:?}", dest);
                }

                InFrame::Msg(other) => {
                    return Err(TransferError::UnexpectedFrame(format!(
                        "non-data frame on data conn: {other:?}",
                    )));
                }
            }
        }

        // inbound 关闭：如果还有未完成的 entry，认定失败
        if let WorkerState::Receiving { entry_id, part, .. } = state {
            let _ = fs::remove_file(&part).await;
            return Err(TransferError::UnexpectedFrame(format!(
                "data conn closed mid-entry {entry_id}",
            )));
        }
        Ok(())
    })
}

enum WorkerState {
    Idle,
    Receiving {
        entry_id: EntryId,
        dest: PathBuf,
        part: PathBuf,
        file: File,
        bytes_left: u64,
    },
}

/// 接收任务的"期望集合"：所有 entry 的元信息 + 完成状态。
/// N 个 worker 共享同一份；每个 worker 拿到 entry 后调 `mark_done`。
pub struct RecvTaskExpect {
    entries: std::collections::HashMap<EntryId, ExpectedEntry>,
    done: std::sync::Mutex<std::collections::HashSet<EntryId>>,
}

impl RecvTaskExpect {
    pub fn new(entries: Vec<ExpectedEntry>) -> Arc<Self> {
        let map = entries.into_iter().map(|e| (e.entry_id, e)).collect();
        Arc::new(Self {
            entries: map,
            done: std::sync::Mutex::new(std::collections::HashSet::new()),
        })
    }

    pub fn get(&self, id: EntryId) -> Option<&ExpectedEntry> {
        self.entries.get(&id)
    }

    pub fn mark_done(&self, id: EntryId) {
        self.done.lock().expect("done mutex poisoned").insert(id);
    }

    /// 是否所有 entry 都已完成。
    pub fn is_complete(&self) -> bool {
        let done = self.done.lock().expect("done mutex poisoned");
        done.len() == self.entries.len()
    }

    pub fn total_bytes(&self) -> u64 {
        self.entries.values().map(|e| e.size).sum()
    }
}

// ============================================================================
// 路径安全
// ============================================================================

/// 把 `rel_path` 安全地拼到 `root` 下，禁止 `..` 或绝对路径前缀逃逸。
pub fn safe_join(root: &Path, rel_path: &str) -> Result<PathBuf> {
    let rel = Path::new(rel_path);
    if rel.is_absolute() {
        return Err(TransferError::PathEscape(rel.to_path_buf()));
    }
    let mut out = root.to_path_buf();
    for comp in rel.components() {
        use std::path::Component;
        match comp {
            Component::Normal(c) => out.push(c),
            Component::CurDir => {}
            // 任何 ParentDir / RootDir / Prefix 一律拒绝
            _ => return Err(TransferError::PathEscape(rel.to_path_buf())),
        }
    }
    Ok(out)
}

fn with_part_suffix(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(".lanclip.part");
    PathBuf::from(s)
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn safe_join_normal() {
        let root = PathBuf::from("/tmp/dl");
        let p = safe_join(&root, "a/b.txt").unwrap();
        assert_eq!(p, PathBuf::from("/tmp/dl/a/b.txt"));
    }

    #[test]
    fn safe_join_rejects_parent() {
        let root = PathBuf::from("/tmp/dl");
        assert!(safe_join(&root, "../etc/passwd").is_err());
        assert!(safe_join(&root, "a/../../etc").is_err());
    }

    #[test]
    fn safe_join_rejects_absolute() {
        let root = PathBuf::from("/tmp/dl");
        assert!(safe_join(&root, "/etc/passwd").is_err());
    }

    #[test]
    fn part_suffix() {
        let p = PathBuf::from("/tmp/dl/a.txt");
        assert_eq!(
            with_part_suffix(&p),
            PathBuf::from("/tmp/dl/a.txt.lanclip.part")
        );
    }

    #[test]
    fn progress_meter_basic() {
        let m = ProgressMeter::new(100);
        assert_eq!(m.bytes_done(), 0);
        m.add(30);
        m.add(20);
        assert_eq!(m.bytes_done(), 50);
        assert_eq!(m.total(), 100);
    }
}
