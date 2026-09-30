use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};

use crate::{app::PeerRow, net::Link};

pub(super) fn draw(
    frame: &mut Frame,
    area: Rect,
    network_id: &str,
    endpoint_id: &str,
    peers: &[PeerRow<'_>],
    status: &str,
) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(if status.is_empty() { 1 } else { 2 }),
    ])
    .split(area);

    let identity = vec![
        Line::from(Span::styled(
            "网络 ID",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(network_id, Style::default().fg(Color::Cyan))),
        Line::from(if endpoint_id.is_empty() {
            String::new()
        } else {
            format!("本机  {endpoint_id}")
        }),
    ];
    frame.render_widget(Paragraph::new(identity), chunks[0]);
    frame.render_widget(Paragraph::new(format!("成员  {}", peers.len())), chunks[1]);

    let peer_lines: Vec<_> = peers
        .iter()
        .map(|peer| {
            Line::from(vec![
                Span::styled(format!("{:<12}", peer.id), link_style(peer.link)),
                Span::raw(peer.latency),
            ])
        })
        .collect();
    frame.render_widget(
        Paragraph::new(peer_lines).wrap(Wrap { trim: false }),
        chunks[2],
    );
    let mut footer = Vec::new();
    if !status.is_empty() {
        footer.push(Line::from(status));
    }
    footer.push(Line::from("Esc 离开网络    Ctrl-C 退出"));
    frame.render_widget(Paragraph::new(footer), chunks[3]);
}

fn link_style(link: Link) -> Style {
    match link {
        Link::Unknown => Style::default(),
        Link::Direct => Style::default().fg(Color::Green),
        Link::Relay => Style::default().fg(Color::Yellow),
        Link::Down => Style::default().fg(Color::Red),
    }
}
