use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use crate::core::types::{Channel, ChannelKind, FileTransfer, Message,
                          MessageStatus, NodeId, Peer, TransferStatus};

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

struct Inner {
    pub self_id:      String,
    pub nickname:     String,
    pub peers:        RwLock<HashMap<NodeId, Peer>>,
    pub channels:     Mutex<Vec<Channel>>,
    pub transfers:    Mutex<Vec<FileTransfer>>,
    pub active_ch:    Mutex<usize>,
    /// Join requests we've received as a group owner: group_name → [(id, nick)]
    pub pending_reqs: Mutex<Vec<(String, NodeId, String)>>,
    pub gateway_mode: std::sync::atomic::AtomicBool,
}

impl AppState {
    pub fn new(self_id: String, nickname: String) -> Self {
        let mut channels = vec![
            Channel::new("#general"),
            Channel::new("#recon"),
            Channel::new("#web-attack"),
        ];
        channels[0].messages.push(Message::system(
            "mshwisper online. Mesh discovery active. All traffic is end-to-end encrypted.".into(),
        ));
        Self(Arc::new(Inner {
            self_id, nickname,
            peers:        RwLock::new(HashMap::new()),
            channels:     Mutex::new(channels),
            transfers:    Mutex::new(Vec::new()),
            active_ch:    Mutex::new(0),
            pending_reqs: Mutex::new(Vec::new()),
            gateway_mode: std::sync::atomic::AtomicBool::new(false),
        }))
    }

    pub fn self_id(&self)  -> &str { &self.0.self_id }
    pub fn nickname(&self) -> &str { &self.0.nickname }

    // ── Peers ─────────────────────────────────────────────────────────────────
    pub async fn peers(&self) -> tokio::sync::RwLockReadGuard<'_, HashMap<NodeId, Peer>> {
        self.0.peers.read().await
    }
    pub async fn peer_count(&self) -> usize { self.0.peers.read().await.len() }
    pub async fn add_peer(&self, p: Peer)     { self.0.peers.write().await.insert(p.id.clone(), p); }
    pub async fn remove_peer(&self, id: &str) { self.0.peers.write().await.remove(id); }
    pub async fn update_peer_seen(&self, id: &str) {
        if let Some(p) = self.0.peers.write().await.get_mut(id) {
            p.last_seen = chrono::Utc::now();
        }
    }

    // ── Channels ──────────────────────────────────────────────────────────────
    pub async fn channels_len(&self) -> usize { self.0.channels.lock().await.len() }

    pub async fn channel_names(&self) -> Vec<String> {
        self.0.channels.lock().await.iter().map(|c| c.name.clone()).collect()
    }
    pub async fn channel_unreads(&self) -> Vec<usize> {
        self.0.channels.lock().await.iter().map(|c| c.unread).collect()
    }
    pub async fn channel_kinds(&self) -> Vec<ChannelKind> {
        self.0.channels.lock().await.iter().map(|c| c.kind.clone()).collect()
    }
    pub async fn active_channel_index(&self) -> usize { *self.0.active_ch.lock().await }

    pub async fn set_active_channel(&self, idx: usize) {
        let len = self.channels_len().await;
        if idx < len {
            *self.0.active_ch.lock().await = idx;
            self.0.channels.lock().await[idx].unread = 0;
        }
    }

    pub async fn add_channel(&self, name: String) {
        let mut ch = self.0.channels.lock().await;
        if !ch.iter().any(|c| c.name == name) { ch.push(Channel::new(name)); }
    }

    pub async fn add_group_channel(&self, name: String, creator_id: NodeId) -> bool {
        let mut ch = self.0.channels.lock().await;
        if ch.iter().any(|c| c.name == name) { return false; }
        ch.push(Channel::new_group(name, creator_id));
        true
    }

    pub async fn group_approve(&self, group_name: &str, member_id: NodeId) {
        let mut ch = self.0.channels.lock().await;
        if let Some(chan) = ch.iter_mut().find(|c| c.name == group_name) {
            if let ChannelKind::Group { members, pending, .. } = &mut chan.kind {
                pending.retain(|p| p != &member_id);
                if !members.contains(&member_id) { members.push(member_id); }
            }
        }
    }

    pub async fn group_add_pending(&self, group_name: &str, requester_id: NodeId) {
        let mut ch = self.0.channels.lock().await;
        if let Some(chan) = ch.iter_mut().find(|c| c.name == group_name) {
            if let ChannelKind::Group { pending, .. } = &mut chan.kind {
                if !pending.contains(&requester_id) { pending.push(requester_id); }
            }
        }
    }

    pub async fn push_message(&self, msg: Message) {
        let mut channels = self.0.channels.lock().await;
        let active = *self.0.active_ch.lock().await;
        let target = msg.channel.clone();
        if let Some(pos) = channels.iter().position(|c| c.name == target) {
            channels[pos].messages.push(msg);
            if pos != active { channels[pos].unread += 1; }
        } else {
            let mut ch = Channel::new(target);
            ch.messages.push(msg);
            channels.push(ch);
        }
    }

    pub async fn mark_delivered(&self, msg_id: &str) {
        let mut channels = self.0.channels.lock().await;
        for ch in channels.iter_mut() {
            for msg in ch.messages.iter_mut() {
                if msg.id == msg_id && msg.status == MessageStatus::Sent {
                    msg.status = MessageStatus::Delivered;
                    return;
                }
            }
        }
    }

    pub async fn mark_sent(&self, msg_id: &str) {
        let mut channels = self.0.channels.lock().await;
        for ch in channels.iter_mut() {
            for msg in ch.messages.iter_mut() {
                if msg.id == msg_id && msg.status == MessageStatus::Sending {
                    msg.status = MessageStatus::Sent;
                    return;
                }
            }
        }
    }

    pub async fn active_messages(&self) -> Vec<Message> {
        let channels = self.0.channels.lock().await;
        let idx = *self.0.active_ch.lock().await;
        channels.get(idx).map(|c| c.messages.clone()).unwrap_or_default()
    }

    // ── Transfers ─────────────────────────────────────────────────────────────
    pub async fn transfers(&self) -> Vec<FileTransfer> {
        self.0.transfers.lock().await.clone()
    }
    pub async fn add_transfer(&self, t: FileTransfer) {
        self.0.transfers.lock().await.push(t);
    }
    pub async fn update_transfer_progress(&self, id: &str, bytes_done: u64) {
        if let Some(t) = self.0.transfers.lock().await.iter_mut().find(|t| t.id == id) {
            t.transferred = bytes_done;
            t.status      = TransferStatus::Active;
        }
    }
    pub async fn complete_transfer(&self, id: &str, ok: bool, err: Option<String>) {
        if let Some(t) = self.0.transfers.lock().await.iter_mut().find(|t| t.id == id) {
            t.status = if ok { TransferStatus::Complete } else { TransferStatus::Failed(err.unwrap_or_default()) };
        }
    }

    // ── Pending join requests ─────────────────────────────────────────────────
    pub async fn pending_requests(&self) -> Vec<(String, NodeId, String)> {
        self.0.pending_reqs.lock().await.clone()
    }
    pub async fn add_pending_request(&self, group: String, id: NodeId, nick: String) {
        let mut p = self.0.pending_reqs.lock().await;
        if !p.iter().any(|(g, i, _)| g == &group && i == &id) {
            p.push((group, id, nick));
        }
    }
    pub async fn remove_pending_request(&self, group: &str, id: &str) {
        self.0.pending_reqs.lock().await.retain(|(g, i, _)| !(g == group && i == id));
    }

    // ── Gateway ───────────────────────────────────────────────────────────────
    pub fn is_gateway(&self) -> bool {
        self.0.gateway_mode.load(std::sync::atomic::Ordering::Relaxed)
    }
    pub fn set_gateway(&self, v: bool) {
        self.0.gateway_mode.store(v, std::sync::atomic::Ordering::Relaxed);
    }

    // ── Secure wipe on exit ───────────────────────────────────────────────────
    pub async fn wipe(&self) {
        for ch in self.0.channels.lock().await.iter_mut() { ch.messages.clear(); }
        self.0.peers.write().await.clear();
        self.0.transfers.lock().await.clear();
        self.0.pending_reqs.lock().await.clear();
    }
}
