//! 连接管理：1 控制连接 + N 数据连接（per peer），带 tie-break 防重连。
//!
//! 调用约定：
//! - **控制连接**由 `ConnectionManager` 持有，inbound 消息通过 `ConnEvent::ControlMessage` 广播给订阅者。
//! - **数据连接**调用方持有（一次性使用），inbound 流由调用方自行消费。
//!
//! Tie-break：`self_id > peer_id` 主动 dial；否则被动等。**双方仅一条控制连接**。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use lanclip_domain::DeviceId;
use lanclip_network::{dial, handshake, ConnHandle, InFrame, NetError, RawServerWs, WsListener};
use lanclip_proto::{ConnRole, DevicePublic, Msg, PROTOCOL_VERSION, WS_PATH_CONTROL, WS_PATH_DATA};
use thiserror::Error;
use tokio::sync::{broadcast, mpsc, Mutex};
use tracing::{debug, info, warn};

// ============================================================================
// 错误
// ============================================================================

#[derive(Debug, Error)]
pub enum ConnMgrError {
    #[error("network: {0}")]
    Net(#[from] NetError),

    #[error("peer not connected: {0}")]
    NotConnected(DeviceId),

    #[error("no known address for peer: {0}")]
    NoAddress(DeviceId),

    #[error("incoming role mismatch: path={path:?} role={role:?}")]
    RoleMismatch { path: String, role: ConnRole },
}

pub type Result<T> = std::result::Result<T, ConnMgrError>;

// ============================================================================
// 事件 & 数据连接通知
// ============================================================================

#[derive(Debug, Clone)]
pub enum ConnEvent {
    ControlConnected {
        peer_id: DeviceId,
        device: DevicePublic,
    },
    ControlDisconnected {
        peer_id: DeviceId,
    },
    ControlMessage {
        peer_id: DeviceId,
        msg: Msg,
    },
}

/// 新数据连接到达通知（无论主动 / 被动）。调用方负责消费 inbound 并最终 drop handle 关闭。
pub struct NewDataConn {
    pub peer_id: DeviceId,
    pub handle: ConnHandle,
    pub inbound: mpsc::Receiver<InFrame>,
}

// ============================================================================
// ConnectionManager
// ============================================================================

pub struct ConnectionManager {
    self_id: DeviceId,
    self_device: DevicePublic,
    state: Mutex<ConnState>,
    event_tx: broadcast::Sender<ConnEvent>,
    new_data_tx: mpsc::Sender<NewDataConn>,
}

#[derive(Default)]
struct ConnState {
    peers: HashMap<DeviceId, PeerEntry>,
}

#[derive(Default)]
struct PeerEntry {
    control: Option<ConnHandle>,
    addrs: Vec<SocketAddr>,
    /// 是否正在 dial 控制连接，避免重复发起。
    control_dialing: bool,
}

impl ConnectionManager {
    /// 创建管理器。返回 (mgr, event_rx, new_data_rx)。
    pub fn new(
        self_id: DeviceId,
        self_device: DevicePublic,
    ) -> (
        Arc<Self>,
        broadcast::Receiver<ConnEvent>,
        mpsc::Receiver<NewDataConn>,
    ) {
        let (event_tx, event_rx) = broadcast::channel(256);
        let (new_data_tx, new_data_rx) = mpsc::channel(16);
        let mgr = Arc::new(Self {
            self_id,
            self_device,
            state: Mutex::new(ConnState::default()),
            event_tx,
            new_data_tx,
        });
        (mgr, event_rx, new_data_rx)
    }

    pub fn self_id(&self) -> &DeviceId {
        &self.self_id
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ConnEvent> {
        self.event_tx.subscribe()
    }

    // ------------------------------------------------------------------------
    // 监听 & 入站处理
    // ------------------------------------------------------------------------

    /// 启动 accept loop（spawn 一个常驻 task）。listener 被 task 持有。
    pub fn start_listener(self: &Arc<Self>, listener: WsListener) {
        let mgr = self.clone();
        tokio::spawn(async move {
            info!(
                "connection manager accept loop on port {}",
                listener.local_port()
            );
            loop {
                match listener.accept_one_with_path().await {
                    Ok((ws, peer_addr, path)) => {
                        let mgr = mgr.clone();
                        tokio::spawn(async move {
                            if let Err(e) = mgr.handle_incoming(ws, peer_addr, path).await {
                                warn!("incoming handshake failed: {e}");
                            }
                        });
                    }
                    Err(e) => {
                        warn!("accept failed: {e}");
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                }
            }
        });
    }

    async fn handle_incoming(
        self: Arc<Self>,
        ws: RawServerWs,
        _peer_addr: SocketAddr,
        path: String,
    ) -> Result<()> {
        // 根据 URL 路径确定本端 role；不识别则按 control 处理（兼容客户端 fallback）
        let role = match path.as_str() {
            WS_PATH_CONTROL => ConnRole::Control,
            WS_PATH_DATA => ConnRole::Data,
            _ => ConnRole::Control,
        };

        let self_hello = self.make_hello(role);
        let (conn, inbound) = handshake(ws, self_hello).await?;

        if conn.remote.role != role {
            return Err(ConnMgrError::RoleMismatch {
                path,
                role: conn.remote.role,
            });
        }

        self.clone().attach_connection(conn, inbound).await
    }

    // ------------------------------------------------------------------------
    // 主动建连
    // ------------------------------------------------------------------------

    /// 更新对 peer 的已知地址（来自 mdns）。
    pub async fn update_peer_addrs(&self, peer_id: &DeviceId, addrs: Vec<SocketAddr>) {
        let mut st = self.state.lock().await;
        let entry = st.peers.entry(peer_id.clone()).or_default();
        entry.addrs = addrs;
    }

    /// 按需建立控制连接（带 tie-break）。
    ///
    /// 规则：仅当 `self_id > peer_id` 时主动 dial；否则等对方来连。
    /// 已连接 / 正在建立 → 直接返回。
    pub async fn try_dial_control(self: &Arc<Self>, peer_id: DeviceId, addrs: Vec<SocketAddr>) {
        // 1. tie-break：小 id 不主动
        if &self.self_id <= &peer_id {
            debug!("tie-break: self_id <= peer_id, wait for incoming control from {peer_id}");
            // 仍然存一下地址（万一需要主动发数据连接）
            self.update_peer_addrs(&peer_id, addrs).await;
            return;
        }

        // 2. 是否已连 / 正在 dial
        {
            let mut st = self.state.lock().await;
            let entry = st.peers.entry(peer_id.clone()).or_default();
            entry.addrs = addrs.clone();
            if entry.control.is_some() || entry.control_dialing {
                return;
            }
            entry.control_dialing = true;
        }

        let mgr = self.clone();
        let pid = peer_id.clone();
        tokio::spawn(async move {
            let result = mgr.dial_and_attach_control(&pid, &addrs).await;
            // 无论成功失败，清除 dialing 标记
            {
                let mut st = mgr.state.lock().await;
                if let Some(entry) = st.peers.get_mut(&pid) {
                    entry.control_dialing = false;
                }
            }
            if let Err(e) = result {
                warn!("dial control to {pid} failed: {e}");
            }
        });
    }

    async fn dial_and_attach_control(
        self: &Arc<Self>,
        peer_id: &DeviceId,
        addrs: &[SocketAddr],
    ) -> Result<()> {
        let addr = *addrs
            .first()
            .ok_or_else(|| ConnMgrError::NoAddress(peer_id.clone()))?;
        let ws = dial(addr, WS_PATH_CONTROL).await?;
        let self_hello = self.make_hello(ConnRole::Control);
        let (conn, inbound) = handshake(ws, self_hello).await?;

        if conn.remote.role != ConnRole::Control {
            return Err(ConnMgrError::RoleMismatch {
                path: WS_PATH_CONTROL.into(),
                role: conn.remote.role,
            });
        }
        let remote_id = DeviceId(conn.remote.device.id.clone());
        if &remote_id != peer_id {
            warn!("dialed {peer_id} but handshake returned {remote_id}");
        }
        self.clone().attach_connection(conn, inbound).await
    }

    /// 主动建立一条数据连接（用于发送端）。
    pub async fn dial_data(
        self: &Arc<Self>,
        peer_id: &DeviceId,
    ) -> Result<(ConnHandle, mpsc::Receiver<InFrame>)> {
        let addr = {
            let st = self.state.lock().await;
            st.peers
                .get(peer_id)
                .and_then(|e| e.addrs.first().copied())
                .ok_or_else(|| ConnMgrError::NoAddress(peer_id.clone()))?
        };

        let ws = dial(addr, WS_PATH_DATA).await?;
        let self_hello = self.make_hello(ConnRole::Data);
        let (conn, inbound) = handshake(ws, self_hello).await?;

        if conn.remote.role != ConnRole::Data {
            return Err(ConnMgrError::RoleMismatch {
                path: WS_PATH_DATA.into(),
                role: conn.remote.role,
            });
        }

        Ok((conn, inbound))
    }

    // ------------------------------------------------------------------------
    // 入栈
    // ------------------------------------------------------------------------

    async fn attach_connection(
        self: Arc<Self>,
        conn: ConnHandle,
        inbound: mpsc::Receiver<InFrame>,
    ) -> Result<()> {
        let peer_id = DeviceId(conn.remote.device.id.clone());
        let device = conn.remote.device.clone();

        match conn.remote.role {
            ConnRole::Control => {
                // 占用槽位
                let replaced = {
                    let mut st = self.state.lock().await;
                    let entry = st.peers.entry(peer_id.clone()).or_default();
                    entry.control.replace(conn.clone()).is_some()
                };
                if replaced {
                    debug!("control conn for {peer_id} replaced (old dropped)");
                }
                info!("control connected: {} ({})", device.name, device.id);

                // 通知
                let _ = self.event_tx.send(ConnEvent::ControlConnected {
                    peer_id: peer_id.clone(),
                    device,
                });

                // spawn inbound forwarder
                let mgr = self.clone();
                let pid = peer_id.clone();
                tokio::spawn(async move {
                    forward_control_inbound(mgr, pid, inbound).await;
                });
            }
            ConnRole::Data => {
                debug!("data connected from {}", device.id);
                let notify = NewDataConn {
                    peer_id: peer_id.clone(),
                    handle: conn,
                    inbound,
                };
                if let Err(e) = self.new_data_tx.send(notify).await {
                    warn!("new_data_tx send failed: {e}");
                }
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------------
    // 发送
    // ------------------------------------------------------------------------

    /// 通过 peer 的控制连接发一条消息。
    pub async fn send_control(&self, peer_id: &DeviceId, msg: &Msg) -> Result<()> {
        let handle = {
            let st = self.state.lock().await;
            st.peers
                .get(peer_id)
                .and_then(|e| e.control.clone())
                .ok_or_else(|| ConnMgrError::NotConnected(peer_id.clone()))?
        };
        handle.send_msg(msg).await?;
        Ok(())
    }

    /// 广播到所有已连接 peer 的控制连接。返回成功发出的条数。
    pub async fn broadcast_control(&self, msg: &Msg) -> usize {
        let handles: Vec<(DeviceId, ConnHandle)> = {
            let st = self.state.lock().await;
            st.peers
                .iter()
                .filter_map(|(id, e)| e.control.clone().map(|h| (id.clone(), h)))
                .collect()
        };
        let mut ok = 0;
        for (id, handle) in handles {
            match handle.send_msg(msg).await {
                Ok(()) => ok += 1,
                Err(e) => warn!("broadcast to {id} failed: {e}"),
            }
        }
        ok
    }

    pub async fn connected_peers(&self) -> Vec<DeviceId> {
        let st = self.state.lock().await;
        st.peers
            .iter()
            .filter(|(_, e)| e.control.is_some())
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub async fn drop_peer(&self, peer_id: &DeviceId) {
        let mut st = self.state.lock().await;
        if let Some(entry) = st.peers.get_mut(peer_id) {
            if let Some(h) = entry.control.take() {
                h.close().await;
            }
        }
        let _ = self.event_tx.send(ConnEvent::ControlDisconnected {
            peer_id: peer_id.clone(),
        });
    }

    // ------------------------------------------------------------------------
    // helpers
    // ------------------------------------------------------------------------

    fn make_hello(&self, role: ConnRole) -> Msg {
        Msg::Hello {
            version: PROTOCOL_VERSION,
            role,
            device: self.self_device.clone(),
        }
    }
}

// ============================================================================
// 控制连接 inbound 转发
// ============================================================================

async fn forward_control_inbound(
    mgr: Arc<ConnectionManager>,
    peer_id: DeviceId,
    mut inbound: mpsc::Receiver<InFrame>,
) {
    while let Some(frame) = inbound.recv().await {
        match frame {
            InFrame::Msg(msg) => {
                let _ = mgr.event_tx.send(ConnEvent::ControlMessage {
                    peer_id: peer_id.clone(),
                    msg,
                });
            }
            InFrame::Binary(_) => {
                warn!("unexpected binary frame on control conn from {peer_id}");
            }
        }
    }
    // inbound 关闭 → 连接断开
    debug!("control inbound for {peer_id} closed");
    {
        let mut st = mgr.state.lock().await;
        if let Some(entry) = st.peers.get_mut(&peer_id) {
            entry.control = None;
        }
    }
    let _ = mgr
        .event_tx
        .send(ConnEvent::ControlDisconnected { peer_id });
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{timeout, Duration};

    fn make_device(id: &str) -> DevicePublic {
        DevicePublic {
            id: id.into(),
            name: format!("device-{id}"),
            os: "mac".into(),
        }
    }

    /// 两个 ConnectionManager 互连：A 主动 dial B（tie-break），双方握手成功，互发 Ping/Pong。
    #[tokio::test]
    async fn two_managers_handshake_and_exchange() {
        // a_id > b_id 字典序 → A 主动 dial
        let a_id = DeviceId("bbb".into());
        let b_id = DeviceId("aaa".into());

        let (a_mgr, mut a_events, _a_data) =
            ConnectionManager::new(a_id.clone(), make_device("bbb"));
        let (b_mgr, mut b_events, _b_data) =
            ConnectionManager::new(b_id.clone(), make_device("aaa"));

        let a_listener = WsListener::bind(0).await.expect("a bind");
        let b_listener = WsListener::bind(0).await.expect("b bind");
        let b_port = b_listener.local_port();

        a_mgr.start_listener(a_listener);
        b_mgr.start_listener(b_listener);

        let b_addr: SocketAddr = ([127, 0, 0, 1], b_port).into();
        a_mgr.try_dial_control(b_id.clone(), vec![b_addr]).await;

        // 双方都应该收到 ControlConnected
        let a_evt = timeout(Duration::from_secs(2), a_events.recv())
            .await
            .expect("a evt timeout")
            .expect("a evt closed");
        assert!(
            matches!(&a_evt, ConnEvent::ControlConnected { peer_id, .. } if peer_id == &b_id),
            "unexpected a evt: {a_evt:?}"
        );

        let b_evt = timeout(Duration::from_secs(2), b_events.recv())
            .await
            .expect("b evt timeout")
            .expect("b evt closed");
        assert!(
            matches!(&b_evt, ConnEvent::ControlConnected { peer_id, .. } if peer_id == &a_id),
            "unexpected b evt: {b_evt:?}"
        );

        // A 发 Ping → B 收到
        a_mgr
            .send_control(&b_id, &Msg::Ping { ts: 42 })
            .await
            .expect("a send ping");
        let b_evt = timeout(Duration::from_secs(2), b_events.recv())
            .await
            .expect("b ping timeout")
            .expect("b evt closed");
        assert!(
            matches!(
                &b_evt,
                ConnEvent::ControlMessage { peer_id, msg: Msg::Ping { ts: 42 } } if peer_id == &a_id
            ),
            "unexpected b evt: {b_evt:?}"
        );

        // B 广播 Pong → A 收到
        let n = b_mgr.broadcast_control(&Msg::Pong { ts: 42 }).await;
        assert_eq!(n, 1, "should broadcast to 1 peer");
        let a_evt = timeout(Duration::from_secs(2), a_events.recv())
            .await
            .expect("a pong timeout")
            .expect("a evt closed");
        assert!(
            matches!(
                &a_evt,
                ConnEvent::ControlMessage { peer_id, msg: Msg::Pong { ts: 42 } } if peer_id == &b_id
            ),
            "unexpected a evt: {a_evt:?}"
        );

        // peer 列表正确
        assert_eq!(a_mgr.connected_peers().await, vec![b_id.clone()]);
        assert_eq!(b_mgr.connected_peers().await, vec![a_id.clone()]);
    }

    /// Tie-break：self_id <= peer_id 时不主动 dial。
    #[tokio::test]
    async fn tie_break_smaller_id_waits() {
        let a_id = DeviceId("aaa".into());
        let b_id = DeviceId("bbb".into());

        let (a_mgr, mut a_events, _a_data) = ConnectionManager::new(a_id, make_device("aaa"));

        let b_addr: SocketAddr = ([127, 0, 0, 1], 1).into();
        a_mgr.try_dial_control(b_id, vec![b_addr]).await;

        // 不应该收到 ControlConnected（也不应该有真的 dial）
        let res = timeout(Duration::from_millis(300), a_events.recv()).await;
        assert!(res.is_err(), "should not receive any event (no dial)");
    }
}
