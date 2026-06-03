//! 局域网设备发现 —— mDNS / DNS-SD。
//!
//! 服务类型 `_lanclip._tcp.local.` 是本软件独有的，自然把同款设备过滤出来。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lanclip_domain::{Device, DeviceId, OsKind, Peer, PeerStatus};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

/// 本软件的 mDNS 服务类型。**独有** —— 只有装了 lanclip 的设备会注册/响应。
pub const SERVICE_TYPE: &str = "_lanclip._tcp.local.";

/// 在 mDNS TXT 里固定的 key。
const TXT_ID: &str = "id";
const TXT_NAME: &str = "name";
const TXT_OS: &str = "os";
const TXT_VERSION: &str = "v";
const TXT_PORT: &str = "port";

/// Peer 离线判定：超过这个时长未刷新视为 Offline。
pub const PEER_TTL: Duration = Duration::from_secs(30);

// ============================================================================
// 配置 & 事件
// ============================================================================

#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    pub self_id: DeviceId,
    pub self_name: String,
    pub self_os: OsKind,
    pub ws_port: u16,
}

#[derive(Debug, Clone)]
pub enum DiscoveryEvent {
    /// 新发现一个 peer（首次解析成功）。
    PeerAdded(Peer),
    /// 已知 peer 的字段（地址/名字/端口）更新。
    PeerUpdated(Peer),
    /// peer 离线或被移除。
    PeerLost(DeviceId),
}

// ============================================================================
// 服务
// ============================================================================

pub struct DiscoveryService {
    daemon: ServiceDaemon,
    peers: Arc<Mutex<HashMap<DeviceId, Peer>>>,
    event_tx: broadcast::Sender<DiscoveryEvent>,
    self_id: DeviceId,
}

impl DiscoveryService {
    /// 启动 mDNS：注册自身并开始 browse。
    pub fn start(config: DiscoveryConfig) -> anyhow::Result<Self> {
        let daemon = ServiceDaemon::new()?;

        Self::register_self(&daemon, &config)?;

        let receiver = daemon.browse(SERVICE_TYPE)?;
        let (event_tx, _) = broadcast::channel(64);
        let peers: Arc<Mutex<HashMap<DeviceId, Peer>>> = Arc::new(Mutex::new(HashMap::new()));

        // 转发 mDNS 事件 → broadcast
        {
            let event_tx = event_tx.clone();
            let peers = peers.clone();
            let self_id = config.self_id.clone();
            tokio::spawn(async move {
                while let Ok(event) = receiver.recv_async().await {
                    handle_event(event, &peers, &event_tx, &self_id);
                }
                debug!("mdns browse receiver closed");
            });
        }

        info!(
            "discovery started: self_id={}, name={}, port={}",
            config.self_id, config.self_name, config.ws_port
        );

        Ok(Self {
            daemon,
            peers,
            event_tx,
            self_id: config.self_id,
        })
    }

    fn register_self(daemon: &ServiceDaemon, config: &DiscoveryConfig) -> anyhow::Result<()> {
        let os_str = config.self_os.to_string();
        let port_str = config.ws_port.to_string();
        let version_str = "1".to_string();

        let props: Vec<(&str, &str)> = vec![
            (TXT_ID, config.self_id.as_str()),
            (TXT_NAME, config.self_name.as_str()),
            (TXT_OS, os_str.as_str()),
            (TXT_VERSION, version_str.as_str()),
            (TXT_PORT, port_str.as_str()),
        ];

        // instance_name 用 device_id 前缀保证唯一；host_name 必须 .local. 结尾。
        let instance_name = format!("lanclip-{}", short_id(&config.self_id));
        let host_name = format!("{instance_name}.local.");

        let info = ServiceInfo::new(
            SERVICE_TYPE,
            &instance_name,
            &host_name,
            "", // 空 → 让 mdns-sd 自动侦测本机 IP
            config.ws_port,
            &props[..],
        )?;

        daemon.register(info)?;
        Ok(())
    }

    /// 订阅事件流。
    pub fn subscribe(&self) -> broadcast::Receiver<DiscoveryEvent> {
        self.event_tx.subscribe()
    }

    /// 当前已知 peer 的快照（不含离线超时的）。
    pub fn peers_snapshot(&self) -> Vec<Peer> {
        let guard = self.peers.lock().expect("peers mutex poisoned");
        guard.values().cloned().collect()
    }

    /// 主动清理超时 peer。建议由 app 层每隔 N 秒调用一次。
    pub fn sweep_stale(&self) -> Vec<DeviceId> {
        let mut removed = Vec::new();
        {
            let mut guard = self.peers.lock().expect("peers mutex poisoned");
            guard.retain(|id, peer| {
                if peer.is_stale(PEER_TTL) {
                    removed.push(id.clone());
                    false
                } else {
                    true
                }
            });
        }
        for id in &removed {
            let _ = self.event_tx.send(DiscoveryEvent::PeerLost(id.clone()));
        }
        removed
    }

    pub fn self_id(&self) -> &DeviceId {
        &self.self_id
    }

    pub fn shutdown(&self) -> anyhow::Result<()> {
        // 0.x API: shutdown 返回 Result<Receiver<DaemonStatus>>；这里只关心成功触发。
        let _ = self.daemon.shutdown()?;
        Ok(())
    }
}

// ============================================================================
// 事件处理
// ============================================================================

fn handle_event(
    event: ServiceEvent,
    peers: &Mutex<HashMap<DeviceId, Peer>>,
    tx: &broadcast::Sender<DiscoveryEvent>,
    self_id: &DeviceId,
) {
    match event {
        ServiceEvent::ServiceResolved(info) => {
            if let Some(peer) = parse_service_info(&info) {
                // 过滤掉自己
                if &peer.device.id == self_id {
                    return;
                }

                let (is_new, peer_out) = {
                    let mut guard = peers.lock().expect("peers mutex poisoned");
                    let is_new = !guard.contains_key(&peer.device.id);
                    guard.insert(peer.device.id.clone(), peer.clone());
                    (is_new, peer)
                };

                let evt = if is_new {
                    info!(
                        "peer discovered: {} ({})",
                        peer_out.device.name, peer_out.device.id
                    );
                    DiscoveryEvent::PeerAdded(peer_out)
                } else {
                    DiscoveryEvent::PeerUpdated(peer_out)
                };
                let _ = tx.send(evt);
            } else {
                warn!(
                    "mdns service resolved but missing required TXT fields: {}",
                    info.get_fullname()
                );
            }
        }
        ServiceEvent::ServiceRemoved(_ty, fullname) => {
            // fullname 形如 "lanclip-xxxx._lanclip._tcp.local."
            // 我们不能从 fullname 直接还原 DeviceId，依靠后续 sweep_stale 兜底。
            debug!("mdns service removed: {fullname}");
        }
        other => {
            debug!("mdns other event: {other:?}");
        }
    }
}

fn parse_service_info(info: &ServiceInfo) -> Option<Peer> {
    let props = info.get_properties();

    let id = props.get_property_val_str(TXT_ID)?;
    let name = props.get_property_val_str(TXT_NAME)?;
    let os = props.get_property_val_str(TXT_OS)?;
    let port_str = props.get_property_val_str(TXT_PORT)?;
    let port: u16 = port_str.parse().ok()?;

    let device = Device {
        id: DeviceId(id.to_string()),
        name: name.to_string(),
        os: parse_os(os),
    };

    let addrs: Vec<SocketAddr> = info
        .get_addresses()
        .iter()
        .map(|ip| SocketAddr::new(*ip, port))
        .collect();

    Some(Peer {
        device,
        addrs,
        status: PeerStatus::Online,
        last_seen: Instant::now(),
    })
}

fn parse_os(s: &str) -> OsKind {
    match s {
        "mac" => OsKind::Mac,
        "windows" => OsKind::Windows,
        "linux" => OsKind::Linux,
        "android" => OsKind::Android,
        _ => OsKind::Unknown,
    }
}

fn short_id(id: &DeviceId) -> String {
    id.as_str().chars().take(8).collect()
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_id_truncates() {
        let id = DeviceId("12345678-aaaa-bbbb-cccc-dddddddddddd".into());
        assert_eq!(short_id(&id), "12345678");
    }

    #[test]
    fn parse_os_known() {
        assert_eq!(parse_os("mac"), OsKind::Mac);
        assert_eq!(parse_os("linux"), OsKind::Linux);
        assert_eq!(parse_os("unknown"), OsKind::Unknown);
    }
}
