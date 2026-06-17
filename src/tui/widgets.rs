use ratatui::{style::{Color, Modifier, Style}, text::{Line, Span}};
use crate::core::types::{Message, MessageContent, MessageStatus};

/// Render a Message into display Lines with WhatsApp-style tick marks
pub fn message_lines(msg: &Message, self_id: &str, _width: u16) -> Vec<Line<'static>> {
    let ts = msg.timestamp.format("%H:%M").to_string();
    let is_own = msg.sender_id == self_id;

    // Tick mark for own messages
    let tick = if is_own {
        match msg.status {
            MessageStatus::Sending   => Span::styled(" ·", Style::default().fg(Color::Rgb(100,100,130))),
            MessageStatus::Sent      => Span::styled(" ✓", Style::default().fg(Color::Rgb(140,140,170))),
            MessageStatus::Delivered => Span::styled(" ✓✓", Style::default().fg(Color::Rgb(80,200,255))),
            MessageStatus::Read      => Span::styled(" ✓✓", Style::default().fg(Color::Rgb(80,255,160))),
            MessageStatus::System    => Span::raw(""),
        }
    } else {
        Span::raw("")
    };

    match &msg.content {
        MessageContent::System(text) => vec![
            Line::from(vec![
                Span::styled(format!("  {} ", ts), Style::default().fg(Color::Rgb(60,60,90))),
                Span::styled(format!("◈ {}", text),
                    Style::default().fg(Color::Rgb(80,80,120)).add_modifier(Modifier::ITALIC)),
            ])
        ],

        MessageContent::Chat(text) => {
            let nc = if is_own { Color::Rgb(80,200,255) } else { nick_color(&msg.sender_nick) };
            vec![Line::from(vec![
                Span::styled(format!("  {} ", ts), Style::default().fg(Color::Rgb(60,60,90))),
                Span::styled(msg.sender_nick.clone(),
                    Style::default().fg(nc).add_modifier(Modifier::BOLD)),
                Span::raw("  "),
                Span::styled(text.clone(), Style::default().fg(Color::Rgb(210,210,235))),
                tick,
            ])]
        }

        MessageContent::Command(cmd) => vec![
            Line::from(vec![
                Span::styled("  ╭─ cmd ", Style::default().fg(Color::Rgb(255,200,60)).add_modifier(Modifier::BOLD)),
                Span::styled(format!("[{}]  ", msg.sender_nick), Style::default().fg(Color::Rgb(120,120,160))),
                Span::styled(ts.clone(), Style::default().fg(Color::Rgb(60,60,90))),
                tick,
            ]),
            Line::from(vec![
                Span::styled("  │  $ ", Style::default().fg(Color::Rgb(255,200,60))),
                Span::styled(cmd.clone(),
                    Style::default().fg(Color::Rgb(220,220,160)).add_modifier(Modifier::BOLD)),
            ]),
            Line::from(Span::styled("  ╰─────", Style::default().fg(Color::Rgb(255,200,60)))),
        ],

        MessageContent::FileOffer { name, size, .. } => vec![
            Line::from(vec![
                Span::styled(format!("  {} ", ts), Style::default().fg(Color::Rgb(60,60,90))),
                Span::styled(msg.sender_nick.clone(),
                    Style::default().fg(Color::Rgb(180,80,255)).add_modifier(Modifier::BOLD)),
                Span::styled(format!(" ◈ file offer: {} ({})", name, human_size(*size)),
                    Style::default().fg(Color::Rgb(180,80,255))),
                tick,
            ])
        ],
    }
}

fn nick_color(nick: &str) -> Color {
    let h: u32 = nick.bytes().fold(0u32, |a,b| a.wrapping_mul(31).wrapping_add(b as u32));
    const P: &[Color] = &[
        Color::Rgb(80,200,255), Color::Rgb(80,255,160), Color::Rgb(255,160,80),
        Color::Rgb(200,80,255), Color::Rgb(255,220,80), Color::Rgb(80,255,220),
        Color::Rgb(255,100,160), Color::Rgb(100,255,100),
    ];
    P[(h as usize) % P.len()]
}

pub fn human_size(bytes: u64) -> String {
    if bytes >= 1<<30 { format!("{:.1} GB", bytes as f64/(1<<30) as f64) }
    else if bytes >= 1<<20 { format!("{:.1} MB", bytes as f64/(1<<20) as f64) }
    else if bytes >= 1<<10 { format!("{:.0} KB", bytes as f64/(1<<10) as f64) }
    else { format!("{} B", bytes) }
}
