use std::{collections::HashMap, time::Duration};

use anyhow::Result;
use futures_util::StreamExt;
use iroh::{
    Endpoint, EndpointId, SecretKey, address_lookup::PkarrRelayClient, endpoint::presets,
    protocol::Router,
};
use iroh_gossip::{
    Gossip,
    api::{ApiError, Event, GossipSender},
};
use tokio::sync::{mpsc, watch};

mod discovery;
mod peer;

pub use discovery::generate_network_id;

/// How this node currently reaches a member.
///
/// `Direct` is a selected IP path. `Relay` is a selected relay path.
/// `Down` means the connection is gone or has no usable path. `Unknown`
/// is only the state before the first observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    Unknown,
    Direct,
    Relay,
    Down,
}

pub enum NetEvent {
    Ready { endpoint_id: String },
    Status(String),
    PeerJoined(String),
    PeerLeft(String),
    Link { peer: String, link: Link },
    Latency { peer: String, rtt: Option<Duration> },
    Failed(String),
}

pub struct Session {
    shutdown: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

impl Session {
    pub fn spawn(network_id: String, events: mpsc::UnboundedSender<NetEvent>) -> Self {
        let (shutdown, notify) = watch::channel(false);
        let task = tokio::spawn(async move {
            if let Err(error) = run(network_id, events.clone(), notify).await {
                let _ = events.send(NetEvent::Failed(error.to_string()));
            }
        });
        Self { shutdown, task }
    }

    pub async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        let _ = self.task.await;
    }
}

const DISCOVERY_INTERVAL: Duration = Duration::from_secs(3);

async fn run(
    network_id: String,
    events: mpsc::UnboundedSender<NetEvent>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let endpoint = Endpoint::bind(presets::N0).await?;
    tokio::select! {
        biased;
        result = shutdown.changed() => {
            result?;
            endpoint.close().await;
            return Ok(());
        }
        () = endpoint.online() => {}
    }
    if *shutdown.borrow() {
        endpoint.close().await;
        return Ok(());
    }

    let endpoint_id = endpoint.id();
    let _ = events.send(NetEvent::Ready {
        endpoint_id: endpoint_id.fmt_short().to_string(),
    });

    let gossip = Gossip::builder().spawn(endpoint.clone());
    let router = Router::builder(endpoint.clone())
        .accept(iroh_gossip::ALPN, gossip.clone())
        .accept(iroh_ping::ALPN, peer::PingReply)
        .spawn();
    let topic = discovery::topic_id(&network_id);
    let (sender, mut receiver) = gossip.subscribe(topic, Vec::new()).await?.split();
    let relay = discovery::discovery_client(&endpoint)?;
    let network_key = discovery::network_secret(&network_id);
    let mut discovery_tick = tokio::time::interval(DISCOVERY_INTERVAL);
    let mut peers = HashMap::<EndpointId, tokio::task::JoinHandle<()>>::new();

    loop {
        tokio::select! {
            biased;
            result = shutdown.changed() => {
                result?;
                break;
            }
            _ = discovery_tick.tick() => {
                discover(
                    &relay,
                    &network_key,
                    endpoint_id,
                    &sender,
                    peers.is_empty(),
                    &events,
                )
                .await;
                if let Err(error) =
                    discovery::publish_self(&relay, &network_key, endpoint_id).await
                {
                    let _ = events.send(NetEvent::Status(format!("发布网络入口失败: {error}")));
                }
            }
            event = receiver.next() => {
                if !on_gossip(event, endpoint.clone(), &mut peers, &events) {
                    break;
                }
            }
        }
    }

    for task in peers.into_values() {
        task.abort();
    }
    router.shutdown().await?;
    endpoint.close().await;
    Ok(())
}

async fn discover(
    relay: &PkarrRelayClient,
    network_key: &SecretKey,
    endpoint_id: EndpointId,
    sender: &GossipSender,
    no_peers: bool,
    events: &mpsc::UnboundedSender<NetEvent>,
) {
    match discovery::lookup_bootstrap(relay, network_key).await {
        Ok(Some(peer)) if peer != endpoint_id => {
            if let Err(error) = sender.join_peers(vec![peer]).await {
                let _ = events.send(NetEvent::Status(format!("连接失败: {error}")));
            }
        }
        Ok(_) => {
            if no_peers {
                let _ = events.send(NetEvent::Status(String::new()));
            }
        }
        Err(error) => {
            let _ = events.send(NetEvent::Status(format!("查找网络入口失败: {error}")));
        }
    }
}

fn on_gossip(
    event: Option<Result<Event, ApiError>>,
    endpoint: Endpoint,
    peers: &mut HashMap<EndpointId, tokio::task::JoinHandle<()>>,
    events: &mpsc::UnboundedSender<NetEvent>,
) -> bool {
    match event {
        Some(Ok(Event::NeighborUp(peer))) => {
            let label = peer.fmt_short().to_string();
            let _ = events.send(NetEvent::PeerJoined(label));
            let restart = peers
                .get(&peer)
                .is_none_or(tokio::task::JoinHandle::is_finished);
            if restart {
                if let Some(task) = peers.remove(&peer) {
                    task.abort();
                }
                peers.insert(peer, peer::spawn(endpoint, peer, events.clone()));
            }
            true
        }
        Some(Ok(Event::NeighborDown(peer))) => {
            let label = peer.fmt_short().to_string();
            if let Some(task) = peers.remove(&peer) {
                task.abort();
            }
            let _ = events.send(NetEvent::PeerLeft(label));
            if peers.is_empty() {
                let _ = events.send(NetEvent::Status(String::new()));
            }
            true
        }
        Some(Ok(_)) => true,
        Some(Err(error)) => {
            let _ = events.send(NetEvent::Status(format!("网络事件失败: {error}")));
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use anyhow::{Result, anyhow};
    use tokio::sync::mpsc;

    use super::{Link, NetEvent, Session, generate_network_id};

    #[tokio::test]
    async fn same_network_id_discovers_a_peer() -> Result<()> {
        let network_id = generate_network_id()?;
        let (tx_a, mut rx_a) = mpsc::unbounded_channel();
        let (tx_b, mut rx_b) = mpsc::unbounded_channel();
        let session_a = Session::spawn(network_id.clone(), tx_a);
        let session_b = Session::spawn(network_id, tx_b);
        let mut notes = Vec::new();

        let discovered = tokio::time::timeout(std::time::Duration::from_secs(45), async {
            let mut joined = [false, false];
            let mut measured = [false, false];
            let mut linked = [false, false];
            loop {
                tokio::select! {
                    event = rx_a.recv() => {
                        note_event(
                            event,
                            &mut notes,
                            &mut joined[0],
                            &mut measured[0],
                            &mut linked[0],
                        )?;
                    }
                    event = rx_b.recv() => {
                        note_event(
                            event,
                            &mut notes,
                            &mut joined[1],
                            &mut measured[1],
                            &mut linked[1],
                        )?;
                    }
                }
                if joined == [true, true] && measured == [true, true] && linked == [true, true] {
                    return Ok(());
                }
            }
        })
        .await;

        session_a.shutdown().await;
        session_b.shutdown().await;

        match discovered {
            Ok(result) => result,
            Err(_) => Err(anyhow!(
                "peers with the same network id did not find each other, report latency, or classify the path: {}",
                notes.join(" | ")
            )),
        }
    }

    fn note_event(
        event: Option<NetEvent>,
        notes: &mut Vec<String>,
        joined: &mut bool,
        measured: &mut bool,
        linked: &mut bool,
    ) -> Result<()> {
        match event {
            Some(NetEvent::PeerJoined(_)) => *joined = true,
            Some(NetEvent::Latency { rtt: Some(_), .. }) => *measured = true,
            Some(NetEvent::Link {
                link: Link::Direct | Link::Relay,
                ..
            }) => *linked = true,
            Some(NetEvent::Status(status) | NetEvent::Failed(status)) => notes.push(status),
            Some(_) => {}
            None => return Err(anyhow!("network session closed")),
        }
        Ok(())
    }
}
