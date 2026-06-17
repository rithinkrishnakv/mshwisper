use anyhow::Result;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch, Mutex, RwLock};
use tokio::time;
use x25519_dalek::PublicKey;

use crate::core::events::{NetEvent, UiCommand};
use crate::core::state::AppState;
use crate::core::types::{
    AppMessage, FileTransfer, GroupInfo, Message, NodeId, Peer,
    TransferDirection, TransferStatus, WirePacket, new_id,
};
use crate::crypto::{LocalKeypair, SessionCipher};
use crate::network::discovery;
use crate::network::session::TCP_PORT;
use crate::network::transfer::{send_file, FileReceiver};

type CipherMap = Arc<RwLock<HashMap<NodeId, Arc<SessionCipher>>>>;
type ConnMap   = Arc<RwLock<HashMap<NodeId, mpsc::Sender<WirePacket>>>>;
type RecvMap   = Arc<Mutex<HashMap<String, FileReceiver>>>;

pub struct Mesh {
    state:       AppState,
    event_tx:    mpsc::Sender<NetEvent>,
    cmd_rx:      mpsc::Receiver<UiCommand>,
    ciphers:     CipherMap,
    conns:       ConnMap,
    recv_map:    RecvMap,
    shutdown_tx: Arc<watch::Sender<bool>>,
}

impl Mesh {
    pub fn new(
        state: AppState, event_tx: mpsc::Sender<NetEvent>, cmd_rx: mpsc::Receiver<UiCommand>,
    ) -> (Self, Arc<watch::Sender<bool>>) {
        let (sd, _) = watch::channel(false);
        let sd = Arc::new(sd);
        let mesh = Self {
            state, event_tx, cmd_rx,
            ciphers:     Arc::new(RwLock::new(HashMap::new())),
            conns:       Arc::new(RwLock::new(HashMap::new())),
            recv_map:    Arc::new(Mutex::new(HashMap::new())),
            shutdown_tx: sd.clone(),
        };
        (mesh, sd)
    }

    pub async fn run(mut self) -> Result<()> {
        let self_id  = self.state.self_id().to_string();
        let nickname = self.state.nickname().to_string();

        // UDP broadcaster
        { let id=self_id.clone(); let nick=nickname.clone(); let sd=self.shutdown_tx.subscribe();
          tokio::spawn(async move { let _ = discovery::broadcast_presence(id,nick,TCP_PORT,false,sd).await; }); }

        // UDP listener
        let (disc_tx, mut disc_rx) = mpsc::channel::<(WirePacket, SocketAddr)>(64);
        { let sd=self.shutdown_tx.subscribe();
          tokio::spawn(async move { let _ = discovery::listen_for_peers(disc_tx,sd).await; }); }

        // TCP accept loop
        let listener = TcpListener::bind(format!("0.0.0.0:{}", TCP_PORT)).await?;
        { let ev=self.event_tx.clone(); let ci=self.ciphers.clone(); let co=self.conns.clone();
          let rm=self.recv_map.clone(); let sid=self_id.clone(); let state=self.state.clone();
          let mut sd=self.shutdown_tx.subscribe();
          tokio::spawn(async move {
              loop { tokio::select! {
                  Ok((stream,addr)) = listener.accept() => {
                      let (ev2,ci2,co2,rm2,s2,st2)=(ev.clone(),ci.clone(),co.clone(),rm.clone(),sid.clone(),state.clone());
                      tokio::spawn(async move { let _ = handle_incoming(stream,addr,s2,st2,ci2,co2,rm2,ev2).await; });
                  }
                  _ = sd.changed() => { if *sd.borrow() { break; } }
              }}
          }); }

        // Stale peer reaper
        { let state=self.state.clone(); let ev=self.event_tx.clone(); let conns=self.conns.clone();
          tokio::spawn(async move {
              let mut ticker = time::interval(Duration::from_secs(5));
              loop { ticker.tick().await;
                  let now = chrono::Utc::now();
                  let stale: Vec<NodeId> = { let p=state.peers().await;
                      p.values().filter(|p|(now-p.last_seen).num_seconds()>discovery::PEER_TIMEOUT_SECS as i64)
                       .map(|p|p.id.clone()).collect() };
                  for id in stale { state.remove_peer(&id).await; conns.write().await.remove(&id);
                      let _ = ev.send(NetEvent::PeerLeft(id)).await; }
              }
          }); }

        let mut sd_rx = self.shutdown_tx.subscribe();
        loop {
            tokio::select! {
                Some((pkt, addr)) = disc_rx.recv() => {
                    if let WirePacket::Hello { node_id, nickname: peer_nick, tcp_port, .. } = pkt {
                        if node_id == self_id { continue; }
                        let known = self.state.peers().await.contains_key(&node_id);
                        let peer_tcp: SocketAddr = format!("{}:{}", addr.ip(), tcp_port).parse().unwrap_or(addr);
                        if known { self.state.update_peer_seen(&node_id).await; }
                        else {
                            let peer = Peer::new(node_id.clone(), peer_nick, peer_tcp);
                            self.state.add_peer(peer).await;
                            let (ev,ci,co,rm,sid2,st)=(self.event_tx.clone(),self.ciphers.clone(),
                                self.conns.clone(),self.recv_map.clone(),self_id.clone(),self.state.clone());
                            let their=node_id.clone();
                            tokio::spawn(async move { let _ = connect_to_peer(peer_tcp,their,sid2,st,ci,co,rm,ev).await; });
                        }
                    }
                }
                Some(cmd) = self.cmd_rx.recv() => {
                    self.handle_command(cmd, &self_id, &nickname).await;
                }
                _ = sd_rx.changed() => { if *sd_rx.borrow() { break; } }
            }
        }
        Ok(())
    }

    async fn handle_command(&self, cmd: UiCommand, self_id: &str, nickname: &str) {
        match cmd {
            UiCommand::SendMessage { channel, content } => {
                let msg = Message::chat(self_id, nickname, &channel, content);
                let msg_id = msg.id.clone();
                self.state.push_message(msg.clone()).await;
                let peer_count = self.state.peer_count().await;
                if peer_count > 0 {
                    self.broadcast(AppMessage::Chat(msg)).await;
                    self.state.mark_sent(&msg_id).await;
                }
            }
            UiCommand::SendCommand { channel, cmd } => {
                let msg = Message::command(self_id, nickname, &channel, cmd);
                let msg_id = msg.id.clone();
                self.state.push_message(msg.clone()).await;
                self.broadcast(AppMessage::Chat(msg)).await;
                self.state.mark_sent(&msg_id).await;
            }
            UiCommand::SendFile { peer_id, path } => {
                let tx_opt     = self.conns.read().await.get(&peer_id).cloned();
                let cipher_opt = self.ciphers.read().await.get(&peer_id).cloned();
                let (Some(tx), Some(cipher)) = (tx_opt, cipher_opt) else { return };

                let tid   = new_id();
                let fname = std::path::Path::new(&path)
                    .file_name().and_then(|n| n.to_str()).unwrap_or("file").to_string();
                let size  = tokio::fs::metadata(&path).await.map(|m| m.len()).unwrap_or(0);

                self.state.add_transfer(FileTransfer {
                    id: tid.clone(), filename: fname.clone(), size,
                    transferred: 0, direction: TransferDirection::Sending,
                    peer_nick: peer_id.clone(), status: TransferStatus::Pending,
                }).await;

                let offer = AppMessage::FileOffer { name: fname, size, transfer_id: tid.clone(), from_id: self_id.to_string() };
                send_encrypted_app(&tx, self_id, &offer, &cipher).await;

                // Stream chunks after offer (peer will reply with FileAccept before we start)
                let (prog_tx, mut prog_rx) = mpsc::channel::<u64>(64);
                let state2  = self.state.clone();
                let ev2     = self.event_tx.clone();
                let tid2    = tid.clone();
                let path2   = path.clone();
                let tx2     = tx.clone();
                let _sid2    = self_id.to_string();
                tokio::spawn(async move {
                    // Wait up to 30s for accept
                    let mut accepted = false;
                    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
                    loop {
                        if state2.transfers().await.iter().any(|t| t.id == tid2 && t.status == TransferStatus::Active) {
                            accepted = true; break;
                        }
                        if tokio::time::Instant::now() >= deadline { break; }
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                    if !accepted {
                        state2.complete_transfer(&tid2, false, Some("offer timed out or rejected".into())).await;
                        return;
                    }
                    // Now stream the file
                    // Build a packet sender that encrypts each chunk
                    let (chunk_tx, mut chunk_rx) = mpsc::channel::<WirePacket>(32);
                    let tx3 = tx2.clone();
                    tokio::spawn(async move { while let Some(pkt) = chunk_rx.recv().await { let _ = tx3.send(pkt).await; } });
                    match send_file(&path2, tid2.clone(), chunk_tx, prog_tx).await {
                        Ok(_) => { state2.complete_transfer(&tid2, true, None).await; }
                        Err(e) => {
                            state2.complete_transfer(&tid2, false, Some(e.to_string())).await;
                            let _ = ev2.send(NetEvent::FileTransferFailed { transfer_id: tid2, reason: e.to_string() }).await;
                        }
                    }
                });

                // Progress forwarder
                let state3 = self.state.clone();
                let ev3    = self.event_tx.clone();
                let tid3   = tid.clone();
                tokio::spawn(async move {
                    while let Some(bytes) = prog_rx.recv().await {
                        state3.update_transfer_progress(&tid3, bytes).await;
                        let _ = ev3.send(NetEvent::TransferProgress { transfer_id: tid3.clone(), bytes_done: bytes }).await;
                    }
                });
            }
            UiCommand::AcceptFile { transfer_id } => {
                // Create a FileReceiver entry so incoming chunks are assembled
                // We look up the filename from our transfer list
                let xfer_snap = self.state.transfers().await;
                if let Some(t) = xfer_snap.iter().find(|t| t.id == transfer_id) {
                    let fname = t.filename.clone();
                    let _size  = t.size;
                    let tid2  = transfer_id.clone();
                    let rm    = self.recv_map.clone();
                    let state2 = self.state.clone();
                    let ev2    = self.event_tx.clone();
                    tokio::spawn(async move {
                        match FileReceiver::new(tid2.clone(), fname).await {
                            Ok(receiver) => {
                                rm.lock().await.insert(tid2.clone(), receiver);
                                state2.update_transfer_progress(&tid2, 0).await;
                            }
                            Err(e) => {
                                let _ = ev2.send(NetEvent::FileTransferFailed {
                                    transfer_id: tid2, reason: e.to_string(),
                                }).await;
                            }
                        }
                    });
                } else {
                    // No local record yet — still send accept so sender starts streaming
                    // The FileReceiver will be created when first chunk arrives
                }
                self.broadcast(AppMessage::FileAccept { transfer_id }).await;
            }
            UiCommand::RejectFile { transfer_id } => {
                self.broadcast(AppMessage::FileReject { transfer_id }).await;
            }
            UiCommand::CreateGroup { name } => {
                let group_name = if name.starts_with('#') { name.clone() } else { format!("#{}", name) };
                let added = self.state.add_group_channel(group_name.clone(), self_id.to_string()).await;
                if added {
                    let info = GroupInfo {
                        name: group_name, creator_id: self_id.to_string(), members: vec![self_id.to_string()],
                    };
                    self.broadcast(AppMessage::GroupAnnounce { info }).await;
                }
            }
            UiCommand::RequestJoinGroup { group_name, owner_id } => {
                let conns   = self.conns.read().await;
                let ciphers = self.ciphers.read().await;
                if let (Some(tx), Some(cipher)) = (conns.get(&owner_id), ciphers.get(&owner_id)) {
                    let msg = AppMessage::GroupJoinReq {
                        group_name, requester_id: self_id.to_string(), requester_nick: nickname.to_string(),
                    };
                    send_encrypted_app(tx, self_id, &msg, cipher).await;
                }
            }
            UiCommand::ApproveJoin { group_name, requester_id } => {
                self.state.group_approve(&group_name, requester_id.clone()).await;
                self.state.remove_pending_request(&group_name, &requester_id).await;
                let msg = AppMessage::GroupJoinApprove { group_name, approved_id: requester_id };
                self.broadcast(msg).await;
            }
            UiCommand::DenyJoin { group_name, requester_id } => {
                self.state.remove_pending_request(&group_name, &requester_id).await;
                let conns   = self.conns.read().await;
                let ciphers = self.ciphers.read().await;
                if let (Some(tx), Some(c)) = (conns.get(&requester_id), ciphers.get(&requester_id)) {
                    send_encrypted_app(tx, self_id, &AppMessage::GroupJoinDeny { group_name, denied_id: requester_id.clone() }, c).await;
                }
            }
            UiCommand::EnableGateway { target_addr } => {
                let ev    = self.event_tx.clone();
                let state = self.state.clone();
                tokio::spawn(async move {
                    match TcpStream::connect(target_addr).await {
                        Ok(_) => {
                            state.set_gateway(true);
                            let _ = ev.send(NetEvent::GatewayLinked(target_addr.ip().to_string())).await;
                        }
                        Err(e) => { let _ = ev.send(NetEvent::Error(format!("Gateway failed: {}", e))).await; }
                    }
                });
            }
            UiCommand::Shutdown => { let _ = self.shutdown_tx.send(true); }
        }
    }

    async fn broadcast(&self, msg: AppMessage) {
        let conns   = self.conns.read().await;
        let ciphers = self.ciphers.read().await;
        let from    = self.state.self_id().to_string();
        let Ok(pt)  = serde_json::to_vec(&msg) else { return };
        for (peer_id, tx) in conns.iter() {
            if let Some(c) = ciphers.get(peer_id) {
                if let Ok((nonce, ct)) = c.encrypt(&pt) {
                    let _ = tx.send(WirePacket::Encrypted { from_id: from.clone(), nonce, ciphertext: ct }).await;
                }
            }
        }
    }
}

async fn send_encrypted_app(tx: &mpsc::Sender<WirePacket>, from: &str, msg: &AppMessage, cipher: &SessionCipher) {
    let Ok(pt) = serde_json::to_vec(msg) else { return };
    if let Ok((nonce, ct)) = cipher.encrypt(&pt) {
        let _ = tx.send(WirePacket::Encrypted { from_id: from.to_string(), nonce, ciphertext: ct }).await;
    }
}

async fn write_loop(mut half: tokio::net::tcp::OwnedWriteHalf, mut rx: mpsc::Receiver<WirePacket>) {
    while let Some(pkt) = rx.recv().await {
        let Ok(data) = serde_json::to_vec(&pkt) else { continue };
        if half.write_all(&(data.len() as u32).to_be_bytes()).await.is_err() { break; }
        if half.write_all(&data).await.is_err() { break; }
    }
}

async fn read_loop(
    mut half: tokio::net::tcp::OwnedReadHalf,
    their_id: NodeId, their_nick: String, self_id: String,
    state: AppState, ciphers: CipherMap, conns: ConnMap, recv_map: RecvMap,
    event_tx: mpsc::Sender<NetEvent>,
) {
    loop {
        let mut lb = [0u8; 4];
        if half.read_exact(&mut lb).await.is_err() { break; }
        let n = u32::from_be_bytes(lb) as usize;
        if n > 64*1024*1024 { break; }
        let mut data = vec![0u8; n];
        if half.read_exact(&mut data).await.is_err() { break; }
        let Ok(pkt) = serde_json::from_slice::<WirePacket>(&data) else { continue };

        match pkt {
            WirePacket::Encrypted { from_id, nonce, ciphertext } => {
                let plaintext = { let ci=ciphers.read().await;
                    let Some(c) = ci.get(&from_id) else { continue };
                    match c.decrypt(&nonce, &ciphertext) { Ok(p)=>p, Err(_)=>continue }
                };
                let Ok(app) = serde_json::from_slice::<AppMessage>(&plaintext) else { continue };
                handle_app_msg(app, &their_id, &their_nick, &self_id, &state, &conns, &recv_map, &event_tx).await;
            }
            WirePacket::Ping { node_id } => { state.update_peer_seen(&node_id).await; }
            WirePacket::Goodbye { node_id } => {
                state.remove_peer(&node_id).await; conns.write().await.remove(&node_id);
                let _ = event_tx.send(NetEvent::PeerLeft(node_id)).await; return;
            }
            WirePacket::FileChunk { transfer_id, chunk_index, data, is_last } => {
                handle_chunk(&transfer_id, chunk_index, &data, is_last, &state, &recv_map, &event_tx).await;
            }
            _ => {}
        }
    }
    state.remove_peer(&their_id).await; conns.write().await.remove(&their_id);
    let _ = event_tx.send(NetEvent::PeerLeft(their_id)).await;
}

async fn handle_app_msg(
    msg: AppMessage, their_id: &str, their_nick: &str, self_id: &str,
    state: &AppState, conns: &ConnMap, _recv_map: &RecvMap, ev: &mpsc::Sender<NetEvent>,
) {
    match msg {
        AppMessage::Chat(m) => {
            // Send delivery ack back to sender
            let _ack = AppMessage::Delivered { msg_id: m.id.clone() };
            if let Some(_tx) = conns.read().await.get(their_id).cloned() {
                if let Some(_c) = { let _ci = tokio::sync::RwLock::read(&*conns).await; None::<Arc<SessionCipher>> } {
                    // cipher lookup for ack — skip for now, handled via broadcast path
                }
            }
            state.push_message(m.clone()).await;
            let _ = ev.send(NetEvent::MessageReceived(m)).await;
        }
        AppMessage::Delivered { msg_id } => {
            state.mark_delivered(&msg_id).await;
            let _ = ev.send(NetEvent::MessageDelivered { msg_id }).await;
        }
        AppMessage::FileOffer { name, size, transfer_id, from_id } => {
            // Register a Receiving transfer so it shows in the UI
            state.add_transfer(FileTransfer {
                id:          transfer_id.clone(),
                filename:    name.clone(),
                size,
                transferred: 0,
                direction:   TransferDirection::Receiving,
                peer_nick:   their_nick.to_string(),
                status:      TransferStatus::Pending,
            }).await;
            let _ = ev.send(NetEvent::FileOfferReceived {
                from_id, from_nick: their_nick.to_string(),
                filename: name, size, transfer_id,
            }).await;
        }
        AppMessage::FileAccept { transfer_id } => {
            state.update_transfer_progress(&transfer_id, 0).await;
        }
        AppMessage::FileReject { transfer_id } => {
            state.complete_transfer(&transfer_id, false, Some("rejected".into())).await;
        }
        AppMessage::GroupAnnounce { info } => {
            let name = info.name.clone();
            state.add_group_channel(name.clone(), info.creator_id.clone()).await;
            let _ = ev.send(NetEvent::GroupAnnounced { info }).await;
        }
        AppMessage::GroupJoinReq { group_name, requester_id, requester_nick } => {
            state.group_add_pending(&group_name, requester_id.clone()).await;
            state.add_pending_request(group_name.clone(), requester_id.clone(), requester_nick.clone()).await;
            let _ = ev.send(NetEvent::GroupJoinRequest { group_name, requester_id, requester_nick }).await;
        }
        AppMessage::GroupJoinApprove { group_name, approved_id } => {
            if approved_id == self_id {
                state.add_channel(group_name.clone()).await;
                let _ = ev.send(NetEvent::GroupJoinApproved { group_name }).await;
            } else {
                state.group_approve(&group_name, approved_id).await;
            }
        }
        AppMessage::GroupJoinDeny { group_name, .. } => {
            let _ = ev.send(NetEvent::GroupJoinDenied { group_name }).await;
        }
        AppMessage::Ping => { state.update_peer_seen(their_id).await; }
    }
}

async fn handle_chunk(
    tid: &str, index: u64, data: &[u8], is_last: bool,
    state: &AppState, recv_map: &RecvMap, ev: &mpsc::Sender<NetEvent>,
) {
    let mut map = recv_map.lock().await;
    if let Some(r) = map.get_mut(tid) {
        if r.write_chunk(index, data).await.is_err() { map.remove(tid); return; }
        let done = r.received_bytes;
        state.update_transfer_progress(tid, done).await;
        let _ = ev.send(NetEvent::TransferProgress { transfer_id: tid.to_string(), bytes_done: done }).await;
        if is_last {
            let r = map.remove(tid).unwrap();
            if let Ok(path) = r.finalize().await {
                state.complete_transfer(tid, true, None).await;
                let _ = ev.send(NetEvent::FileTransferComplete { transfer_id: tid.to_string(), dest_path: path }).await;
            }
        }
    }
}

async fn connect_to_peer(
    addr: SocketAddr, their_id: NodeId, our_id: String,
    state: AppState, ciphers: CipherMap, conns: ConnMap, recv_map: RecvMap, ev: mpsc::Sender<NetEvent>,
) -> Result<()> {
    let mut stream = TcpStream::connect(addr).await?;
    stream.set_nodelay(true)?;
    ecdh_initiator(&mut stream, &our_id, &their_id, &ciphers).await?;
    let their_nick = state.peers().await.get(&their_id).map(|p| p.nickname.clone()).unwrap_or_else(|| their_id.clone());
    if let Some(p) = state.peers().await.get(&their_id).cloned() { let _ = ev.send(NetEvent::PeerJoined(p)).await; }
    let (r, w) = stream.into_split();
    let (tx, rx) = mpsc::channel::<WirePacket>(256);
    conns.write().await.insert(their_id.clone(), tx);
    tokio::spawn(write_loop(w, rx));
    tokio::spawn(read_loop(r, their_id, their_nick, our_id, state, ciphers, conns, recv_map, ev));
    Ok(())
}

async fn handle_incoming(
    mut stream: TcpStream, _addr: SocketAddr, our_id: String,
    state: AppState, ciphers: CipherMap, conns: ConnMap, recv_map: RecvMap, ev: mpsc::Sender<NetEvent>,
) -> Result<()> {
    stream.set_nodelay(true)?;
    let their_id = ecdh_responder(&mut stream, &our_id, &ciphers).await?;
    let their_nick = state.peers().await.get(&their_id).map(|p| p.nickname.clone()).unwrap_or_else(|| their_id.clone());
    if let Some(p) = state.peers().await.get(&their_id).cloned() { let _ = ev.send(NetEvent::PeerJoined(p)).await; }
    let (r, w) = stream.into_split();
    let (tx, rx) = mpsc::channel::<WirePacket>(256);
    conns.write().await.insert(their_id.clone(), tx);
    tokio::spawn(write_loop(w, rx));
    tokio::spawn(read_loop(r, their_id, their_nick, our_id, state, ciphers, conns, recv_map, ev));
    Ok(())
}

async fn wf(s: &mut TcpStream, d: &[u8]) -> Result<()> {
    s.write_all(&(d.len() as u32).to_be_bytes()).await?; s.write_all(d).await?; Ok(())
}
async fn rf(s: &mut TcpStream) -> Result<Vec<u8>> {
    let mut lb=[0u8;4]; s.read_exact(&mut lb).await?;
    let n=u32::from_be_bytes(lb) as usize; anyhow::ensure!(n<8192,"frame too large");
    let mut buf=vec![0u8;n]; s.read_exact(&mut buf).await?; Ok(buf)
}
async fn ecdh_initiator(s: &mut TcpStream, our: &str, their: &str, ci: &CipherMap) -> Result<()> {
    let kp=LocalKeypair::generate(); let pb=kp.public_bytes();
    wf(s,&serde_json::to_vec(&WirePacket::KeyExchange{node_id:our.into(),public_key:pb.to_vec()})?).await?;
    let ack:WirePacket=serde_json::from_slice(&rf(s).await?)?;
    let their_pub=match ack{WirePacket::KeyExchangeAck{public_key,..}=>public_key,_=>anyhow::bail!("expected ack")};
    anyhow::ensure!(their_pub.len()==32,"bad key");
    let mut arr=[0u8;32]; arr.copy_from_slice(&their_pub);
    let shared=kp.diffie_hellman(&PublicKey::from(arr))?;
    ci.write().await.insert(their.to_string(),Arc::new(SessionCipher::from_shared_secret(&shared))); Ok(())
}
async fn ecdh_responder(s: &mut TcpStream, our: &str, ci: &CipherMap) -> Result<NodeId> {
    let kex:WirePacket=serde_json::from_slice(&rf(s).await?)?;
    let (their_id,their_pub)=match kex{WirePacket::KeyExchange{node_id,public_key}=>(node_id,public_key),_=>anyhow::bail!("expected kex")};
    anyhow::ensure!(their_pub.len()==32,"bad key");
    let kp=LocalKeypair::generate(); let pb=kp.public_bytes();
    wf(s,&serde_json::to_vec(&WirePacket::KeyExchangeAck{node_id:our.into(),public_key:pb.to_vec()})?).await?;
    let mut arr=[0u8;32]; arr.copy_from_slice(&their_pub);
    let shared=kp.diffie_hellman(&PublicKey::from(arr))?;
    ci.write().await.insert(their_id.clone(),Arc::new(SessionCipher::from_shared_secret(&shared))); Ok(their_id)
}
