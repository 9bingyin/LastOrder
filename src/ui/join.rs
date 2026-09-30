use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};

pub(super) fn draw(frame: &mut Frame, area: Rect, input: &str, error: Option<&str>) {
    let lines = vec![
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
