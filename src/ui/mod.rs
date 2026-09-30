mod join;
mod network;

use ratatui::{
    Frame,
    widgets::{Block, Borders},
};

use crate::app::{App, ScreenView};

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let block = Block::default().borders(Borders::ALL).title(" LastOrder ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    match app.screen_kind() {
        ScreenView::Join { input, error } => join::draw(frame, inner, input, error),
        ScreenView::Network {
            network_id,
            endpoint_id,
            peers,
            status,
        } => network::draw(frame, inner, network_id, endpoint_id, &peers, status),
    }
}
