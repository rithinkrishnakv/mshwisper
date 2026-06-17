#![allow(dead_code)]
#![allow(unused_variables)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::borrow_deref_ref)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::while_let_loop)]
#![allow(clippy::collapsible_match)]
#![allow(clippy::unnecessary_unwrap)]

mod core;
mod crypto;
mod network;
mod tui;

use anyhow::Result;
use std::io::{self, Write};
use tokio::sync::mpsc;
use uuid::Uuid;

use core::{events::{NetEvent, UiCommand}, state::AppState};
use network::mesh::Mesh;
use tui::app::App;

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    print!("\x1b[2J\x1b[H");

    println!("\x1b[38;2;80;180;255m");
    println!("  mshwisper");
    println!("  Zero-Config Encrypted LAN Mesh");
    println!("\x1b[0m");
    println!("  \x1b[38;2;80;80;120mAll traffic: ECDH Curve25519 + AES-256-GCM");
    println!("  RAM-only footprint. Ctrl+X to instant-kill.");
    println!("  Works on Linux (Kali), Windows CMD, and Termux.\x1b[0m");
    println!();

    // Detect environment for user hints
    let env_hint = detect_env();
    if !env_hint.is_empty() {
        println!("  \x1b[38;2;255;200;60m{}\x1b[0m", env_hint);
        println!();
    }

    let nickname = loop {
        print!("  \x1b[38;2;80;180;255mNickname:\x1b[0m ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let nick = input.trim().to_string();
        if nick.is_empty() {
            println!("  \x1b[31mNickname cannot be empty.\x1b[0m");
            continue;
        }
        if nick.len() > 24 {
            println!("  \x1b[31mMax 24 chars.\x1b[0m");
            continue;
        }
        if nick.contains(|c: char| !c.is_alphanumeric() && c != '_' && c != '-') {
            println!("  \x1b[31mAlphanumeric, _ and - only.\x1b[0m");
            continue;
        }
        break nick;
    };

    let node_id = Uuid::new_v4().to_string();
    let local_ip = network::discovery::local_ip()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| "unknown".into());

    println!();
    println!("  \x1b[38;2;80;255;160m[+]\x1b[0m Nick    : \x1b[38;2;80;180;255m{}\x1b[0m", nickname);
    println!("  \x1b[38;2;80;255;160m[+]\x1b[0m Node ID : \x1b[38;2;80;80;120m{}\x1b[0m", &node_id[..8]);
    println!("  \x1b[38;2;80;255;160m[+]\x1b[0m Local IP: \x1b[38;2;80;80;120m{}\x1b[0m", local_ip);
    println!("  \x1b[38;2;80;255;160m[+]\x1b[0m Discovery: UDP :{}", network::discovery::DISCOVERY_PORT);
    println!("  \x1b[38;2;80;255;160m[+]\x1b[0m Sessions : TCP :{}", network::session::TCP_PORT);
    println!();
    println!("  \x1b[38;2;60;60;90mPress Enter to start...\x1b[0m");
    let mut dummy = String::new();
    io::stdin().read_line(&mut dummy)?;

    let (net_event_tx, net_event_rx) = mpsc::channel::<NetEvent>(512);
    let (ui_cmd_tx,   ui_cmd_rx)     = mpsc::channel::<UiCommand>(256);

    let state = AppState::new(node_id.clone(), nickname.clone());

    {
        let mesh_state = state.clone();
        let (mesh, _sd) = Mesh::new(mesh_state, net_event_tx, ui_cmd_rx);
        tokio::spawn(async move {
            if let Err(e) = mesh.run().await {
                eprintln!("Mesh error: {e}");
            }
        });
    }

    let mut app = App::new(state, net_event_rx, ui_cmd_tx);
    app.run().await?;
    Ok(())
}

fn detect_env() -> &'static str {
    // Termux sets TERMUX_VERSION or PREFIX=/data/data/com.termux/files/usr
    if std::env::var("TERMUX_VERSION").is_ok()
        || std::env::var("PREFIX").map(|p| p.contains("termux")).unwrap_or(false)
    {
        return "Termux detected. Ensure you have: pkg install rust openssl";
    }
    // Windows
    #[cfg(target_os = "windows")]
    { return "Windows detected. Run in Windows Terminal for best color support."; }
    ""
}
