use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::core::events::UiCommand;
use crate::tui::app::{App, InputResult};

pub struct InputHandler;

impl InputHandler {
    pub async fn handle(app: &mut App, key: KeyEvent) -> InputResult {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            KeyCode::PageUp   => { let c = app.state.active_channel_index().await; if c>0 { app.state.set_active_channel(c-1).await; app.scroll_offset=0; } }
            KeyCode::PageDown => { let n=app.state.channels_len().await; let c=app.state.active_channel_index().await; if c+1<n { app.state.set_active_channel(c+1).await; app.scroll_offset=0; } }

            KeyCode::Up   if ctrl => { app.scroll_offset = app.scroll_offset.saturating_add(1); }
            KeyCode::Down if ctrl => { if app.scroll_offset>0 { app.scroll_offset-=1; } }
            KeyCode::Up    => { if app.input_buf.is_empty() { app.scroll_offset=app.scroll_offset.saturating_add(3); } else { app.history_prev(); } }
            KeyCode::Down  => { if app.input_buf.is_empty() { if app.scroll_offset>0 { app.scroll_offset=app.scroll_offset.saturating_sub(3); } } else { app.history_next(); } }

            KeyCode::Left  if ctrl => { app.move_word_left(); }
            KeyCode::Right if ctrl => { app.move_word_right(); }
            KeyCode::Left   => { app.move_char_left(); }
            KeyCode::Right  => { app.move_char_right(); }
            KeyCode::Home   => { app.cursor_pos = 0; }
            KeyCode::End    => { app.cursor_pos = app.input_buf.len(); }

            KeyCode::Char('a') if ctrl => { app.cursor_pos = 0; }
            KeyCode::Char('e') if ctrl => { app.cursor_pos = app.input_buf.len(); }
            KeyCode::Char('k') if ctrl => { app.yank_buf = app.input_buf[app.cursor_pos..].to_string(); app.input_buf.truncate(app.cursor_pos); }
            KeyCode::Char('u') if ctrl => { app.yank_buf = app.input_buf[..app.cursor_pos].to_string(); app.input_buf = app.input_buf[app.cursor_pos..].to_string(); app.cursor_pos=0; }
            KeyCode::Char('v') if ctrl => { let y=app.yank_buf.clone(); app.insert_str(&y); }
            KeyCode::Char('y') if ctrl => { let y=app.yank_buf.clone(); app.insert_str(&y); }
            KeyCode::Char('w') if ctrl => { app.delete_word_before(); }
            KeyCode::Backspace if ctrl => { app.delete_word_before(); }

            KeyCode::Char(c) if !ctrl => { app.insert_char(c); }
            KeyCode::Backspace => { app.delete_char_before(); }
            KeyCode::Delete    => { app.delete_char_after(); }

            KeyCode::Enter => {
                let raw = app.input_buf.trim().to_string();
                if raw.is_empty() { return InputResult::Continue; }
                app.history_push(raw.clone());
                app.input_buf.clear(); app.cursor_pos=0; app.scroll_offset=0; app.status_msg=None;
                return Self::dispatch(app, raw).await;
            }

            KeyCode::Esc => {
                app.status_msg=None; app.show_help=false; app.show_join_requests=false;
                if app.history_idx.is_some() { app.input_buf=app.input_draft.clone(); app.cursor_pos=app.input_buf.len(); app.history_idx=None; }
            }
            _ => {}
        }
        InputResult::Continue
    }

    async fn dispatch(app: &mut App, raw: String) -> InputResult {
        if let Some(cmd) = raw.strip_prefix("/cmd ") {
            let ch = active_ch(app).await;
            let _ = app.cmd_tx.send(UiCommand::SendCommand { channel: ch, cmd: cmd.to_string() }).await;

        } else if let Some(rest) = raw.strip_prefix("/join ") {
            let name = rest.trim();
            let ch = if name.starts_with('#') { name.to_string() } else { format!("#{}", name) };
            app.state.add_channel(ch.clone()).await;
            let n = app.state.channels_len().await;
            app.state.set_active_channel(n-1).await;
            app.set_status(format!("Joined {}", ch), false);

        } else if let Some(rest) = raw.strip_prefix("/mshsend ") {
            // /mshsend <peer_nick_or_id> <path>
            let parts: Vec<&str> = rest.trim().splitn(2, ' ').collect();
            if parts.len() == 2 {
                let peer_target = parts[0].trim().to_string();
                let path        = parts[1].trim().to_string();
                // Resolve nick → peer_id
                let peer_id_opt = {
                    let peers = app.state.peers().await;
                    peers.values()
                        .find(|p| p.nickname == peer_target || p.id.starts_with(&peer_target))
                        .map(|p| p.id.clone())
                };
                if let Some(peer_id) = peer_id_opt {
                    let _ = app.cmd_tx.send(UiCommand::SendFile { peer_id, path: path.clone() }).await;
                    app.set_status(format!("Sending {}…", path), false);
                } else {
                    app.set_status(format!("Peer '{}' not found. Use /mshsend <nick> <path>", peer_target), true);
                }
            } else {
                app.set_status("Usage: /mshsend <peer_nick> <file_path>".into(), true);
            }

        } else if let Some(rest) = raw.strip_prefix("/group ") {
            // Create a private group
            let name = rest.trim();
            let gname = if name.starts_with('#') { name.to_string() } else { format!("#{}", name) };
            let _ = app.cmd_tx.send(UiCommand::CreateGroup { name: gname.clone() }).await;
            app.set_status(format!("Created private group {} — announced to mesh", gname), false);

        } else if let Some(rest) = raw.strip_prefix("/request ") {
            // /request #group-name <owner_node_id>
            let parts: Vec<&str> = rest.trim().splitn(2, ' ').collect();
            if parts.len() == 2 {
                let gname    = parts[0].trim();
                let owner_id = parts[1].trim().to_string();
                let gname    = if gname.starts_with('#') { gname.to_string() } else { format!("#{}", gname) };
                let _ = app.cmd_tx.send(UiCommand::RequestJoinGroup { group_name: gname.clone(), owner_id }).await;
                app.set_status(format!("Join request sent for {}", gname), false);
            } else {
                app.set_status("Usage: /request <#group> <owner_node_id>".into(), true);
            }

        } else if let Some(rest) = raw.strip_prefix("/approve ") {
            let parts: Vec<&str> = rest.trim().splitn(2, ' ').collect();
            if parts.len() == 2 {
                let _ = app.cmd_tx.send(UiCommand::ApproveJoin {
                    group_name: parts[0].trim().to_string(),
                    requester_id: parts[1].trim().to_string(),
                }).await;
            } else { app.set_status("Usage: /approve <#group> <requester_id>".into(), true); }

        } else if let Some(rest) = raw.strip_prefix("/deny ") {
            let parts: Vec<&str> = rest.trim().splitn(2, ' ').collect();
            if parts.len() == 2 {
                let _ = app.cmd_tx.send(UiCommand::DenyJoin {
                    group_name: parts[0].trim().to_string(),
                    requester_id: parts[1].trim().to_string(),
                }).await;
            } else { app.set_status("Usage: /deny <#group> <requester_id>".into(), true); }

        } else if let Some(_rest) = raw.strip_prefix("/requests") {
            app.show_join_requests = !app.show_join_requests;

        } else if let Some(rest) = raw.strip_prefix("/gateway ") {
            if let Ok(addr) = rest.trim().parse() {
                let _ = app.cmd_tx.send(UiCommand::EnableGateway { target_addr: addr }).await;
                app.set_status(format!("Connecting gateway to {}…", rest.trim()), false);
            } else { app.set_status(format!("Invalid address '{}' — use ip:port", rest.trim()), true); }

        } else if raw == "/gateway" {
            app.set_status("Usage: /gateway <ip:port>".into(), false);
        } else if raw == "/help" {
            app.show_help = true;
        } else if raw == "/quit" || raw == "/exit" {
            let _ = app.cmd_tx.send(UiCommand::Shutdown).await;
            return InputResult::Quit;
        } else if raw.starts_with('/') {
            app.set_status(format!("Unknown command: {}  (F1 for help)", raw), true);
        } else {
            let ch = active_ch(app).await;
            let _ = app.cmd_tx.send(UiCommand::SendMessage { channel: ch, content: raw }).await;
        }
        InputResult::Continue
    }
}

async fn active_ch(app: &App) -> String {
    let names = app.state.channel_names().await;
    let idx   = app.state.active_channel_index().await;
    names.get(idx).cloned().unwrap_or_else(|| "#general".into())
}
