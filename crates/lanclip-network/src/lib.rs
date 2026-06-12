//! lanclip 网络层：WebSocket 监听 / 拨号 / 握手。
//!
//! 设计要点：
//! - 一条 WS 连接 = 1 个 writer task + 1 个 reader task；外部只通过 `mpsc` 与连接交互。
//! - 握手第一帧固定为 `Msg::Hello`，含 `role` (control/data) 和对端 `DevicePublic`。
//! - 不在这一层做连接池 / tie-break / 业务调度，那是 app 层的事。

// NetError 合理地包裹了体积较大的 `tungstenite::Error`，此处的 large-err 提示可忽略。
#![allow(clippy::result_large_err)]

use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use lanclip_proto::{ConnRole, DevicePublic, Msg, ProtoError, MIN_PROTOCOL_VERSION};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{accept_async, connect_async, WebSocketStream};
use tracing::{debug, info, warn};

/// 握手阶段（等待对端 Hello）最长等待时间，防止慢速/恶意连接长期占用 task。
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// WS 升级（accept / connect）最长等待时间。
pub const WS_UPGRADE_TIMEOUT: Duration = Duration::from_secs(10);

// ============================================================================
// 错误
// ============================================================================

#[derive(Debug, Error)]
pub enum NetError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("ws: {0}")]
    Ws(#[from] tokio_tungstenite::tungstenite::Error),

    #[error("proto: {0}")]
    Proto(#[from] ProtoError),

    #[error("expected Hello as first frame")]
    HelloExpected,

    #[error("incompatible protocol version: {0}")]
    IncompatibleVersion(u16),

    #[error("connection closed before handshake")]
    Closed,

    #[error("handshake timed out")]
    Timeout,
}

pub type Result<T> = std::result::Result<T, NetError>;

// ============================================================================
// 帧类型
// ============================================================================

/// 外部写入 WS 的一帧。
#[derive(Debug, Clone)]
pub enum OutFrame {
    /// JSON 控制帧（Text）。
    Text(String),
    /// 二进制数据帧。
    Binary(Bytes),
    /// 主动关闭连接（writer task 收到后会 close 并退出）。
    Close,
}

/// 从 WS 读到的一帧（已解析）。
#[derive(Debug)]
pub enum InFrame {
    /// 已解析的 JSON 控制帧。
    Msg(Msg),
    /// 二进制数据帧（文件 chunk）。
    Binary(Bytes),
}

// ============================================================================
// 连接句柄
// ============================================================================

/// 对端在 Hello 阶段声明的信息。
#[derive(Debug, Clone)]
pub struct RemoteHello {
    pub version: u16,
    pub role: ConnRole,
    pub device: DevicePublic,
}

/// 已握手的连接：出口 sender + 对端信息。
///
/// 配套的 inbound `Receiver<InFrame>` 由 `handshake` 一并返回，调用方持有并消费。
#[derive(Debug, Clone)]
pub struct ConnHandle {
    pub remote: RemoteHello,
    pub out: mpsc::Sender<OutFrame>,
}

impl ConnHandle {
    /// 发送一个控制消息（JSON）。
    pub async fn send_msg(&self, msg: &Msg) -> Result<()> {
        let s = msg.encode()?;
        self.out
            .send(OutFrame::Text(s))
            .await
            .map_err(|_| NetError::Closed)
    }

    /// 发送一帧二进制数据。
    pub async fn send_binary(&self, bytes: Bytes) -> Result<()> {
        self.out
            .send(OutFrame::Binary(bytes))
            .await
            .map_err(|_| NetError::Closed)
    }

    /// 主动关闭。
    pub async fn close(&self) {
        let _ = self.out.send(OutFrame::Close).await;
    }
}

// ============================================================================
// 监听 & 拨号
// ============================================================================

/// WS 服务端监听器。
pub struct WsListener {
    listener: TcpListener,
    local_addr: SocketAddr,
}

impl WsListener {
    /// 绑定到 `0.0.0.0:port`；`port = 0` 表示由 OS 分配。
    pub async fn bind(port: u16) -> Result<Self> {
        let listener = TcpListener::bind(("0.0.0.0", port)).await?;
        let local_addr = listener.local_addr()?;
        info!("ws listening on {local_addr}");
        Ok(Self {
            listener,
            local_addr,
        })
    }

    pub fn local_port(&self) -> u16 {
        self.local_addr.port()
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// 接受一条新连接并完成 WS 升级。
    pub async fn accept_one(&self) -> Result<(WebSocketStream<TcpStream>, SocketAddr)> {
        let (tcp, peer) = self.listener.accept().await?;
        let ws = tokio::time::timeout(WS_UPGRADE_TIMEOUT, accept_async(tcp))
            .await
            .map_err(|_| NetError::Timeout)??;
        debug!("ws accepted from {peer}");
        Ok((ws, peer))
    }

    /// 接受一条新连接，同时返回客户端请求的 **HTTP 路径**（用于区分 control / data 角色）。
    pub async fn accept_one_with_path(
        &self,
    ) -> Result<(WebSocketStream<TcpStream>, SocketAddr, String)> {
        let (tcp, peer) = self.listener.accept().await?;
        let path = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let path_for_cb = path.clone();
        let upgrade = tokio_tungstenite::accept_hdr_async(
            tcp,
            move |req: &tokio_tungstenite::tungstenite::handshake::server::Request,
                  resp: tokio_tungstenite::tungstenite::handshake::server::Response| {
                if let Ok(mut p) = path_for_cb.lock() {
                    *p = req.uri().path().to_string();
                }
                Ok(resp)
            },
        );
        let ws = tokio::time::timeout(WS_UPGRADE_TIMEOUT, upgrade)
            .await
            .map_err(|_| NetError::Timeout)??;
        let path_str = path.lock().expect("path lock poisoned").clone();
        debug!("ws accepted from {peer} path={path_str}");
        Ok((ws, peer, path_str))
    }
}

/// 主动连到 `ws://addr<path>`。
pub async fn dial(addr: SocketAddr, path: &str) -> Result<RawClientWs> {
    let url = format!("ws://{addr}{path}");
    debug!("ws dialing {url}");
    let (ws, _resp) = tokio::time::timeout(WS_UPGRADE_TIMEOUT, connect_async(&url))
        .await
        .map_err(|_| NetError::Timeout)??;
    Ok(ws)
}

/// 客户端拨号返回的 WS 流类型。
pub type RawClientWs = WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// 服务端 accept 返回的 WS 流类型。
pub type RawServerWs = WebSocketStream<tokio::net::TcpStream>;

// ============================================================================
// 握手
// ============================================================================

/// 在一条已升级的 WS 上完成握手：
/// 1. 先发自己的 Hello；
/// 2. 等对端第一帧必须为 Hello。
///
/// 成功后返回 `ConnHandle`（出口 sender）+ inbound `Receiver`。
pub async fn handshake<S>(
    ws: WebSocketStream<S>,
    self_hello: Msg,
) -> Result<(ConnHandle, mpsc::Receiver<InFrame>)>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    debug_assert!(
        matches!(self_hello, Msg::Hello { .. }),
        "self_hello must be Msg::Hello"
    );

    let (out_tx, mut in_rx) = spawn_conn(ws);

    // 1. 发出自己的 Hello
    out_tx
        .send(OutFrame::Text(self_hello.encode()?))
        .await
        .map_err(|_| NetError::Closed)?;

    // 2. 等对端 Hello（带超时，防 slowloris：连上却迟迟不发 Hello）
    let first = tokio::time::timeout(HANDSHAKE_TIMEOUT, in_rx.recv())
        .await
        .map_err(|_| NetError::Timeout)?
        .ok_or(NetError::Closed)?;
    let remote = match first {
        InFrame::Msg(Msg::Hello {
            version,
            role,
            device,
        }) => {
            if version < MIN_PROTOCOL_VERSION {
                return Err(NetError::IncompatibleVersion(version));
            }
            RemoteHello {
                version,
                role,
                device,
            }
        }
        _ => return Err(NetError::HelloExpected),
    };

    debug!(
        "handshake ok: peer={} role={:?} version={}",
        remote.device.id, remote.role, remote.version
    );

    Ok((
        ConnHandle {
            remote,
            out: out_tx,
        },
        in_rx,
    ))
}

// ============================================================================
// 内部：spawn 一条已升级 WS 的 read/write 双 task
// ============================================================================

/// 把一条已升级的 WS 拆成两个 task：
/// - writer：从 `out_rx` 读 `OutFrame`，写到 WS sink；
/// - reader：从 WS stream 读帧，解析后投递到 `in_tx`。
fn spawn_conn<S>(ws: WebSocketStream<S>) -> (mpsc::Sender<OutFrame>, mpsc::Receiver<InFrame>)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut sink, mut stream) = ws.split();
    let (out_tx, mut out_rx) = mpsc::channel::<OutFrame>(64);
    let (in_tx, in_rx) = mpsc::channel::<InFrame>(64);

    // writer
    tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            let ws_msg = match frame {
                OutFrame::Text(s) => WsMessage::Text(s.into()),
                OutFrame::Binary(b) => WsMessage::Binary(b),
                OutFrame::Close => {
                    let _ = sink.send(WsMessage::Close(None)).await;
                    break;
                }
            };
            if let Err(e) = sink.send(ws_msg).await {
                warn!("ws write error: {e}");
                break;
            }
        }
        let _ = sink.close().await;
        debug!("ws writer task exit");
    });

    // reader
    tokio::spawn(async move {
        while let Some(msg) = stream.next().await {
            match msg {
                Ok(WsMessage::Text(t)) => match Msg::decode(t.as_str()) {
                    Ok(m) => {
                        if in_tx.send(InFrame::Msg(m)).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => warn!("invalid json frame: {e}"),
                },
                Ok(WsMessage::Binary(b)) => {
                    if in_tx.send(InFrame::Binary(b)).await.is_err() {
                        break;
                    }
                }
                Ok(WsMessage::Ping(_)) | Ok(WsMessage::Pong(_)) => {
                    // tungstenite 默认会自动回 Pong；这里不处理
                }
                Ok(WsMessage::Close(_)) => {
                    debug!("ws peer closed");
                    break;
                }
                Ok(WsMessage::Frame(_)) => {} // raw frame, ignore
                Err(e) => {
                    warn!("ws read error: {e}");
                    break;
                }
            }
        }
        debug!("ws reader task exit");
    });

    (out_tx, in_rx)
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use lanclip_proto::{ConnRole, DevicePublic, PROTOCOL_VERSION};

    fn dummy_hello(id: &str, role: ConnRole) -> Msg {
        Msg::Hello {
            version: PROTOCOL_VERSION,
            role,
            device: DevicePublic {
                id: id.into(),
                name: format!("device-{id}"),
                os: "mac".into(),
            },
        }
    }

    /// 启动一个本地 server，client 拨号后双方握手 → 互发一条消息 → 关闭。
    #[tokio::test]
    async fn handshake_and_echo() {
        let listener = WsListener::bind(0).await.expect("bind");
        let addr: SocketAddr = ([127, 0, 0, 1], listener.local_port()).into();

        // server task：accept 一条并完成握手
        let server_task = tokio::spawn(async move {
            let (ws, _) = listener.accept_one().await.expect("accept");
            let (handle, mut rx) = handshake(ws, dummy_hello("server", ConnRole::Control))
                .await
                .expect("server handshake");
            // 读一条消息然后回一条
            match rx.recv().await {
                Some(InFrame::Msg(Msg::Ping { ts })) => {
                    handle.send_msg(&Msg::Pong { ts }).await.expect("send pong");
                }
                other => panic!("unexpected: {other:?}"),
            }
            handle.close().await;
        });

        // client：dial → 握手 → 发 ping → 等 pong
        let ws = dial(addr, lanclip_proto::WS_PATH_CONTROL)
            .await
            .expect("dial");
        let (handle, mut rx) = handshake(ws, dummy_hello("client", ConnRole::Control))
            .await
            .expect("client handshake");
        assert_eq!(handle.remote.device.id, "server");
        assert_eq!(handle.remote.role, ConnRole::Control);

        handle.send_msg(&Msg::Ping { ts: 42 }).await.expect("send");
        match rx.recv().await {
            Some(InFrame::Msg(Msg::Pong { ts })) => assert_eq!(ts, 42),
            other => panic!("unexpected: {other:?}"),
        }

        server_task.await.expect("server join");
    }
}
