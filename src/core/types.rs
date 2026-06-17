use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

pub type NodeId = String;

// ── Peer ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Peer {
    pub id:         NodeId,
    pub nickname:   String,
    pub addr:       SocketAddr,
    pub subnet:     String,
    pub last_seen:  DateTime<Utc>,
    pub is_gateway: bool,
}
impl Peer {
    pub fn new(id: NodeId, nickname: String, addr: SocketAddr) -> Self {
        let subnet = subnet_from_addr(&addr);
        Self { id, nickname, addr, subnet, last_seen: Utc::now(), is_gateway: false }
    }
}

// ── Channel / Group ───────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Channel {
    pub name:       String,
    pub messages:   Vec<Message>,
    pub unread:     usize,
    pub kind:       ChannelKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ChannelKind {
    /// Open channel — anyone on the mesh can join by typing /join
    Open,
    /// Private group — must request to join; creator approves
    Group { creator_id: NodeId, members: Vec<NodeId>, pending: Vec<NodeId> },
}

impl Channel {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), messages: Vec::new(), unread: 0, kind: ChannelKind::Open }
    }
    pub fn new_group(name: impl Into<String>, creator_id: NodeId) -> Self {
        let creator = creator_id.clone();
        Self {
            name: name.into(),
            messages: Vec::new(),
            unread: 0,
            kind: ChannelKind::Group {
                creator_id,
                members: vec![creator],
                pending: Vec::new(),
            },
        }
    }
    #[allow(dead_code)]
    pub fn is_group(&self) -> bool { matches!(self.kind, ChannelKind::Group { .. }) }
    #[allow(dead_code)]
    pub fn is_member(&self, node_id: &str) -> bool {
        match &self.kind {
            ChannelKind::Open => true,
            ChannelKind::Group { members, .. } => members.iter().any(|m| m == node_id),
        }
    }
}

// ── Message ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub id:          String,
    pub sender_id:   NodeId,
    pub sender_nick: String,
    pub channel:     String,
    pub content:     MessageContent,
    pub timestamp:   DateTime<Utc>,
    pub status:      MessageStatus,
}

/// Delivery status — shown as tick marks (WhatsApp-style)
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MessageStatus {
    /// Only exists locally (e.g. outgoing before any peer ack)
    Sending,
    /// Sent to at least one peer (single tick ✓)
    Sent,
    /// Delivered to all currently-connected peers (double tick ✓✓)
    Delivered,
    /// Read acknowledgement (not yet implemented — reserved)
    Read,
    /// System-generated, no tick shown
    System,
}

impl Message {
    pub fn chat(sid: &str, nick: &str, ch: &str, text: String) -> Self {
        Self {
            id: new_id(), sender_id: sid.into(), sender_nick: nick.into(),
            channel: ch.into(), content: MessageContent::Chat(text),
            timestamp: Utc::now(), status: MessageStatus::Sending,
        }
    }
    pub fn system(text: String) -> Self {
        Self {
            id: new_id(), sender_id: "system".into(), sender_nick: "system".into(),
            channel: "#general".into(), content: MessageContent::System(text),
            timestamp: Utc::now(), status: MessageStatus::System,
        }
    }
    pub fn command(sid: &str, nick: &str, ch: &str, cmd: String) -> Self {
        Self {
            id: new_id(), sender_id: sid.into(), sender_nick: nick.into(),
            channel: ch.into(), content: MessageContent::Command(cmd),
            timestamp: Utc::now(), status: MessageStatus::Sending,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum MessageContent {
    Chat(String),
    System(String),
    Command(String),
    FileOffer { name: String, size: u64, transfer_id: String },
}

// ── File Transfer ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct FileTransfer {
    pub id:          String,
    pub filename:    String,
    pub size:        u64,
    pub transferred: u64,
    pub direction:   TransferDirection,
    pub peer_nick:   String,
    pub status:      TransferStatus,
}
impl FileTransfer {
    pub fn progress_pct(&self) -> u8 {
        if self.size == 0 { return 100; }
        ((self.transferred as f64 / self.size as f64) * 100.0).min(100.0) as u8
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum TransferDirection { Sending, Receiving }

#[derive(Clone, Debug, PartialEq)]
pub enum TransferStatus {
    Pending,
    Active,
    Complete,
    Failed(String),
}

// ── Group management wire messages ────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GroupInfo {
    pub name:       String,
    pub creator_id: NodeId,
    pub members:    Vec<NodeId>,
}

// ── Wire protocol ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum WirePacket {
    Hello          { node_id: NodeId, nickname: String, tcp_port: u16, is_gateway: bool },
    KeyExchange    { node_id: NodeId, public_key: Vec<u8> },
    KeyExchangeAck { node_id: NodeId, public_key: Vec<u8> },
    Encrypted      { from_id: NodeId, nonce: Vec<u8>, ciphertext: Vec<u8> },
    Ping           { node_id: NodeId },
    Goodbye        { node_id: NodeId },
    FileChunk      { transfer_id: String, chunk_index: u64, data: Vec<u8>, is_last: bool },
}

/// Inner decrypted payload sent between peers
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AppMessage {
    Chat(Message),
    /// Delivery acknowledgement for a message id
    Delivered      { msg_id: String },
    FileOffer      { name: String, size: u64, transfer_id: String, from_id: NodeId },
    FileAccept     { transfer_id: String },
    FileReject     { transfer_id: String },
    /// Group management
    GroupAnnounce  { info: GroupInfo },
    GroupJoinReq   { group_name: String, requester_id: NodeId, requester_nick: String },
    GroupJoinApprove { group_name: String, approved_id: NodeId },
    GroupJoinDeny  { group_name: String, denied_id: NodeId },
    Ping,
}

fn subnet_from_addr(addr: &SocketAddr) -> String {
    match addr {
        SocketAddr::V4(v4) => { let o = v4.ip().octets(); format!("{}.{}.x.x", o[0], o[1]) }
        SocketAddr::V6(_)  => "IPv6".into(),
    }
}

pub fn new_id() -> String { uuid::Uuid::new_v4().to_string() }
