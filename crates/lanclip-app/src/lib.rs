//! Application 层：服务编排 + 状态持有。
//!
//! M0 阶段提供：配置加载/保存、日志初始化、服务启动入口。
//! 连接编排（控制 + N 数据连接、tie-break、消息分发）将在 M2 完整实现。

pub mod clipboard_history;
pub mod clipboard_sync;
pub mod config;
pub mod connections;
pub mod history_store;
pub mod logging;
pub mod transfer_service;

use std::sync::Arc;

use lanclip_clipboard::ClipboardService;
use lanclip_discovery::{DiscoveryConfig, DiscoveryEvent, DiscoveryService};
use lanclip_domain::{DeviceId, OsKind};
use lanclip_network::WsListener;
use lanclip_proto::DevicePublic;
use tokio::sync::{broadcast, RwLock};
use tracing::{info, warn};

pub use clipboard_history::{ClipboardHistory, HistoryEntry, DEFAULT_MAX_ENTRIES};
pub use config::AppConfig;
pub use connections::{ConnEvent, ConnMgrError, ConnectionManager, NewDataConn};
pub use history_store::HistoryStore;
pub use lanclip_proto::Msg;
pub use transfer_service::{TransferService, TransferSvcConfig, TransferSvcError};

/// 应用句柄：UI 层只需持有它。
pub struct Application {
    pub config: Arc<RwLock<AppConfig>>,
    pub self_id: DeviceId,
    pub discovery: DiscoveryService,
    pub clipboard: Arc<ClipboardService>,
    pub clipboard_history: Arc<ClipboardHistory>,
    pub listener_port: u16,
    pub conn_mgr: Arc<ConnectionManager>,
    pub conn_event_rx: broadcast::Receiver<ConnEvent>,
    pub transfer: Arc<TransferService>,
}

impl Application {
    /// 启动所有后台服务。
    pub async fn start() -> anyhow::Result<Self> {
        let config = AppConfig::load_or_create()?;
        let self_id = config.device_id.clone();
        let self_name = config.device_name.clone();
        let self_os = OsKind::current();

        info!(
            "starting lanclip: device_id={}, name={}",
            self_id, self_name
        );

        // 1) WS 监听（端口由 OS 分配）
        let listener = WsListener::bind(0).await?;
        let listener_port = listener.local_port();

        // 2) 剪切板服务
        let (clipboard_raw, clipboard_local_rx) = ClipboardService::start()?;
        let clipboard = Arc::new(clipboard_raw);

        // 3) 发现服务
        let discovery = DiscoveryService::start(DiscoveryConfig {
            self_id: self_id.clone(),
            self_name: self_name.clone(),
            self_os,
            ws_port: listener_port,
        })?;

        // 4) 连接管理器
        let self_device = DevicePublic {
            id: self_id.0.clone(),
            name: self_name,
            os: self_os.to_string(),
        };
        let (conn_mgr, conn_event_rx, new_data_rx) =
            ConnectionManager::new(self_id.clone(), self_device);
        conn_mgr.start_listener(listener);

        // 5) 桥接 discovery → conn_mgr
        spawn_discovery_bridge(conn_mgr.clone(), discovery.subscribe());

        // 6) 剪切板历史 + 同步桥接（本机 ↔ 远端）
        let config_dir = AppConfig::config_dir()?;
        std::fs::create_dir_all(&config_dir)?;
        let db_path = config_dir.join("history.sqlite3");
        let store = match HistoryStore::open(&db_path) {
            Ok(s) => Some(s),
            Err(e) => {
                warn!(
                    "failed to open sqlite history store at {}: {e}",
                    db_path.display()
                );
                None
            }
        };
        let clipboard_history = ClipboardHistory::new(DEFAULT_MAX_ENTRIES, store);
        let download_root = config.download_dir.clone();
        let transfer_parallelism = config.transfer_parallelism;
        let auto_accept_transfer = config.auto_accept_transfer;
        let config_handle = Arc::new(RwLock::new(config));
        clipboard_sync::spawn(
            self_id.clone(),
            clipboard.clone(),
            clipboard_local_rx,
            conn_mgr.clone(),
            clipboard_history.clone(),
            config_handle.clone(),
        );
        spawn_pairing_bridge(self_id.clone(), config_handle.clone(), conn_mgr.subscribe());

        // 7) 文件传输服务
        let transfer = TransferService::spawn(
            self_id.clone(),
            conn_mgr.clone(),
            new_data_rx,
            TransferSvcConfig {
                download_root,
                parallelism: transfer_parallelism,
                auto_accept: auto_accept_transfer,
            },
        );

        Ok(Self {
            config: config_handle,
            self_id,
            discovery,
            clipboard,
            clipboard_history,
            listener_port,
            conn_mgr,
            conn_event_rx,
            transfer,
        })
    }
}

fn spawn_pairing_bridge(
    self_id: DeviceId,
    config: Arc<RwLock<AppConfig>>,
    mut events: broadcast::Receiver<ConnEvent>,
) {
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(ConnEvent::ControlMessage { peer_id, msg }) => match msg {
                    Msg::PairConfirm { code, .. } if code == pair_code(&self_id, &peer_id) => {
                        let mut cfg = config.write().await;
                        if !cfg.trusted_peers.iter().any(|id| id == &peer_id) {
                            cfg.trusted_peers.push(peer_id.clone());
                        }
                        if let Err(e) = cfg.save() {
                            warn!("failed to save trusted peer: {e}");
                        }
                    }
                    Msg::PairCancel { .. } => {
                        let mut cfg = config.write().await;
                        cfg.trusted_peers.retain(|id| id != &peer_id);
                        if let Err(e) = cfg.save() {
                            warn!("failed to save trusted peer removal: {e}");
                        }
                    }
                    Msg::PairRequest { code, .. } => {
                        info!("pair request from {peer_id}; confirmation code {code}");
                    }
                    _ => {}
                },
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!("pairing bridge lagged: {n} events skipped");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

fn pair_code(a: &DeviceId, b: &DeviceId) -> String {
    let (left, right) = if a <= b {
        (a.as_str(), b.as_str())
    } else {
        (b.as_str(), a.as_str())
    };
    let digest = blake3::hash(format!("lanclip-pair:{left}:{right}").as_bytes());
    let n = u32::from_be_bytes([
        digest.as_bytes()[0],
        digest.as_bytes()[1],
        digest.as_bytes()[2],
        digest.as_bytes()[3],
    ]) % 1_000_000;
    format!("{n:06}")
}

fn spawn_discovery_bridge(
    conn_mgr: Arc<ConnectionManager>,
    mut disco_rx: broadcast::Receiver<DiscoveryEvent>,
) {
    tokio::spawn(async move {
        loop {
            match disco_rx.recv().await {
                Ok(DiscoveryEvent::PeerAdded(p)) | Ok(DiscoveryEvent::PeerUpdated(p)) => {
                    conn_mgr
                        .try_dial_control(p.device.id.clone(), p.addrs.clone())
                        .await;
                }
                Ok(DiscoveryEvent::PeerLost(id)) => {
                    conn_mgr.drop_peer(&id).await;
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!("discovery bridge lagged: {n} events skipped");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}
