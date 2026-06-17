use std::net::SocketAddr;
use super::types::{GroupInfo, Message, NodeId};

#[derive(Clone, Debug)]
pub enum NetEvent {
    PeerJoined(crate::core::types::Peer),
    PeerLeft(NodeId),
    MessageReceived(Message),
    MessageDelivered { msg_id: String },
    FileOfferReceived {
        from_id: NodeId, from_nick: String,
        filename: String, size: u64, transfer_id: String,
    },
    TransferProgress     { transfer_id: String, bytes_done: u64 },
    FileTransferComplete { transfer_id: String, dest_path: String },
    FileTransferFailed   { transfer_id: String, reason: String },
    /// A private group was announced on the mesh
    GroupAnnounced       { info: GroupInfo },
    /// Someone is requesting to join a group we own
    GroupJoinRequest     { group_name: String, requester_id: NodeId, requester_nick: String },
    /// Our join request was approved
    GroupJoinApproved    { group_name: String },
    /// Our join request was denied
    GroupJoinDenied      { group_name: String },
    GatewayLinked(String),
    Error(String),
}

#[derive(Clone, Debug)]
pub enum UiCommand {
    SendMessage      { channel: String, content: String },
    SendCommand      { channel: String, cmd: String },
    SendFile         { peer_id: NodeId, path: String },
    AcceptFile       { transfer_id: String },
    RejectFile       { transfer_id: String },
    /// Create a new private group and announce it
    CreateGroup      { name: String },
    /// Request to join a private group owned by someone else
    RequestJoinGroup { group_name: String, owner_id: NodeId },
    /// Approve a pending join request (group owner only)
    ApproveJoin      { group_name: String, requester_id: NodeId },
    /// Deny a pending join request
    DenyJoin         { group_name: String, requester_id: NodeId },
    EnableGateway    { target_addr: SocketAddr },
    Shutdown,
}
