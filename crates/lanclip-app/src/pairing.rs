//! 设备配对状态机（双向确认，防止单方面伪造信任）。
//!
//! 安全模型（详见审查报告 #1）：
//! - `pair_code` 是 **确定性** 的（由两端 DeviceId 推导），仅作为双方肉眼核对的视觉校验码，
//!   **不能** 单独作为安全凭证 —— 因为 DeviceId 经 mDNS 明文广播，任何局域网设备都能算出它。
//! - 真正的防伪靠 **双向握手**：
//!   1. 本机点"配对" → 记 `pending_outgoing[peer]` + 发 `PairRequest`；
//!   2. 对端收到 `PairRequest` → 记 `pending_incoming[self]`（等用户确认，UI 高亮）；
//!   3. 对端点"确认" → 校验 `pending_incoming` 存在 → 写信任 + 回 `PairConfirm`；
//!   4. 本机收到 `PairConfirm` → **必须** 校验本机存在 `pending_outgoing[peer]` 才写信任。
//!
//! 第 4 步的校验是关键：攻击者即便伪造 `PairConfirm`，本机若从未发起过对它的配对请求，
//! 就不会有 `pending_outgoing`，confirm 会被丢弃。

use std::collections::HashSet;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use lanclip_domain::DeviceId;

/// pending 配对请求的有效期；超过则视为过期，需重新发起。
const PAIR_PENDING_TTL: Duration = Duration::from_secs(120);

#[derive(Clone)]
struct PendingEntry {
    peer: DeviceId,
    since: Instant,
}

#[derive(Default)]
struct Inner {
    /// 本机主动发起、尚未收到对端 confirm 的配对。
    outgoing: Vec<PendingEntry>,
    /// 收到对端 PairRequest、等待本机用户确认的配对。
    incoming: Vec<PendingEntry>,
}

/// 配对状态管理器（线程安全，可被 control_api 与 pairing bridge 共享）。
#[derive(Clone, Default)]
pub struct PairingManager {
    inner: Arc<RwLock<Inner>>,
}

impl PairingManager {
    pub fn new() -> Self {
        Self::default()
    }

    fn prune(inner: &mut Inner) {
        let now = Instant::now();
        inner
            .outgoing
            .retain(|e| now.duration_since(e.since) <= PAIR_PENDING_TTL);
        inner
            .incoming
            .retain(|e| now.duration_since(e.since) <= PAIR_PENDING_TTL);
    }

    /// 本机发起对 `peer` 的配对（记录 outgoing pending）。
    pub fn begin_outgoing(&self, peer: &DeviceId) {
        let mut inner = self.inner.write().expect("pairing lock poisoned");
        Self::prune(&mut inner);
        inner.outgoing.retain(|e| &e.peer != peer);
        inner.outgoing.push(PendingEntry {
            peer: peer.clone(),
            since: Instant::now(),
        });
    }

    /// 收到对端 `PairRequest`，记录为 incoming pending（等用户确认）。
    pub fn note_incoming_request(&self, peer: &DeviceId) {
        let mut inner = self.inner.write().expect("pairing lock poisoned");
        Self::prune(&mut inner);
        inner.incoming.retain(|e| &e.peer != peer);
        inner.incoming.push(PendingEntry {
            peer: peer.clone(),
            since: Instant::now(),
        });
    }

    /// 本机用户确认对 `peer` 的配对：消费 incoming pending。
    /// 返回 true 表示之前确实收到过该 peer 的请求（正常路径）。
    /// 即便返回 false 也允许继续（用户主动发起方向），但调用方应据此决定语义。
    pub fn consume_incoming(&self, peer: &DeviceId) -> bool {
        let mut inner = self.inner.write().expect("pairing lock poisoned");
        Self::prune(&mut inner);
        let before = inner.incoming.len();
        inner.incoming.retain(|e| &e.peer != peer);
        inner.incoming.len() != before
    }

    /// 收到对端 `PairConfirm`：仅当本机存在对应 outgoing pending 时才认可（消费它）。
    /// 返回 true 表示合法（本机确实发起过），调用方据此写入信任列表。
    pub fn accept_confirm(&self, peer: &DeviceId) -> bool {
        let mut inner = self.inner.write().expect("pairing lock poisoned");
        Self::prune(&mut inner);
        let before = inner.outgoing.len();
        inner.outgoing.retain(|e| &e.peer != peer);
        inner.outgoing.len() != before
    }

    /// 取消与某 peer 的所有 pending（用于解除配对/取消）。
    pub fn cancel(&self, peer: &DeviceId) {
        let mut inner = self.inner.write().expect("pairing lock poisoned");
        inner.outgoing.retain(|e| &e.peer != peer);
        inner.incoming.retain(|e| &e.peer != peer);
    }

    /// 当前等待本机确认的 incoming peer 列表（供 UI 高亮"对方请求配对"）。
    pub fn pending_incoming(&self) -> HashSet<DeviceId> {
        let mut inner = self.inner.write().expect("pairing lock poisoned");
        Self::prune(&mut inner);
        inner.incoming.iter().map(|e| e.peer.clone()).collect()
    }

    /// 当前本机已发起、等待对端确认的 outgoing peer 列表（供 UI 显示"等待对方确认"）。
    pub fn pending_outgoing(&self) -> HashSet<DeviceId> {
        let mut inner = self.inner.write().expect("pairing lock poisoned");
        Self::prune(&mut inner);
        inner.outgoing.iter().map(|e| e.peer.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(s: &str) -> DeviceId {
        DeviceId(s.into())
    }

    #[test]
    fn confirm_without_outgoing_is_rejected() {
        let m = PairingManager::new();
        // 没有发起过对 attacker 的配对 → 收到伪造 confirm 应被拒
        assert!(!m.accept_confirm(&dev("attacker")));
    }

    #[test]
    fn confirm_with_outgoing_is_accepted_once() {
        let m = PairingManager::new();
        m.begin_outgoing(&dev("peer"));
        assert!(m.accept_confirm(&dev("peer")), "first confirm accepted");
        // 重放同一 confirm 应被拒（pending 已消费）
        assert!(!m.accept_confirm(&dev("peer")), "replay rejected");
    }

    #[test]
    fn incoming_request_then_consume() {
        let m = PairingManager::new();
        assert!(!m.consume_incoming(&dev("peer")), "no request yet");
        m.note_incoming_request(&dev("peer"));
        assert!(m.consume_incoming(&dev("peer")), "request present");
    }

    #[test]
    fn cancel_clears_pending() {
        let m = PairingManager::new();
        m.begin_outgoing(&dev("peer"));
        m.note_incoming_request(&dev("peer"));
        m.cancel(&dev("peer"));
        assert!(!m.accept_confirm(&dev("peer")));
        assert!(!m.consume_incoming(&dev("peer")));
    }

    #[test]
    fn pending_sets_report_state() {
        let m = PairingManager::new();
        m.begin_outgoing(&dev("out"));
        m.note_incoming_request(&dev("in"));
        assert!(m.pending_outgoing().contains(&dev("out")));
        assert!(m.pending_incoming().contains(&dev("in")));
    }
}
