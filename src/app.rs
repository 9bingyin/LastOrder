use std::{collections::BTreeMap, time::Duration};

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use ratatui::DefaultTerminal;
use tokio::sync::mpsc;

use crate::{
    net::{self, NetEvent, Session},
    ui,
};

const NETWORK_ID_LIMIT: usize = 128;

enum Screen {
    Join {
        input: String,
        error: Option<String>,
    },
    Network(NetworkView),
}

struct NetworkView {
    network_id: String,
    endpoint_id: String,
    peers: BTreeMap<String, String>,
    status: String,
}

enum Command {
    None,
    Quit,
    Leave,
}

pub struct App {
    screen: Screen,
    session: Option<Session>,
    events: Option<mpsc::UnboundedReceiver<NetEvent>>,
}

pub async fn run(terminal: &mut DefaultTerminal) -> Result<()> {
    let mut app = App {
        screen: Screen::Join {
            input: String::new(),
            error: None,
        },
        session: None,
        events: None,
    };
    let mut input = EventStream::new();
    let result = loop {
        if let Err(error) = terminal.draw(|frame| ui::draw(frame, &app)) {
            break Err(error.into());
        }
        tokio::select! {
            biased;
            event = input.next() => {
                let Some(event) = event else {
                    break Ok(());
                };
                match app.on_event(event?)? {
                    Command::Quit => break Ok(()),
                    Command::Leave => app.leave().await,
                    Command::None => {}
                }
            }
            event = recv_net(&mut app.events) => {
                if let Some(event) = event {
                    app.on_net(event);
                }
            }
        }
    };
    if let Some(session) = app.session.take() {
        session.shutdown().await;
    }
    result
}

async fn recv_net(events: &mut Option<mpsc::UnboundedReceiver<NetEvent>>) -> Option<NetEvent> {
    match events {
        Some(events) => events.recv().await,
        None => std::future::pending().await,
    }
}

impl App {
    fn on_event(&mut self, event: Event) -> Result<Command> {
        let Event::Key(key) = event else {
            return Ok(Command::None);
        };
        if key.kind == KeyEventKind::Release {
            return Ok(Command::None);
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Ok(Command::Quit);
        }
        match key.code {
            KeyCode::Esc if self.is_network() => Ok(Command::Leave),
            KeyCode::Esc => Ok(Command::Quit),
            _ if self.is_network() => Ok(Command::None),
            KeyCode::Enter => self.join_from_input(),
            KeyCode::Char('g') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.generate();
                Ok(Command::None)
            }
            KeyCode::F(2) => {
                self.generate();
                Ok(Command::None)
            }
            KeyCode::Backspace => {
                self.edit_input(|input| {
                    input.pop();
                });
                Ok(Command::None)
            }
            KeyCode::Char(character)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.edit_input(|input| {
                    if input.chars().count() < NETWORK_ID_LIMIT {
                        input.push(character);
                    }
                });
                Ok(Command::None)
            }
            _ => Ok(Command::None),
        }
    }

    fn is_network(&self) -> bool {
        matches!(self.screen, Screen::Network(_))
    }

    fn join_from_input(&mut self) -> Result<Command> {
        let network_id = {
            let Screen::Join { input, error } = &mut self.screen else {
                return Ok(Command::None);
            };
            let network_id = input.trim().to_string();
            if network_id.is_empty() {
                *error = Some("请输入或生成网络 ID".to_string());
                return Ok(Command::None);
            }
            network_id
        };
        self.join(network_id);
        Ok(Command::None)
    }

    fn generate(&mut self) {
        let Screen::Join { input, error } = &mut self.screen else {
            return;
        };
        match net::generate_network_id() {
            Ok(network_id) => {
                *input = network_id;
                *error = None;
            }
            Err(cause) => *error = Some(cause.to_string()),
        }
    }

    fn join(&mut self, network_id: String) {
        let (sender, receiver) = mpsc::unbounded_channel();
        self.session = Some(Session::spawn(network_id.clone(), sender));
        self.events = Some(receiver);
        self.screen = Screen::Network(NetworkView {
            network_id,
            endpoint_id: "上线中".to_string(),
            peers: BTreeMap::new(),
            status: "正在上线".to_string(),
        });
    }

    async fn leave(&mut self) {
        let network_id = match &self.screen {
            Screen::Network(view) => view.network_id.clone(),
            Screen::Join { .. } => return,
        };
        if let Some(session) = self.session.take() {
            session.shutdown().await;
        }
        self.events = None;
        self.screen = Screen::Join {
            input: network_id,
            error: None,
        };
    }

    fn edit_input(&mut self, edit: impl FnOnce(&mut String)) {
        let Screen::Join { input, error } = &mut self.screen else {
            return;
        };
        edit(input);
        *error = None;
    }

    fn on_net(&mut self, event: NetEvent) {
        let Screen::Network(view) = &mut self.screen else {
            return;
        };
        match event {
            NetEvent::Ready { endpoint_id } => view.endpoint_id = endpoint_id,
            NetEvent::Status(status) => view.status = status,
            NetEvent::PeerJoined(peer) => {
                view.peers
                    .entry(peer)
                    .or_insert_with(|| "测量中".to_string());
            }
            NetEvent::PeerLeft(peer) => {
                view.peers.remove(&peer);
            }
            NetEvent::Latency { peer, rtt } => {
                if let Some(latency) = view.peers.get_mut(&peer) {
                    *latency = rtt.map(format_rtt).unwrap_or_else(|| "不可达".to_string());
                }
            }
            NetEvent::Failed(error) => view.status = error,
        }
    }
}

impl App {
    pub(crate) fn screen_kind(&self) -> ScreenView<'_> {
        match &self.screen {
            Screen::Join { input, error } => ScreenView::Join {
                input,
                error: error.as_deref(),
            },
            Screen::Network(view) => ScreenView::Network {
                network_id: &view.network_id,
                endpoint_id: &view.endpoint_id,
                peers: view
                    .peers
                    .iter()
                    .map(|(id, latency)| PeerRow { id, latency })
                    .collect(),
                status: &view.status,
            },
        }
    }
}

pub(crate) enum ScreenView<'a> {
    Join {
        input: &'a str,
        error: Option<&'a str>,
    },
    Network {
        network_id: &'a str,
        endpoint_id: &'a str,
        peers: Vec<PeerRow<'a>>,
        status: &'a str,
    },
}

pub(crate) struct PeerRow<'a> {
    pub id: &'a str,
    pub latency: &'a str,
}

fn format_rtt(rtt: Duration) -> String {
    let millis = rtt.as_secs_f64() * 1000.0;
    if millis >= 1000.0 {
        format!("{:.2} s", millis / 1000.0)
    } else if millis >= 10.0 {
        format!("{millis:.0} ms")
    } else {
        format!("{millis:.1} ms")
    }
}
