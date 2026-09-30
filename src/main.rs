mod app;
mod net;
mod ui;

use anyhow::{Context, Result};

#[tokio::main]
async fn main() -> Result<()> {
    let mut terminal = ratatui::try_init().context("failed to start terminal")?;
    let result = app::run(&mut terminal).await;
    ratatui::restore();
    result
}
