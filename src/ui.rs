use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::app::{App, PeerRow, ScreenView};

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" LastOrder ")
        .title_bottom(" 私有网络 ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    match app.screen_kind() {
        ScreenView::Join { input, error } => draw_join(frame, inner, input, error),
        ScreenView::Network {
            network_id,
            endpoint_id,
            peers,
            status,
        } => draw_network(frame, inner, network_id, endpoint_id, &peers, status),
    }
}

fn draw_join(frame: &mut Frame, area: Rect, input: &str, error: Option<&str>) {
    let lines = vec![
        Line::from("打开后必须进入一个私有网络。没有公开房间。"),
        Line::from(""),
        Line::from(Span::styled(
            "网络 ID",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!(" {input}_"),
            Style::default().fg(Color::Cyan),
        )),
        Line::from(""),
        Line::from("Enter 加入    Ctrl-G 或 F2 生成    Esc 或 Ctrl-C 退出"),
        Line::from(Span::styled(
            error.unwrap_or("把这个 ID 交给要一起使用的人。"),
            Style::default().fg(if error.is_some() {
                Color::Red
            } else {
                Color::DarkGray
            }),
        )),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn draw_network(
    frame: &mut Frame,
    area: Rect,
    network_id: &str,
    endpoint_id: &str,
    peers: &[PeerRow<'_>],
    status: &str,
) {
    let chunks = Layout::vertical([
        Constraint::Length(4),
        Constraint::Length(2),
        Constraint::Min(3),
        Constraint::Length(2),
    ])
    .split(area);

    let identity = vec![
        Line::from(Span::styled(
            "网络 ID",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(network_id, Style::default().fg(Color::Cyan))),
        Line::from(format!("本机  {endpoint_id}")),
    ];
    frame.render_widget(Paragraph::new(identity), chunks[0]);

    let peer_header = if peers.is_empty() {
        "成员  还没有其他人".to_string()
    } else {
        format!("成员  {}", peers.len())
    };
    frame.render_widget(Paragraph::new(peer_header), chunks[1]);

    let peer_lines: Vec<_> = if peers.is_empty() {
        vec![Line::from("等待同一网络 ID 的节点")]
    } else {
        peers
            .iter()
            .map(|peer| Line::from(format!("{:<12}{}", peer.id, peer.latency)))
            .collect()
    };
    frame.render_widget(
        Paragraph::new(peer_lines).wrap(Wrap { trim: false }),
        chunks[2],
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(status),
            Line::from("Esc 离开网络    Ctrl-C 退出"),
        ]),
        chunks[3],
    );
}
