//! 文件传输服务：协调控制连接的协商 + N 数据连接的并发传输。
//!
//! 约束（M5）：
//! - 每对 peer 同一时刻 **最多一个** 进行中的接收任务（防止数据连接路由歧义）；
//! - 不支持文件夹递归（仅平铺文件列表），M5+ 再加；
//! - 不支持取消（留 TODO）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use lanclip_domain::{DeviceId, FileEntry, TaskId};
use lanclip_proto::{FileEntryMeta, Msg};
use lanclip_transfer::{
    spawn_recv_worker, spawn_send, ExpectedEntry, ProgressMeter, RecvTaskExpect,
    TransferError as TxError,
};
use thiserror::Error;
use tokio::sync::{broadcast, mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::connections::{ConnEvent, ConnectionManager, NewDataConn};

// ============================================================================
// 错误
// ============================================================================

#[derive(Debug, Error)]
pub enum TransferSvcError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("network/conn-mgr: {0}")]
    Conn(#[from] crate::connections::ConnMgrError),

    #[error("transfer: {0}")]
    Tx(#[from] TxError),

    #[error("peer rejected transfer: {0}")]
    Rejected(String),

    #[error("peer disconnected during transfer")]
    PeerDisconnected,

    #[error("no files to send")]
    Empty,

    #[error("file not found: {0:?}")]
    FileNotFound(PathBuf),

    #[error("directories not supported yet (M5+): {0:?}")]
    DirectoryUnsupported(PathBuf),

    #[error("peer {0} already has an active receive task")]
    RecvBusy(DeviceId),
}

pub type Result<T> = std::result::Result<T, TransferSvcError>;

// ============================================================================
// 配置
// ============================================================================

#[derive(Debug, Clone)]
pub struct TransferSvcConfig {
    pub download_root: PathBuf,
    pub parallelism: usize,
    pub auto_accept: bool,
}

// ============================================================================
// 状态
// ============================================================================

struct TransferState {
    /// 接收端：peer → 当前进行中的接收任务（最多 1 个）。
    incoming: HashMap<DeviceId, IncomingTask>,
    /// 发送端：task_id → 进行中的发送任务（等待 accept 时持有 signal）。
    outgoing: HashMap<TaskId, OutgoingTask>,
}

impl TransferState {
    fn new() -> Self {
        Self {
            incoming: HashMap::new(),
            outgoing: HashMap::new(),
        }
    }
}

#[allow(dead_code)] // sender/sender_name 供未来接入 UI 时使用
struct IncomingTask {
    task_id: TaskId,
    sender: DeviceId,
    sender_name: String,
    expected: Arc<RecvTaskExpect>,
    download_root: PathBuf,
    progress: ProgressMeter,
    workers: Vec<JoinHandle<std::result::Result<(), TxError>>>,
    /// 保活：每条数据连接的 ConnHandle 必须存活到任务结束，
    /// 否则 outbound mpsc 关闭 → writer task close WS → 对端 reader 关闭 → inbound 中断。
    data_handles: Vec<lanclip_network::ConnHandle>,
    started_at: Instant,
}

#[allow(dead_code)] // task_id/peer_id 供调试/未来查询使用
struct OutgoingTask {
    task_id: TaskId,
    peer_id: DeviceId,
    accept_signal: Option<oneshot::Sender<std::result::Result<(), String>>>,
}

// ============================================================================
// 服务
// ============================================================================

pub struct TransferService {
    #[allow(dead_code)] // 预留给日后冲突检查/源头样查询
    self_id: DeviceId,
    conn_mgr: Arc<ConnectionManager>,
    config: TransferSvcConfig,
    state: Mutex<TransferState>,
}

impl TransferService {
    /// 启动服务：spawn 两个常驻 task（控制事件路由 + 数据连接路由）。
    pub fn spawn(
        self_id: DeviceId,
        conn_mgr: Arc<ConnectionManager>,
        new_data_rx: mpsc::Receiver<NewDataConn>,
        config: TransferSvcConfig,
    ) -> Arc<Self> {
        let svc = Arc::new(Self {
            self_id,
            conn_mgr: conn_mgr.clone(),
            config,
            state: Mutex::new(TransferState::new()),
        });

        // 控制事件路由
        let svc_for_ctrl = svc.clone();
        let ctrl_events = conn_mgr.subscribe();
        tokio::spawn(run_control_event_loop(svc_for_ctrl, ctrl_events));

        // 数据连接路由
        let svc_for_data = svc.clone();
        tokio::spawn(run_data_conn_loop(svc_for_data, new_data_rx));

        svc
    }

    /// 发送文件（UI 调用）。**当前不支持文件夹**。
    /// 阻塞到所有 worker 完成、对端确认完成。
    pub async fn send_files(
        self: &Arc<Self>,
        peer_id: DeviceId,
        files: Vec<PathBuf>,
    ) -> Result<TaskId> {
        if files.is_empty() {
            return Err(TransferSvcError::Empty);
        }

        // 1. 扫描文件
        let entries = scan_files(&files).await?;
        let total_bytes: u64 = entries.iter().map(|e| e.size).sum();
        let task_id = Uuid::new_v4();

        info!(
            "send_files: task={task_id} peer={peer_id} files={} total={} bytes",
            entries.len(),
            total_bytes
        );

        // 2. 注册 accept 等待 channel
        let (accept_tx, accept_rx) = oneshot::channel();
        {
            let mut st = self.state.lock().await;
            st.outgoing.insert(
                task_id,
                OutgoingTask {
                    task_id,
                    peer_id: peer_id.clone(),
                    accept_signal: Some(accept_tx),
                },
            );
        }

        // 用 defer-like cleanup
        let result = self
            .send_files_inner(task_id, &peer_id, entries, total_bytes, accept_rx)
            .await;

        // 3. 清理 registry
        {
            let mut st = self.state.lock().await;
            st.outgoing.remove(&task_id);
        }

        result.map(|_| task_id)
    }

    async fn send_files_inner(
        self: &Arc<Self>,
        task_id: TaskId,
        peer_id: &DeviceId,
        entries: Vec<FileEntry>,
        total_bytes: u64,
        accept_rx: oneshot::Receiver<std::result::Result<(), String>>,
    ) -> Result<()> {
        // 1. 发 TransferOffer
        let entries_meta: Vec<FileEntryMeta> = entries
            .iter()
            .map(|e| FileEntryMeta {
                entry_id: e.entry_id,
                rel_path: e.rel_path.clone(),
                size: e.size,
            })
            .collect();

        self.conn_mgr
            .send_control(
                peer_id,
                &Msg::TransferOffer {
                    task_id,
                    entries: entries_meta,
                    total: total_bytes,
                },
            )
            .await?;

        // 2. 等 accept
        let accept_result = accept_rx
            .await
            .map_err(|_| TransferSvcError::PeerDisconnected)?;
        match accept_result {
            Ok(()) => debug!("send {task_id}: peer accepted"),
            Err(reason) => return Err(TransferSvcError::Rejected(reason)),
        }

        // 3. 建立 N 条数据连接
        let n = self.config.parallelism.min(entries.len()).max(1);
        let mut data_conns = Vec::with_capacity(n);
        for i in 0..n {
            let (handle, inbound) = self.conn_mgr.dial_data(peer_id).await?;
            // 发送端不期望从 data 连接收数据；drain 防止 reader 堆积
            tokio::spawn(drain_inbound(inbound));
            debug!("send {task_id}: data conn #{i} established");
            data_conns.push(handle);
        }

        // 4. spawn_send + wait
        let send_handle = spawn_send(task_id, entries, data_conns);
        let send_result = send_handle.wait().await;

        // 5. 无论成功失败，通知对端
        match send_result {
            Ok(()) => {
                self.conn_mgr
                    .send_control(peer_id, &Msg::TransferDone { task_id })
                    .await?;
                info!("send {task_id}: completed, {total_bytes} bytes");
                Ok(())
            }
            Err(e) => {
                let _ = self
                    .conn_mgr
                    .send_control(peer_id, &Msg::TransferCancel { task_id })
                    .await;
                Err(TransferSvcError::Tx(e))
            }
        }
    }

    // ------------------------------------------------------------------------
    // 控制事件处理
    // ------------------------------------------------------------------------

    async fn on_control_msg(self: &Arc<Self>, peer_id: DeviceId, msg: Msg) {
        match msg {
            Msg::TransferOffer {
                task_id,
                entries,
                total,
            } => {
                self.handle_offer(peer_id, task_id, entries, total).await;
            }
            Msg::TransferAccept { task_id } => self.handle_accept(task_id, Ok(())).await,
            Msg::TransferReject { task_id, reason } => {
                self.handle_accept(task_id, Err(reason)).await
            }
            Msg::TransferDone { task_id } => self.handle_done(peer_id, task_id).await,
            Msg::TransferCancel { task_id } => self.handle_cancel(peer_id, task_id).await,
            _ => {} // 其它消息（剪切板等）不在本服务范围
        }
    }

    async fn handle_offer(
        self: &Arc<Self>,
        peer_id: DeviceId,
        task_id: TaskId,
        entries: Vec<FileEntryMeta>,
        total: u64,
    ) {
        info!(
            "recv: offer from {peer_id} task={task_id} files={} total={total} bytes",
            entries.len()
        );

        if !self.config.auto_accept {
            // M5 阶段：未实现弹窗，等价于自动拒绝
            let _ = self
                .conn_mgr
                .send_control(
                    &peer_id,
                    &Msg::TransferReject {
                        task_id,
                        reason: "manual accept not implemented".into(),
                    },
                )
                .await;
            return;
        }

        // 一个 peer 同时只允许一个接收任务
        {
            let st = self.state.lock().await;
            if st.incoming.contains_key(&peer_id) {
                drop(st);
                warn!("recv: rejecting {task_id} from {peer_id}: another task in progress");
                let _ = self
                    .conn_mgr
                    .send_control(
                        &peer_id,
                        &Msg::TransferReject {
                            task_id,
                            reason: "peer busy".into(),
                        },
                    )
                    .await;
                return;
            }
        }

        // 注册
        let expected_entries: Vec<ExpectedEntry> = entries
            .into_iter()
            .map(|e| ExpectedEntry {
                entry_id: e.entry_id,
                rel_path: e.rel_path,
                size: e.size,
            })
            .collect();
        let expected = RecvTaskExpect::new(expected_entries);
        let sender_name = peer_id.0.clone(); // 简化：用 id 当目录名（精确名字未来从 PeerRegistry 拿）
        let download_root = self.config.download_root.join(&sender_name);

        {
            let mut st = self.state.lock().await;
            st.incoming.insert(
                peer_id.clone(),
                IncomingTask {
                    task_id,
                    sender: peer_id.clone(),
                    sender_name,
                    expected,
                    download_root,
                    progress: ProgressMeter::new(total),
                    workers: Vec::new(),
                    data_handles: Vec::new(),
                    started_at: Instant::now(),
                },
            );
        }

        // 回 Accept
        if let Err(e) = self
            .conn_mgr
            .send_control(&peer_id, &Msg::TransferAccept { task_id })
            .await
        {
            warn!("recv: send Accept failed: {e}");
        }
    }

    async fn handle_accept(
        self: &Arc<Self>,
        task_id: TaskId,
        result: std::result::Result<(), String>,
    ) {
        let signal = {
            let mut st = self.state.lock().await;
            st.outgoing
                .get_mut(&task_id)
                .and_then(|t| t.accept_signal.take())
        };
        if let Some(s) = signal {
            let _ = s.send(result);
        } else {
            debug!("ignored accept/reject for unknown task {task_id}");
        }
    }

    async fn handle_done(self: &Arc<Self>, peer_id: DeviceId, task_id: TaskId) {
        info!("recv: TransferDone from {peer_id} task={task_id}");
        let task = {
            let mut st = self.state.lock().await;
            st.incoming.remove(&peer_id)
        };
        let Some(task) = task else {
            warn!("recv: TransferDone for unknown peer {peer_id}");
            return;
        };
        if task.task_id != task_id {
            warn!(
                "recv: TransferDone task mismatch (peer {peer_id}: expected {} got {})",
                task.task_id, task_id
            );
            return;
        }

        // 等所有 worker join
        let n = task.workers.len();
        let mut errs = 0usize;
        for w in task.workers {
            match w.await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    warn!("recv: worker err: {e}");
                    errs += 1;
                }
                Err(e) => {
                    warn!("recv: worker panicked: {e}");
                    errs += 1;
                }
            }
        }
        let complete = task.expected.is_complete();
        let elapsed = task.started_at.elapsed();
        info!(
            "recv: task {task_id} from {peer_id} finished: workers={n} errs={errs} complete={complete} elapsed={:?}",
            elapsed
        );
    }

    async fn handle_cancel(self: &Arc<Self>, peer_id: DeviceId, task_id: TaskId) {
        info!("recv: TransferCancel from {peer_id} task={task_id}");
        let task = {
            let mut st = self.state.lock().await;
            st.incoming.remove(&peer_id)
        };
        if let Some(t) = task {
            for w in t.workers {
                w.abort();
            }
        }
    }

    // ------------------------------------------------------------------------
    // 数据连接路由
    // ------------------------------------------------------------------------

    async fn on_new_data_conn(self: &Arc<Self>, conn: NewDataConn) {
        let NewDataConn {
            peer_id,
            handle,
            inbound,
        } = conn;

        // 查找该 peer 当前的接收任务
        let (task_id, expected, download_root, progress) = {
            let st = self.state.lock().await;
            let Some(task) = st.incoming.get(&peer_id) else {
                warn!("data conn from {peer_id} but no active recv task; closing");
                handle.close().await;
                return;
            };
            (
                task.task_id,
                task.expected.clone(),
                task.download_root.clone(),
                task.progress.clone(),
            )
        };

        let worker = spawn_recv_worker(task_id, download_root, expected, inbound, progress);

        // 挂回 task：handle 保活，worker 待 join
        let mut st = self.state.lock().await;
        if let Some(task) = st.incoming.get_mut(&peer_id) {
            task.data_handles.push(handle);
            task.workers.push(worker);
        } else {
            // 任务在我们 spawn worker 期间被移除 → 取消 worker
            worker.abort();
        }
    }
}

// ============================================================================
// 事件循环
// ============================================================================

async fn run_control_event_loop(
    svc: Arc<TransferService>,
    mut events: broadcast::Receiver<ConnEvent>,
) {
    loop {
        match events.recv().await {
            Ok(ConnEvent::ControlMessage { peer_id, msg }) => {
                svc.on_control_msg(peer_id, msg).await;
            }
            Ok(_) => {}
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!("transfer-svc: control events lagged: {n}");
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
    debug!("transfer-svc: control event loop exit");
}

async fn run_data_conn_loop(svc: Arc<TransferService>, mut rx: mpsc::Receiver<NewDataConn>) {
    while let Some(conn) = rx.recv().await {
        svc.on_new_data_conn(conn).await;
    }
    debug!("transfer-svc: data conn loop exit");
}

async fn drain_inbound(mut rx: mpsc::Receiver<lanclip_network::InFrame>) {
    while rx.recv().await.is_some() {}
}

// ============================================================================
// 文件扫描
// ============================================================================

/// 扫描用户选择的文件列表（**当前不递归文件夹**）。
async fn scan_files(files: &[PathBuf]) -> Result<Vec<FileEntry>> {
    let mut entries = Vec::with_capacity(files.len());
    let mut next_id: u32 = 0;
    for path in files {
        let meta = tokio::fs::metadata(path)
            .await
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => TransferSvcError::FileNotFound(path.clone()),
                _ => TransferSvcError::Io(e),
            })?;
        if meta.is_dir() {
            return Err(TransferSvcError::DirectoryUnsupported(path.clone()));
        }
        if !meta.is_file() {
            continue;
        }
        let rel_path = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("file-{next_id}"));
        entries.push(FileEntry {
            entry_id: next_id,
            rel_path,
            source_path: Some(path.clone()),
            size: meta.len(),
        });
        next_id += 1;
    }
    if entries.is_empty() {
        return Err(TransferSvcError::Empty);
    }
    Ok(entries)
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::time::Duration;
    use tempfile::tempdir;
    use tokio::io::AsyncWriteExt;

    fn dev(s: &str) -> DeviceId {
        DeviceId(s.into())
    }

    fn device_public(id: &str) -> lanclip_proto::DevicePublic {
        lanclip_proto::DevicePublic {
            id: id.into(),
            name: format!("dev-{id}"),
            os: "mac".into(),
        }
    }

    async fn write_file(path: &Path, content: &[u8]) {
        let mut f = tokio::fs::File::create(path).await.unwrap();
        f.write_all(content).await.unwrap();
        f.flush().await.unwrap();
    }

    /// 端到端：A 起两个 service（A/B），A 发 3 个文件 → B 落盘并校验内容。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn end_to_end_multi_file_transfer() {
        // ---- 起两个 ConnectionManager + listener ----
        let a_id = dev("bbb"); // > b_id, A 主动 dial B
        let b_id = dev("aaa");

        use lanclip_network::WsListener;

        let (a_cm, _a_events, a_data_rx) =
            ConnectionManager::new(a_id.clone(), device_public("bbb"));
        let (b_cm, _b_events, b_data_rx) =
            ConnectionManager::new(b_id.clone(), device_public("aaa"));

        let a_listener = WsListener::bind(0).await.unwrap();
        let b_listener = WsListener::bind(0).await.unwrap();
        let b_port = b_listener.local_port();

        a_cm.start_listener(a_listener);
        b_cm.start_listener(b_listener);

        let b_addr: std::net::SocketAddr = ([127, 0, 0, 1], b_port).into();
        a_cm.try_dial_control(b_id.clone(), vec![b_addr]).await;

        // 等控制连接建好
        wait_until(Duration::from_secs(2), || async {
            !a_cm.connected_peers().await.is_empty() && !b_cm.connected_peers().await.is_empty()
        })
        .await
        .expect("control connections");

        // ---- 起两个 TransferService ----
        let tmpdir = tempdir().unwrap();
        let a_root = tmpdir.path().join("a-recv");
        let b_root = tmpdir.path().join("b-recv");

        let a_svc = TransferService::spawn(
            a_id.clone(),
            a_cm.clone(),
            a_data_rx,
            TransferSvcConfig {
                download_root: a_root,
                parallelism: 3,
                auto_accept: true,
            },
        );
        let b_svc = TransferService::spawn(
            b_id.clone(),
            b_cm.clone(),
            b_data_rx,
            TransferSvcConfig {
                download_root: b_root.clone(),
                parallelism: 3,
                auto_accept: true,
            },
        );
        let _ = a_svc; // 仅 b 接收

        // ---- 准备 3 个源文件 ----
        let src_dir = tmpdir.path().join("src");
        tokio::fs::create_dir_all(&src_dir).await.unwrap();
        let f1 = src_dir.join("hello.txt");
        let f2 = src_dir.join("data.bin");
        let f3 = src_dir.join("empty.dat");
        write_file(&f1, b"hello world").await;
        let big: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        write_file(&f2, &big).await;
        write_file(&f3, b"x").await;

        // ---- 由 A 主动调 send_files 发给 B ----
        let task_id = b_svc.clone(); // 借用避免被 drop
        let _ = task_id;
        let send_res = tokio::time::timeout(
            Duration::from_secs(10),
            a_svc
                .clone()
                .send_files(b_id.clone(), vec![f1.clone(), f2.clone(), f3.clone()]),
        )
        .await
        .expect("send timeout")
        .expect("send err");
        let _task_id = send_res;

        // 等接收侧处理完 done（handle_done 异步）
        wait_until(Duration::from_secs(5), || {
            let root = b_root.clone();
            async move {
                tokio::fs::metadata(root.join("bbb").join("hello.txt"))
                    .await
                    .is_ok()
                    && tokio::fs::metadata(root.join("bbb").join("data.bin"))
                        .await
                        .is_ok()
                    && tokio::fs::metadata(root.join("bbb").join("empty.dat"))
                        .await
                        .is_ok()
            }
        })
        .await
        .expect("files on disk");

        // ---- 校验内容 ----
        let r1 = tokio::fs::read(b_root.join("bbb").join("hello.txt"))
            .await
            .unwrap();
        assert_eq!(r1, b"hello world");
        let r2 = tokio::fs::read(b_root.join("bbb").join("data.bin"))
            .await
            .unwrap();
        assert_eq!(r2, big);
        let r3 = tokio::fs::read(b_root.join("bbb").join("empty.dat"))
            .await
            .unwrap();
        assert_eq!(r3, b"x");
    }

    async fn wait_until<F, Fut>(deadline: Duration, mut f: F) -> std::result::Result<(), ()>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let started = Instant::now();
        loop {
            if f().await {
                return Ok(());
            }
            if started.elapsed() > deadline {
                return Err(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}
