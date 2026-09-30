use std::{collections::BTreeMap, time::Duration};

use crate::net::{Link, NetEvent};

pub(super) enum Screen {
    Join {
        input: String,
        error: Option<String>,
    },
    Network(NetworkView),
}

pub(super) struct NetworkView {
    pub(super) network_id: String,
    pub(super) endpoint_id: String,
    pub(super) peers: BTreeMap<String, PeerState>,
    pub(super) status: String,
}

pub(super) struct PeerState {
    latency: String,
    link: Link,
}

impl NetworkView {
    pub(super) fn apply(&mut self, event: NetEvent) {
        match event {
            NetEvent::Ready { endpoint_id } => self.endpoint_id = endpoint_id,
            NetEvent::Status(status) => self.status = status,
            NetEvent::PeerJoined(peer) => {
                self.peers.entry(peer).or_insert_with(|| PeerState {
                    latency: "测量中".to_string(),
                    link: Link::Unknown,
                });
            }
            NetEvent::PeerLeft(peer) => {
                self.peers.remove(&peer);
            }
            NetEvent::Link { peer, link } => {
                if let Some(state) = self.peers.get_mut(&peer) {
                    state.link = link;
                }
            }
            NetEvent::Latency { peer, rtt } => {
                if let Some(state) = self.peers.get_mut(&peer) {
                    state.latency = rtt.map(format_rtt).unwrap_or_else(|| "不可达".to_string());
                }
            }
            NetEvent::Failed(error) => self.status = error,
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
    pub link: Link,
}

pub(super) fn screen_view(screen: &Screen) -> ScreenView<'_> {
    match screen {
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
                .map(|(id, peer)| PeerRow {
                    id,
                    latency: &peer.latency,
                    link: peer.link,
                })
                .collect(),
            status: &view.status,
        },
    }
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
