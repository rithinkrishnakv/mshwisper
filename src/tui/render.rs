use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, BorderType, Clear, List, ListItem, Paragraph},
};


use crate::core::types::{
    ChannelKind, FileTransfer, Message, MessageContent,
    TransferDirection, TransferStatus,
};
use crate::tui::app::App;
use crate::tui::widgets::{human_size, message_lines};

pub struct RenderSnapshot {
    pub channel_names:   Vec<String>,
    pub channel_unreads: Vec<usize>,
    pub channel_kinds:   Vec<ChannelKind>,
    pub active_idx:      usize,
    pub messages:        Vec<Message>,
    pub peer_nicks:      Vec<String>,
    pub transfers:       Vec<FileTransfer>,
    pub is_gateway:      bool,
    pub self_nick:       String,
    pub self_id:         String,
    pub pending_reqs:    Vec<(String, String, String)>, // (group, id, nick)
}

/// Clickable regions tracked for mouse interaction
#[derive(Clone, Debug)]
pub struct ClickRegion {
    pub rect:   Rect,
    pub action: ClickAction,
}

#[derive(Clone, Debug)]
pub enum ClickAction {
    SelectChannel(usize),
    ScrollUp,
    ScrollDown,
    ClosePopup,
    AcceptFile,
    RejectFile,
    ApproveJoin { group: String, requester_id: String },
    DenyJoin    { group: String, requester_id: String },
    OpenHelp,
}

pub struct Renderer;

impl Renderer {
    pub fn new() -> Self { Self }

    pub fn draw(&self, f: &mut Frame, app: &App, snap: &RenderSnapshot) -> Vec<ClickRegion> {
        let size = f.size();
        let mut regions: Vec<ClickRegion> = Vec::new();

        let root = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(0),
                Constraint::Length(1),
                Constraint::Length(3),
            ])
            .split(size);

        self.draw_titlebar(f, root[0], snap, &mut regions);
        self.draw_body(f, root[1], app, snap, &mut regions);
        self.draw_statusbar(f, root[2], app);
        self.draw_input(f, root[3], app);

        // Overlays (rendered last, highest z-order)
        if !snap.pending_reqs.is_empty() && app.show_join_requests {
            self.draw_join_requests_popup(f, size, snap, &mut regions);
        }
        if let Some((fname, sz, _, from)) = &app.file_offer {
            self.draw_file_popup(f, size, fname, *sz, from, &mut regions);
        }
        if app.show_help { self.draw_help(f, size, &mut regions); }

        regions
    }

    // ── Titlebar ──────────────────────────────────────────────────────────────
    fn draw_titlebar(&self, f: &mut Frame, area: Rect, snap: &RenderSnapshot, regions: &mut Vec<ClickRegion>) {
        let gw = if snap.is_gateway {
            Span::styled(" ◈ GATEWAY ", Style::default().fg(Color::Black).bg(Color::Rgb(255,200,60)).add_modifier(Modifier::BOLD))
        } else { Span::raw("") };

        let pending_badge = if !snap.pending_reqs.is_empty() {
            Span::styled(
                format!(" ⚑ {} join req{} ", snap.pending_reqs.len(), if snap.pending_reqs.len()==1{""} else {"s"}),
                Style::default().fg(Color::Black).bg(Color::Rgb(255,100,100)).add_modifier(Modifier::BOLD),
            )
        } else { Span::raw("") };

        // F1 help clickable region (last ~8 cols)
        let help_rect = Rect::new(area.x + area.width.saturating_sub(8), area.y, 8, 1);
        regions.push(ClickRegion { rect: help_rect, action: ClickAction::OpenHelp });

        let line = Line::from(vec![
            Span::styled(" ◆ mshwisper ", Style::default().fg(Color::Black).bg(Color::Rgb(80,180,255)).add_modifier(Modifier::BOLD)),
            Span::styled(" MESH:ONLINE ", Style::default().fg(Color::Rgb(80,255,160)).add_modifier(Modifier::BOLD)),
            Span::styled(format!(" {} peer{} ", snap.peer_nicks.len(), if snap.peer_nicks.len()==1{""} else {"s"}),
                Style::default().fg(Color::Rgb(120,120,160))),
            gw,
            pending_badge,
            Span::styled(format!("  {} ", snap.self_nick),
                Style::default().fg(Color::Rgb(100,100,140)).add_modifier(Modifier::ITALIC)),
            Span::styled(" [F1:help] ", Style::default().fg(Color::Rgb(60,60,100))),
        ]);
        f.render_widget(Paragraph::new(line).style(Style::default().bg(Color::Rgb(12,12,20))), area);
    }

    // ── Body ──────────────────────────────────────────────────────────────────
    fn draw_body(&self, f: &mut Frame, area: Rect, app: &App, snap: &RenderSnapshot, regions: &mut Vec<ClickRegion>) {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(18), Constraint::Percentage(52), Constraint::Percentage(30)])
            .split(area);
        self.draw_sidebar(f, cols[0], snap, regions);
        self.draw_feed(f, cols[1], app, snap, regions);
        self.draw_arsenal(f, cols[2], snap);
    }

    // ── Sidebar: channels + operators ─────────────────────────────────────────
    fn draw_sidebar(&self, f: &mut Frame, area: Rect, snap: &RenderSnapshot, regions: &mut Vec<ClickRegion>) {
        let halves = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(area);

        // Channels
        let ch_inner_y = halves[0].y + 1; // inside border
        let ch_items: Vec<ListItem> = snap.channel_names.iter().enumerate().map(|(i, name)| {
            let active  = i == snap.active_idx;
            let unread  = snap.channel_unreads.get(i).copied().unwrap_or(0);
            let is_grp  = snap.channel_kinds.get(i).map(|k| matches!(k, ChannelKind::Group{..})).unwrap_or(false);
            let icon    = if is_grp { "⬡ " } else { if active { "▶ " } else { "○ " } };
            let (label, style) = if active {
                (format!(" {}{}", icon, name),
                 Style::default().fg(Color::Black).bg(Color::Rgb(80,180,255)).add_modifier(Modifier::BOLD))
            } else if unread > 0 {
                (format!(" {}{}  ({})", icon, name, unread),
                 Style::default().fg(Color::Rgb(255,200,60)).add_modifier(Modifier::BOLD))
            } else {
                (format!(" {}{}", icon, name), Style::default().fg(Color::Rgb(80,80,110)))
            };
            // Register click region for each channel row
            let row_rect = Rect::new(halves[0].x, ch_inner_y + i as u16, halves[0].width, 1);
            regions.push(ClickRegion { rect: row_rect, action: ClickAction::SelectChannel(i) });
            ListItem::new(label).style(style)
        }).collect();

        f.render_widget(
            List::new(ch_items).block(styled_block(" CHANNELS ")),
            halves[0],
        );

        // Operators
        let mut op_items = vec![ListItem::new(Line::from(vec![
            Span::styled(" ◆ ", Style::default().fg(Color::Rgb(80,180,255))),
            Span::styled(format!("{} (you)", snap.self_nick),
                Style::default().fg(Color::Rgb(80,180,255)).add_modifier(Modifier::ITALIC)),
        ]))];
        if snap.peer_nicks.is_empty() {
            op_items.push(ListItem::new(
                Span::styled("   waiting…", Style::default().fg(Color::Rgb(60,60,90)).add_modifier(Modifier::ITALIC))
            ));
        } else {
            for nick in &snap.peer_nicks {
                op_items.push(ListItem::new(Line::from(vec![
                    Span::styled(" ● ", Style::default().fg(Color::Rgb(80,255,160))),
                    Span::styled(nick.clone(), Style::default().fg(Color::Rgb(200,200,230))),
                ])));
            }
        }
        f.render_widget(List::new(op_items).block(styled_block(" OPERATORS ")), halves[1]);
    }

    // ── Message feed ─────────────────────────────────────────────────────────
    fn draw_feed(&self, f: &mut Frame, area: Rect, app: &App, snap: &RenderSnapshot, regions: &mut Vec<ClickRegion>) {
        let ch_name = snap.channel_names.get(snap.active_idx).cloned().unwrap_or_else(|| "#general".into());
        let inner_h = area.height.saturating_sub(2) as usize;

        let mut all_lines: Vec<Line<'static>> = Vec::new();
        for msg in &snap.messages {
            all_lines.extend(message_lines(msg, &snap.self_id, area.width));
            all_lines.push(Line::from(""));
        }
        if all_lines.last().map(|l: &Line| l.spans.is_empty()).unwrap_or(false) { all_lines.pop(); }

        let total      = all_lines.len();
        let max_offset = total.saturating_sub(inner_h);
        let offset     = app.scroll_offset.min(max_offset);
        let start      = if total > inner_h { (total - inner_h).saturating_sub(offset) } else { 0 };
        let visible: Vec<Line<'static>> = all_lines.into_iter().skip(start).take(inner_h).collect();

        let scroll_hint = if offset > 0 { format!(" ↑ {} ", offset) } else { String::new() };

        // Clickable scroll arrows in feed title area
        let title_y = area.y;
        regions.push(ClickRegion {
            rect: Rect::new(area.x + area.width.saturating_sub(6), title_y, 3, 1),
            action: ClickAction::ScrollUp,
        });
        regions.push(ClickRegion {
            rect: Rect::new(area.x + area.width.saturating_sub(3), title_y, 3, 1),
            action: ClickAction::ScrollDown,
        });

        f.render_widget(
            Paragraph::new(visible).block(
                Block::default()
                    .title(Line::from(vec![
                        Span::styled(format!(" {} ", ch_name),
                            Style::default().fg(Color::Rgb(220,220,255)).add_modifier(Modifier::BOLD)),
                        Span::styled(scroll_hint,
                            Style::default().fg(Color::Rgb(255,200,60))),
                        Span::styled(" [↑][↓] ",
                            Style::default().fg(Color::Rgb(50,50,80))),
                    ]))
                    .borders(Borders::ALL).border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(Color::Rgb(40,40,65)))
                    .style(Style::default().bg(Color::Rgb(8,8,16)))
            ),
            area,
        );
    }

    // ── Arsenal + Transfers ───────────────────────────────────────────────────
    fn draw_arsenal(&self, f: &mut Frame, area: Rect, snap: &RenderSnapshot) {
        let halves = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(area);

        // Commands from feed
        let cmd_items: Vec<ListItem> = {
            let cmds: Vec<&Message> = snap.messages.iter()
                .filter(|m| matches!(&m.content, MessageContent::Command(_)))
                .collect();
            if cmds.is_empty() {
                vec![ListItem::new(Span::styled("  no commands yet",
                    Style::default().fg(Color::Rgb(60,60,90)).add_modifier(Modifier::ITALIC)))]
            } else {
                cmds.iter().rev().take(10).flat_map(|m| {
                    let cmd = match &m.content { MessageContent::Command(c) => c.as_str(), _ => "" };
                    vec![
                        ListItem::new(Line::from(vec![
                            Span::styled(format!("  {} ", m.timestamp.format("%H:%M")),
                                Style::default().fg(Color::Rgb(80,80,110))),
                            Span::styled(m.sender_nick.clone(),
                                Style::default().fg(Color::Rgb(120,160,255))),
                        ])),
                        ListItem::new(Line::from(vec![
                            Span::styled("  $ ", Style::default().fg(Color::Rgb(255,200,60)).add_modifier(Modifier::BOLD)),
                            Span::styled(cmd.to_string(),
                                Style::default().fg(Color::Rgb(220,220,180)).add_modifier(Modifier::BOLD)),
                        ])),
                    ]
                }).collect()
            }
        };
        f.render_widget(
            List::new(cmd_items).block(
                Block::default()
                    .title(Span::styled(" ARSENAL ", Style::default().fg(Color::Rgb(255,200,60)).add_modifier(Modifier::BOLD)))
                    .borders(Borders::ALL).border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(Color::Rgb(40,40,65)))
                    .style(Style::default().bg(Color::Rgb(10,10,18)))
            ),
            halves[0],
        );

        // File transfers
        let xfer_items: Vec<ListItem> = if snap.transfers.is_empty() {
            vec![ListItem::new(Span::styled("  no transfers",
                Style::default().fg(Color::Rgb(60,60,90)).add_modifier(Modifier::ITALIC)))]
        } else {
            snap.transfers.iter().map(|t| {
                let (color, status_str) = match &t.status {
                    TransferStatus::Active    => (Color::Rgb(80,200,255), "active"),
                    TransferStatus::Complete  => (Color::Rgb(80,255,160), "done ✓"),
                    TransferStatus::Failed(_) => (Color::Rgb(255,80,80),  "failed"),
                    TransferStatus::Pending   => (Color::Rgb(255,200,60), "pending"),
                };
                let dir = match t.direction {
                    TransferDirection::Sending   => "▲",
                    TransferDirection::Receiving => "▼",
                };
                let pct    = t.progress_pct();
                let bw     = 10usize;
                let filled = (bw * pct as usize / 100).min(bw);
                let bar    = format!("{}{}", "█".repeat(filled), "░".repeat(bw - filled));
                let fname: String = t.filename.chars().take(14).collect();
                ListItem::new(Line::from(vec![
                    Span::styled(format!("  {} {:<14} ", dir, fname), Style::default().fg(color).add_modifier(Modifier::BOLD)),
                    Span::raw("\n"),
                    Span::styled(format!("    {} {:>3}% {}", bar, pct, status_str), Style::default().fg(color)),
                ]))
            }).collect()
        };
        f.render_widget(
            List::new(xfer_items).block(
                Block::default()
                    .title(Span::styled(" TRANSFERS ", Style::default().fg(Color::Rgb(180,80,255)).add_modifier(Modifier::BOLD)))
                    .borders(Borders::ALL).border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(Color::Rgb(40,40,65)))
                    .style(Style::default().bg(Color::Rgb(10,10,18)))
            ),
            halves[1],
        );
    }

    // ── Status bar ────────────────────────────────────────────────────────────
    fn draw_statusbar(&self, f: &mut Frame, area: Rect, app: &App) {
        let (text, fg) = if let Some(msg) = &app.status_msg {
            let c = if app.status_error { Color::Rgb(255,80,80) } else { Color::Rgb(80,255,160) };
            (format!(" {} ", msg), c)
        } else {
            (" F1:help  PgUp/Dn:channel  ↑↓:scroll  /join /cmd /group /gateway  Ctrl+X:KILL".into(),
             Color::Rgb(60,60,90))
        };
        f.render_widget(Paragraph::new(text).style(Style::default().fg(fg).bg(Color::Rgb(8,8,14))), area);
    }

    // ── Input ─────────────────────────────────────────────────────────────────
    fn draw_input(&self, f: &mut Frame, area: Rect, app: &App) {
        let block = Block::default()
            .title(Span::styled(" INPUT ", Style::default().fg(Color::Rgb(80,180,255))))
            .borders(Borders::ALL).border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Rgb(50,80,130)))
            .style(Style::default().bg(Color::Rgb(8,8,16)));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let line = if app.input_buf.is_empty() {
            Line::from(Span::styled(
                "  Type a message, or /join  /cmd  /group  /gateway  /help  /quit",
                Style::default().fg(Color::Rgb(45,45,70)),
            ))
        } else {
            let pos    = app.cursor_pos;
            let before = &app.input_buf[..pos];
            let after  = &app.input_buf[pos..];
            let (cur, rest) = if let Some(ch) = after.chars().next() {
                (ch.to_string(), &after[ch.len_utf8()..])
            } else { (" ".to_string(), "") };
            let text_style = if app.input_buf.starts_with('/') {
                Style::default().fg(Color::Rgb(255,200,60)).add_modifier(Modifier::BOLD)
            } else { Style::default().fg(Color::Rgb(220,220,255)) };
            let mut spans = vec![];
            if app.history_idx.is_some() {
                let idx = app.history_idx.unwrap();
                spans.push(Span::styled(
                    format!(" [↑{}/{}] ", idx+1, app.history.len()),
                    Style::default().fg(Color::Rgb(255,200,60)),
                ));
            }
            spans.extend_from_slice(&[
                Span::styled(before.to_string(), text_style),
                Span::styled(cur, Style::default().bg(Color::Rgb(80,180,255)).fg(Color::Black)),
                Span::styled(rest.to_string(), text_style),
            ]);
            Line::from(spans)
        };
        f.render_widget(Paragraph::new(line).style(Style::default().fg(Color::Rgb(220,220,255))), inner);
    }

    // ── File offer popup ──────────────────────────────────────────────────────
    fn draw_file_popup(&self, f: &mut Frame, size: Rect, fname: &str, bytes: u64, from: &str,
                       regions: &mut Vec<ClickRegion>) {
        let pw = 56u16.min(size.width);
        let ph = 9u16.min(size.height);
        let popup = center_rect(pw, ph, size);
        f.render_widget(Clear, popup);

        // Register Y / N click regions
        let btn_y = popup.y + 6;
        regions.push(ClickRegion { rect: Rect::new(popup.x+2,  btn_y, 12, 1), action: ClickAction::AcceptFile });
        regions.push(ClickRegion { rect: Rect::new(popup.x+18, btn_y, 12, 1), action: ClickAction::RejectFile });

        let sz = human_size(bytes);
        let lines = vec![
            Line::from(""),
            Line::from(vec![
                Span::raw("  "),
                Span::styled(from, Style::default().fg(Color::Rgb(80,180,255)).add_modifier(Modifier::BOLD)),
                Span::raw(" wants to send:"),
            ]),
            Line::from(vec![
                Span::styled(format!("  {} ", fname),
                    Style::default().fg(Color::Rgb(220,220,255)).add_modifier(Modifier::BOLD)),
                Span::styled(format!("({})", sz), Style::default().fg(Color::Rgb(120,120,160))),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::raw("  "),
                Span::styled(" [Y] Accept ", Style::default().fg(Color::Black).bg(Color::Rgb(80,255,160)).add_modifier(Modifier::BOLD)),
                Span::raw("   "),
                Span::styled(" [N] Reject ", Style::default().fg(Color::Black).bg(Color::Rgb(255,80,80)).add_modifier(Modifier::BOLD)),
            ]),
            Line::from(Span::styled("  (click or press Y / N)", Style::default().fg(Color::Rgb(60,60,90)).add_modifier(Modifier::ITALIC))),
        ];
        f.render_widget(
            Paragraph::new(lines)
                .block(Block::default()
                    .title(Span::styled(" ◆ Incoming File ",
                        Style::default().fg(Color::Rgb(255,200,60)).add_modifier(Modifier::BOLD)))
                    .borders(Borders::ALL).border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(Color::Rgb(255,200,60))))
                .style(Style::default().bg(Color::Rgb(14,14,26))),
            popup,
        );
    }

    // ── Join requests popup (for group owners) ────────────────────────────────
    fn draw_join_requests_popup(&self, f: &mut Frame, size: Rect,
                                 snap: &RenderSnapshot, regions: &mut Vec<ClickRegion>) {
        let pw = 58u16.min(size.width);
        let ph = (4 + snap.pending_reqs.len() as u16 * 2 + 2).min(size.height);
        let popup = center_rect(pw, ph, size);
        f.render_widget(Clear, popup);

        let mut lines = vec![Line::from("")];
        for (i, (group, req_id, nick)) in snap.pending_reqs.iter().enumerate() {
            let row_y = popup.y + 1 + i as u16 * 2;
            // Approve / Deny click regions
            regions.push(ClickRegion {
                rect: Rect::new(popup.x + 2, row_y, 11, 1),
                action: ClickAction::ApproveJoin { group: group.clone(), requester_id: req_id.clone() },
            });
            regions.push(ClickRegion {
                rect: Rect::new(popup.x + 16, row_y, 9, 1),
                action: ClickAction::DenyJoin { group: group.clone(), requester_id: req_id.clone() },
            });
            lines.push(Line::from(vec![
                Span::styled(format!("  {} ", nick),
                    Style::default().fg(Color::Rgb(220,220,255)).add_modifier(Modifier::BOLD)),
                Span::styled(format!("→ {}", group), Style::default().fg(Color::Rgb(120,120,160))),
            ]));
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(" [A] Approve ", Style::default().fg(Color::Black).bg(Color::Rgb(80,255,160)).add_modifier(Modifier::BOLD)),
                Span::raw("  "),
                Span::styled(" [D] Deny ", Style::default().fg(Color::Black).bg(Color::Rgb(255,80,80)).add_modifier(Modifier::BOLD)),
            ]));
        }
        lines.push(Line::from(""));

        f.render_widget(
            Paragraph::new(lines)
                .block(Block::default()
                    .title(Span::styled(" ⚑ Join Requests ",
                        Style::default().fg(Color::Rgb(255,100,100)).add_modifier(Modifier::BOLD)))
                    .borders(Borders::ALL).border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(Color::Rgb(255,100,100))))
                .style(Style::default().bg(Color::Rgb(14,14,26))),
            popup,
        );
    }

    // ── Help popup ────────────────────────────────────────────────────────────
    fn draw_help(&self, f: &mut Frame, size: Rect, regions: &mut Vec<ClickRegion>) {
        let pw = 64u16.min(size.width);
        let ph = 32u16.min(size.height);
        let popup = center_rect(pw, ph, size);
        f.render_widget(Clear, popup);

        // Click anywhere to close
        regions.push(ClickRegion { rect: popup, action: ClickAction::ClosePopup });

        let h  = Style::default().fg(Color::Rgb(80,180,255)).add_modifier(Modifier::BOLD);
        let k  = Style::default().fg(Color::Rgb(255,200,60));
        let d  = Style::default().fg(Color::Rgb(180,180,210));
        let dm = Style::default().fg(Color::Rgb(80,80,110)).add_modifier(Modifier::ITALIC);

        let lines = vec![
            Line::from(""),
            Line::from(Span::styled("  NAVIGATION", h)),
            Line::from(vec![Span::styled("  PgUp / PgDn         ", k), Span::styled("Switch channel (or click channel name)", d)]),
            Line::from(vec![Span::styled("  ↑ / ↓               ", k), Span::styled("Scroll feed  / browse history (when typing)", d)]),
            Line::from(vec![Span::styled("  Ctrl+↑ / Ctrl+↓     ", k), Span::styled("Scroll feed 1 line", d)]),
            Line::from(vec![Span::styled("  Mouse click          ", k), Span::styled("Channel select, accept/deny, scroll", d)]),
            Line::from(vec![Span::styled("  Mouse scroll         ", k), Span::styled("Scroll message feed", d)]),
            Line::from(""),
            Line::from(Span::styled("  EDITING", h)),
            Line::from(vec![Span::styled("  Ctrl+Left/Right     ", k), Span::styled("Move cursor by word", d)]),
            Line::from(vec![Span::styled("  Ctrl+Backspace / W  ", k), Span::styled("Delete previous word", d)]),
            Line::from(vec![Span::styled("  Ctrl+K              ", k), Span::styled("Cut to end of line", d)]),
            Line::from(vec![Span::styled("  Ctrl+U              ", k), Span::styled("Cut to start of line", d)]),
            Line::from(vec![Span::styled("  Ctrl+V              ", k), Span::styled("Paste last cut", d)]),
            Line::from(vec![Span::styled("  Ctrl+A / Home       ", k), Span::styled("Jump to line start", d)]),
            Line::from(vec![Span::styled("  Ctrl+E / End        ", k), Span::styled("Jump to line end", d)]),
            Line::from(""),
            Line::from(Span::styled("  COMMANDS", h)),
            Line::from(vec![Span::styled("  /join <name>         ", k), Span::styled("Join or create an open channel", d)]),
            Line::from(vec![Span::styled("  /group <name>        ", k), Span::styled("Create a private group (invite-only)", d)]),
            Line::from(vec![Span::styled("  /request <grp> <id>  ", k), Span::styled("Request to join a private group", d)]),
            Line::from(vec![Span::styled("  /cmd <shell syntax>  ", k), Span::styled("Broadcast command to all operators", d)]),
            Line::from(vec![Span::styled("  /gateway <ip:port>   ", k), Span::styled("Bridge this node to another subnet", d)]),
            Line::from(vec![Span::styled("  /quit                ", k), Span::styled("Graceful exit (wipes all session data)", d)]),
            Line::from(""),
            Line::from(Span::styled("  MESSAGE TICKS", h)),
            Line::from(vec![Span::styled("  ·   ", k), Span::styled("Sending (no peers yet)", d)]),
            Line::from(vec![Span::styled("  ✓   ", k), Span::styled("Sent to at least one peer", d)]),
            Line::from(vec![Span::styled("  ✓✓  ", k), Span::styled("Delivered (peer acknowledged)", d)]),
            Line::from(""),
            Line::from(Span::styled("  SECURITY", h)),
            Line::from(vec![
                Span::styled("  Ctrl+X  ", Style::default().fg(Color::Rgb(255,80,80)).add_modifier(Modifier::BOLD)),
                Span::styled("KILL SWITCH — wipe keys, blank screen, instant exit", d),
            ]),
            Line::from(Span::styled("  Wire: ECDH Curve25519 + AES-256-GCM  |  RAM-only  |  no logs ever", dm)),
            Line::from(""),
            Line::from(Span::styled("  Click anywhere or press Esc / F1 to close", dm)),
        ];

        f.render_widget(
            Paragraph::new(lines)
                .block(Block::default()
                    .title(Span::styled(" ◆ mshwisper — HELP ", h))
                    .borders(Borders::ALL).border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(Color::Rgb(80,180,255))))
                .style(Style::default().bg(Color::Rgb(10,10,20))),
            popup,
        );
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn styled_block(title: &str) -> Block<'static> {
    Block::default()
        .title(Span::styled(title.to_string(),
            Style::default().fg(Color::Rgb(80,180,255)).add_modifier(Modifier::BOLD)))
        .borders(Borders::ALL).border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Rgb(40,40,65)))
        .style(Style::default().bg(Color::Rgb(10,10,18)))
}

fn center_rect(w: u16, h: u16, area: Rect) -> Rect {
    Rect::new(
        area.x + area.width.saturating_sub(w) / 2,
        area.y + area.height.saturating_sub(h) / 2,
        w.min(area.width),
        h.min(area.height),
    )
}
