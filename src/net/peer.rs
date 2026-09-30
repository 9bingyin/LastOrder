use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use futures_util::StreamExt;
use iroh::{
    Endpoint, EndpointAddr, EndpointId,
    endpoint::{Connection, PathList},
    protocol::{AcceptError, ProtocolHandler},
};
use tokio::sync::mpsc;

use super::{Link, NetEvent};

const PING_INTERVAL: Duration = Duration::from_secs(2);

pub(super) fn spawn(
    endpoint: Endpoint,
    peer: EndpointId,
    events: mpsc::UnboundedSender<NetEvent>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let label = peer.fmt_short().to_string();
        let connection = match endpoint
            .connect(EndpointAddr::from(peer), iroh_ping::ALPN)
            .await
        {
            Ok(connection) => connection,
            Err(_) => {
                let _ = events.send(NetEvent::Link {
                    peer: label.clone(),
                    link: Link::Down,
                });
                let _ = events.send(NetEvent::Latency {
                    peer: label,
                    rtt: None,
                });
                return;
            }
        };
        let watched = connection.clone();
        let mut paths = watched.paths_stream();
        let mut ping_interval = tokio::time::interval(PING_INTERVAL);
        ping_interval.tick().await;
        loop {
            tokio::select! {
                biased;
                event = paths.next() => {
                    match event {
                        Some(list) => {
                            if let Some(link) = link_from_paths(&list) {
                                let _ = events.send(NetEvent::Link {
                                    peer: label.clone(),
                                    link,
                                });
                            }
                        }
                        None => {
                            let _ = events.send(NetEvent::Link {
                                peer: label,
                                link: Link::Down,
                            });
                            break;
                        }
                    }
                }
                _ = ping_interval.tick() => {
                    let rtt = ping_once(&connection).await.ok();
                    let link = link_from_paths(&connection.paths()).unwrap_or(Link::Down);
                    let _ = events.send(NetEvent::Latency {
                        peer: label.clone(),
                        rtt,
                    });
                    let _ = events.send(NetEvent::Link {
                        peer: label.clone(),
                        link,
                    });
                }
            }
        }
    })
}

fn link_from_paths(paths: &PathList<'_>) -> Option<Link> {
    let path = paths.iter().find(|path| path.is_selected())?;
    Some(if path.is_ip() {
        Link::Direct
    } else if path.is_relay() {
        Link::Relay
    } else {
        Link::Down
    })
}

async fn ping_once(connection: &Connection) -> Result<Duration> {
    let (mut send, mut recv) = connection
        .open_bi()
        .await
        .context("failed to open ping stream")?;
    let start = Instant::now();
    send.write_all(b"PING")
        .await
        .context("failed to send ping")?;
    send.finish().context("failed to finish ping")?;
    let response = recv
        .read_to_end(4)
        .await
        .context("failed to read ping reply")?;
    if response.as_slice() != b"PONG" {
        return Err(anyhow!("unexpected ping reply"));
    }
    Ok(start.elapsed())
}

/// Answers `iroh-ping` without writing to the terminal.
///
/// `iroh_ping::Ping` measures the round trip, but its built-in handler prints
/// each accepted connection to stdout.
#[derive(Debug)]
pub(super) struct PingReply;

impl ProtocolHandler for PingReply {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        loop {
            let (mut send, mut recv) = match connection.accept_bi().await {
                Ok(streams) => streams,
                Err(_) => return Ok(()),
            };
            let request = recv.read_to_end(4).await.map_err(AcceptError::from_err)?;
            if request.as_slice() != b"PING" {
                return Err(AcceptError::from_err(std::io::Error::other(
                    "unexpected ping request",
                )));
            }
            send.write_all(b"PONG")
                .await
                .map_err(AcceptError::from_err)?;
            send.finish().map_err(AcceptError::from_err)?;
        }
    }
}
