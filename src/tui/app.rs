use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode,
            KeyModifiers, MouseButton, MouseEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::{io, time::Duration};
use tokio::sync::mpsc;

use crate::core::events::{NetEvent, UiCommand};
use crate::core::state::AppState;

use crate::tui::{
    input::InputHandler,
    render::{ClickAction, ClickRegion, Renderer, RenderSnapshot},
};

const HISTORY_LIMIT: usize = 200;

pub struct App {
    pub state:            AppState,
    pub cmd_tx:           mpsc::Sender<UiCommand>,
    event_rx:             mpsc::Receiver<NetEvent>,

    pub input_buf:        String,
    pub cursor_pos:       usize,
    pub yank_buf:         String,

    pub history:          Vec<String>,
    pub history_idx:      Option<usize>,
    pub input_draft:      String,

    pub scroll_offset:    usize,
    pub status_msg:       Option<String>,
    pub status_error:     bool,

    pub file_offer:       Option<(String, u64, String, String)>,
    pub show_help:        bool,
    pub show_join_requests: bool,

    /// Last frame's clickable regions — updated each draw
    click_regions:        Vec<ClickRegion>,
}

impl App {
    pub fn new(state: AppState, event_rx: mpsc::Receiver<NetEvent>, cmd_tx: mpsc::Sender<UiCommand>) -> Self {
        Self {
            state, cmd_tx, event_rx,
            input_buf: String::new(), cursor_pos: 0, yank_buf: String::new(),
            history: Vec::new(), history_idx: None, input_draft: String::new(),
            scroll_offset: 0, status_msg: None, status_error: false,
            file_offer: None, show_help: false, show_join_requests: false,
            click_regions: Vec::new(),
        }
    }

    pub fn set_status(&mut self, msg: String, err: bool) {
        self.status_msg   = Some(msg);
        self.status_error = err;
    }

    // ── Editing ───────────────────────────────────────────────────────────────
    pub fn insert_char(&mut self, c: char) {
        self.input_buf.insert(self.cursor_pos, c);
        self.cursor_pos += c.len_utf8();
    }
    pub fn insert_str(&mut self, s: &str) {
        self.input_buf.insert_str(self.cursor_pos, s);
        self.cursor_pos += s.len();
    }
    pub fn delete_char_before(&mut self) {
        if self.cursor_pos == 0 { return; }
        let prev = prev_char_boundary(&self.input_buf, self.cursor_pos);
        self.input_buf.remove(prev);
        self.cursor_pos = prev;
    }
    pub fn delete_char_after(&mut self) {
        if self.cursor_pos < self.input_buf.len() { self.input_buf.remove(self.cursor_pos); }
    }
    pub fn delete_word_before(&mut self) {
        if self.cursor_pos == 0 { return; }
        let start = word_start_before(&self.input_buf, self.cursor_pos);
        self.yank_buf = self.input_buf[start..self.cursor_pos].to_string();
        self.input_buf.drain(start..self.cursor_pos);
        self.cursor_pos = start;
    }
    pub fn move_char_left(&mut self) {
        if self.cursor_pos > 0 { self.cursor_pos = prev_char_boundary(&self.input_buf, self.cursor_pos); }
    }
    pub fn move_char_right(&mut self) {
        if self.cursor_pos < self.input_buf.len() {
            let ch = self.input_buf[self.cursor_pos..].chars().next().unwrap();
            self.cursor_pos += ch.len_utf8();
        }
    }
    pub fn move_word_left(&mut self)  { self.cursor_pos = word_start_before(&self.input_buf, self.cursor_pos); }
    pub fn move_word_right(&mut self) { self.cursor_pos = word_end_after(&self.input_buf, self.cursor_pos); }

    // ── History ───────────────────────────────────────────────────────────────
    pub fn history_push(&mut self, line: String) {
        if self.history.last().map(|l| l == &line).unwrap_or(false) { self.history_idx = None; return; }
        self.history.push(line);
        if self.history.len() > HISTORY_LIMIT { self.history.remove(0); }
        self.history_idx = None;
    }
    pub fn history_prev(&mut self) {
        if self.history.is_empty() { return; }
        match self.history_idx {
            None    => { self.input_draft = self.input_buf.clone(); self.history_idx = Some(self.history.len()-1); }
            Some(0) => {}
            Some(i) => { self.history_idx = Some(i-1); }
        }
        if let Some(i) = self.history_idx { self.input_buf = self.history[i].clone(); self.cursor_pos = self.input_buf.len(); }
    }
    pub fn history_next(&mut self) {
        match self.history_idx {
            None => {}
            Some(i) if i+1 >= self.history.len() => {
                self.history_idx = None; self.input_buf = self.input_draft.clone(); self.cursor_pos = self.input_buf.len();
            }
            Some(i) => { self.history_idx = Some(i+1); self.input_buf = self.history[i+1].clone(); self.cursor_pos = self.input_buf.len(); }
        }
    }

    // ── Main run loop ─────────────────────────────────────────────────────────
    pub async fn run(&mut self) -> Result<()> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        let tick = Duration::from_millis(16);
        let mut last = std::time::Instant::now();

        loop {
            // Snapshot state
            let snap = self.build_snapshot().await;

            // Draw + capture click regions
            let regions = {
                let app_ref = &*self;
                let mut captured: Vec<ClickRegion> = Vec::new();
                terminal.draw(|f| {
                    captured = Renderer::new().draw(f, app_ref, &snap);
                })?;
                captured
            };
            self.click_regions = regions;

            // Poll input
            let timeout = tick.checked_sub(last.elapsed()).unwrap_or(Duration::ZERO);
            if event::poll(timeout)? {
                match event::read()? {
                    Event::Key(key) => {
                        // Kill switch
                        if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('x') {
                            self.kill_switch(&mut terminal).await?;
                            return Ok(());
                        }
                        // F1
                        if key.code == KeyCode::F(1) { self.show_help = !self.show_help; continue; }
                        if self.show_help {
                            if matches!(key.code, KeyCode::Esc | KeyCode::F(1)) { self.show_help = false; }
                            continue;
                        }
                        // Join requests popup: A / D
                        if self.show_join_requests {
                            if matches!(key.code, KeyCode::Esc) { self.show_join_requests = false; continue; }
                        }
                        // File offer Y/N
                        if let Some((fname, _sz, tid, _nick)) = self.file_offer.clone() {
                            match key.code {
                                KeyCode::Char('y') | KeyCode::Char('Y') => {
                                    let _ = self.cmd_tx.send(UiCommand::AcceptFile { transfer_id: tid }).await;
                                    self.file_offer = None;
                                    self.set_status(format!("Downloading {}…", fname), false);
                                }
                                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                                    let _ = self.cmd_tx.send(UiCommand::RejectFile { transfer_id: tid }).await;
                                    self.file_offer = None;
                                    self.set_status("Transfer rejected.".into(), false);
                                }
                                _ => {}
                            }
                            continue;
                        }
                        if InputHandler::handle(self, key).await == InputResult::Quit { break; }
                    }
                    Event::Mouse(mouse) => {
                        self.handle_mouse(mouse).await;
                    }
                    _ => {}
                }
            }

            // Drain network events
            loop {
                match self.event_rx.try_recv() {
                    Ok(ev) => self.handle_net_event(ev).await,
                    Err(_) => break,
                }
            }

            if last.elapsed() >= tick { last = std::time::Instant::now(); }
        }

        self.graceful_shutdown(&mut terminal).await?;
        Ok(())
    }

    // ── Mouse handler ─────────────────────────────────────────────────────────
    async fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let col = mouse.column;
                let row = mouse.row;
                // Find which region was clicked
                let action = self.click_regions.iter().find(|r| {
                    col >= r.rect.x && col < r.rect.x + r.rect.width &&
                    row >= r.rect.y && row < r.rect.y + r.rect.height
                }).map(|r| r.action.clone());

                if let Some(action) = action {
                    self.dispatch_click(action).await;
                }
            }
            MouseEventKind::ScrollUp => {
                self.scroll_offset = self.scroll_offset.saturating_add(3);
            }
            MouseEventKind::ScrollDown => {
                if self.scroll_offset > 0 { self.scroll_offset = self.scroll_offset.saturating_sub(3); }
            }
            _ => {}
        }
    }

    async fn dispatch_click(&mut self, action: ClickAction) {
        match action {
            ClickAction::SelectChannel(idx) => {
                self.state.set_active_channel(idx).await;
                self.scroll_offset = 0;
            }
            ClickAction::ScrollUp => {
                self.scroll_offset = self.scroll_offset.saturating_add(3);
            }
            ClickAction::ScrollDown => {
                if self.scroll_offset > 0 { self.scroll_offset = self.scroll_offset.saturating_sub(3); }
            }
            ClickAction::ClosePopup => {
                self.show_help = false;
                self.show_join_requests = false;
            }
            ClickAction::AcceptFile => {
                if let Some((fname, _, tid, _)) = self.file_offer.clone() {
                    let _ = self.cmd_tx.send(UiCommand::AcceptFile { transfer_id: tid }).await;
                    self.file_offer = None;
                    self.set_status(format!("Downloading {}…", fname), false);
                }
            }
            ClickAction::RejectFile => {
                if let Some((_, _, tid, _)) = self.file_offer.clone() {
                    let _ = self.cmd_tx.send(UiCommand::RejectFile { transfer_id: tid }).await;
                    self.file_offer = None;
                    self.set_status("Transfer rejected.".into(), false);
                }
            }
            ClickAction::ApproveJoin { group, requester_id } => {
                let _ = self.cmd_tx.send(UiCommand::ApproveJoin {
                    group_name: group, requester_id,
                }).await;
            }
            ClickAction::DenyJoin { group, requester_id } => {
                let _ = self.cmd_tx.send(UiCommand::DenyJoin {
                    group_name: group, requester_id,
                }).await;
            }
            ClickAction::OpenHelp => { self.show_help = true; }
        }
    }

    // ── Net event handler ─────────────────────────────────────────────────────
    async fn handle_net_event(&mut self, ev: NetEvent) {
        match ev {
            NetEvent::PeerJoined(peer) => {
                self.state.push_message(crate::core::types::Message::system(
                    format!("[+] {} joined  [{}]", peer.nickname, peer.addr)
                )).await;
                self.scroll_offset = 0;
            }
            NetEvent::PeerLeft(id) => {
                let nick = { let p = self.state.peers().await; p.get(&id).map(|x| x.nickname.clone()).unwrap_or_else(|| id.clone()) };
                self.state.push_message(crate::core::types::Message::system(format!("[-] {} left", nick))).await;
            }
            NetEvent::MessageReceived(_)             => { self.scroll_offset = 0; }
            NetEvent::MessageDelivered { msg_id }    => { self.state.mark_delivered(&msg_id).await; }
            NetEvent::FileOfferReceived { from_nick, filename, size, transfer_id, .. } => {
                self.file_offer = Some((filename, size, transfer_id, from_nick));
            }
            NetEvent::TransferProgress { transfer_id, bytes_done } => {
                self.state.update_transfer_progress(&transfer_id, bytes_done).await;
            }
            NetEvent::FileTransferComplete { transfer_id, dest_path } => {
                self.state.complete_transfer(&transfer_id, true, None).await;
                self.set_status(format!("Download done → {}", dest_path), false);
            }
            NetEvent::FileTransferFailed { transfer_id, reason } => {
                self.state.complete_transfer(&transfer_id, false, Some(reason.clone())).await;
                self.set_status(format!("Transfer failed: {}", reason), true);
            }
            NetEvent::GroupAnnounced { info } => {
                self.set_status(format!("Group {} exists — /request {} <owner_id> to join", info.name, info.name), false);
            }
            NetEvent::GroupJoinRequest { group_name, requester_id, requester_nick } => {
                self.state.add_pending_request(group_name, requester_id, requester_nick).await;
                self.show_join_requests = true;
            }
            NetEvent::GroupJoinApproved { group_name } => {
                self.set_status(format!("You were approved to join {}", group_name), false);
                self.state.push_message(crate::core::types::Message::system(
                    format!("You joined {}", group_name)
                )).await;
            }
            NetEvent::GroupJoinDenied { group_name } => {
                self.set_status(format!("Join request for {} was denied", group_name), true);
            }
            NetEvent::GatewayLinked(sub) => {
                self.set_status(format!("Gateway linked: {}", sub), false);
                self.state.add_channel(format!("#{}-bridge", sub)).await;
                self.state.push_message(crate::core::types::Message::system(
                    format!("Gateway bridged to subnet {}", sub)
                )).await;
            }
            NetEvent::Error(e) => { self.set_status(e, true); }
        }
    }

    async fn build_snapshot(&self) -> RenderSnapshot {
        let channel_names   = self.state.channel_names().await;
        let channel_unreads = self.state.channel_unreads().await;
        let channel_kinds   = self.state.channel_kinds().await;
        let active_idx      = self.state.active_channel_index().await;
        let messages        = self.state.active_messages().await;
        let peer_nicks      = { let p = self.state.peers().await; p.values().map(|p| p.nickname.clone()).collect() };
        let transfers       = self.state.transfers().await;
        let is_gateway      = self.state.is_gateway();
        let self_nick       = self.state.nickname().to_string();
        let self_id         = self.state.self_id().to_string();
        let pending_reqs    = self.state.pending_requests().await;
        RenderSnapshot { channel_names, channel_unreads, channel_kinds, active_idx,
                         messages, peer_nicks, transfers, is_gateway, self_nick, self_id, pending_reqs }
    }

    async fn graceful_shutdown(&mut self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
        let _ = self.cmd_tx.send(UiCommand::Shutdown).await;
        self.state.wipe().await;
        unsafe { for b in self.input_buf.as_bytes_mut() { *b = 0; } } self.input_buf.clear();
        unsafe { for b in self.yank_buf.as_bytes_mut()  { *b = 0; } } self.yank_buf.clear();
        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
        terminal.show_cursor()?;
        println!("\r\nmshwisper: session wiped. Goodbye.\r\n");
        Ok(())
    }

    async fn kill_switch(&mut self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
        let _ = self.cmd_tx.send(UiCommand::Shutdown).await;
        self.state.wipe().await;
        unsafe { for b in self.input_buf.as_bytes_mut() { *b = 0; } } self.input_buf.clear();
        unsafe { for b in self.yank_buf.as_bytes_mut()  { *b = 0; } } self.yank_buf.clear();
        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
        terminal.show_cursor()?;
        print!("\x1b[2J\x1b[H");
        std::io::Write::flush(&mut io::stdout())?;
        Ok(())
    }
}

#[derive(PartialEq)]
pub enum InputResult { Continue, Quit }

fn prev_char_boundary(s: &str, pos: usize) -> usize {
    let mut p = pos.saturating_sub(1);
    while p > 0 && !s.is_char_boundary(p) { p -= 1; }
    p
}
fn word_start_before(s: &str, pos: usize) -> usize {
    let b = &s.as_bytes()[..pos]; let mut i = pos;
    while i > 0 && b[i-1] == b' ' { i -= 1; }
    while i > 0 && b[i-1] != b' ' { i -= 1; }
    i
}
fn word_end_after(s: &str, pos: usize) -> usize {
    let b = s.as_bytes(); let mut i = pos;
    while i < b.len() && b[i] == b' ' { i += 1; }
    while i < b.len() && b[i] != b' ' { i += 1; }
    i
}
