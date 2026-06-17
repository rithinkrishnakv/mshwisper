//! UDP peer discovery — works on Linux, Windows, and Android/Termux.
//!
//! Android blocks 255.255.255.255 at the OS level, so we compute the
//! subnet-directed broadcast address from the local interface IP instead
//! (e.g. 192.168.1.255 for a /24). This also works on Windows and Linux.
//! Falls back to 255.255.255.255 if we can't determine the subnet.

use anyhow::Result;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;
use tokio::net::UdpSocket as TokioUdp;
use tokio::sync::{mpsc, watch};

use crate::core::types::WirePacket;

pub const DISCOVERY_PORT: u16    = 47800;
pub const DISCOVERY_INTERVAL_MS: u64 = 3000;
pub const PEER_TIMEOUT_SECS: u64     = 15;

/// Broadcast our Hello on the LAN every 3 seconds.
pub async fn broadcast_presence(
    node_id:    String,
    nickname:   String,
    tcp_port:   u16,
    is_gateway: bool,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let sock = UdpSocket::bind("0.0.0.0:0")?;
    sock.set_broadcast(true)?;

    let packet = WirePacket::Hello { node_id, nickname, tcp_port, is_gateway };
    let payload = serde_json::to_vec(&packet)?;

    // Prefer subnet-directed broadcast; fall back to limited broadcast
    let bcast_ip = directed_broadcast().unwrap_or(Ipv4Addr::BROADCAST);
    let target: SocketAddr = SocketAddr::new(IpAddr::V4(bcast_ip), DISCOVERY_PORT);

    let mut interval = tokio::time::interval(Duration::from_millis(DISCOVERY_INTERVAL_MS));

    loop {
        tokio::select! {
            _ = interval.tick() => {
                let _ = sock.send_to(&payload, target);
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() { break; }
            }
        }
    }
    Ok(())
}

/// Listen for Hello broadcasts from peers.
pub async fn listen_for_peers(
    tx: mpsc::Sender<(WirePacket, SocketAddr)>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    // Bind on all interfaces so we catch both loopback (same-machine testing)
    // and LAN broadcast packets.
    let sock = TokioUdp::bind(format!("0.0.0.0:{}", DISCOVERY_PORT)).await?;
    sock.set_broadcast(true)?;

    let mut buf = [0u8; 4096];

    loop {
        tokio::select! {
            result = sock.recv_from(&mut buf) => {
                if let Ok((len, addr)) = result {
                    if let Ok(pkt) = serde_json::from_slice::<WirePacket>(&buf[..len]) {
                        let _ = tx.send((pkt, addr)).await;
                    }
                }
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() { break; }
            }
        }
    }
    Ok(())
}

/// Get the primary outbound local IPv4 address.
/// Uses a connect trick — no packet is actually sent.
pub fn local_ip() -> Option<Ipv4Addr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    // Use a public IP as destination — OS picks the right interface
    sock.connect("8.8.8.8:80").ok()?;
    match sock.local_addr().ok()?.ip() {
        IpAddr::V4(v4) => Some(v4),
        _ => None,
    }
}

/// Compute the subnet-directed broadcast address.
///
/// Android forbids 255.255.255.255 (limited broadcast) but allows directed
/// broadcast to the subnet (e.g. 192.168.1.255). We assume /24 when we
/// can't determine the prefix length, which covers the vast majority of
/// home/lab networks.
pub fn directed_broadcast() -> Option<Ipv4Addr> {
    let local = local_ip()?;
    let o = local.octets();

    // Heuristic subnet detection:
    //   10.x.x.x        → assume /24  → 10.x.x.255
    //   172.16-31.x.x   → assume /24  → 172.y.x.255
    //   192.168.x.x     → assume /24  → 192.168.x.255
    //   169.254.x.x     → assume /16  → 169.254.255.255 (link-local)
    //   anything else   → /24 guess
    let bcast = match o[0] {
        10                        => Ipv4Addr::new(o[0], o[1], o[2], 255),
        172 if o[1] >= 16 && o[1] <= 31 => Ipv4Addr::new(o[0], o[1], o[2], 255),
        192 if o[1] == 168        => Ipv4Addr::new(o[0], o[1], o[2], 255),
        169 if o[1] == 254        => Ipv4Addr::new(169, 254, 255, 255),
        _                         => Ipv4Addr::new(o[0], o[1], o[2], 255),
    };
    Some(bcast)
}
