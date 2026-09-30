use std::{
    collections::{HashMap, HashSet},
    str::FromStr,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use futures_util::StreamExt;
use iroh::{
    Endpoint, EndpointAddr, EndpointId, SecretKey,
    address_lookup::{EndpointInfo, N0_DNS_PKARR_RELAY_PROD, PkarrRelayClient, UserData},
    endpoint::{Connection, PathList, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use iroh_gossip::{Gossip, TopicId, api::Event};
use tokio::sync::{mpsc, watch};
use url::Url;

const DISCOVERY_INTERVAL: Duration = Duration::from_secs(3);
const PING_INTERVAL: Duration = Duration::from_secs(2);

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

pub fn generate_network_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).context("failed to generate network id")?;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let mut network_id = String::with_capacity(hex.len() + hex.len() / 4);
    for (index, character) in hex.chars().enumerate() {
        if index > 0 && index.is_multiple_of(4) {
            network_id.push('-');
        }
        network_id.push(character);
    }
    Ok(network_id)
}

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
        .accept(iroh_ping::ALPN, PingReply)
        .spawn();
    let topic = topic_id(&network_id);
    let (sender, mut receiver) = gossip.subscribe(topic, Vec::new()).await?.split();
    let relay = discovery_client(&endpoint)?;
    let network_key = network_secret(&network_id);
    let mut discovery = tokio::time::interval(DISCOVERY_INTERVAL);
    let mut peers: HashSet<EndpointId> = HashSet::new();
    let mut peer_tasks: HashMap<EndpointId, tokio::task::JoinHandle<()>> = HashMap::new();

    loop {
        tokio::select! {
            biased;
            result = shutdown.changed() => {
                result?;
                break;
            }
            _ = discovery.tick() => {
                match lookup_bootstrap(&relay, &network_key).await {
                    Ok(Some(peer)) if peer != endpoint_id => {
                        if let Err(error) = sender.join_peers(vec![peer]).await {
                            let _ = events.send(NetEvent::Status(format!("连接失败: {error}")));
                        }
                    }
                    Ok(_) => {
                        if peers.is_empty() {
                            let _ = events.send(NetEvent::Status(String::new()));
                        }
                    }
                    Err(error) => {
                        let _ = events.send(NetEvent::Status(format!("查找网络入口失败: {error}")));
                    }
                }
                if let Err(error) = publish_self(&relay, &network_key, endpoint_id).await {
                    let _ = events.send(NetEvent::Status(format!("发布网络入口失败: {error}")));
                }
            }
            event = receiver.next() => {
                match event {
                    Some(Ok(Event::NeighborUp(peer))) => {
                        let label = peer.fmt_short().to_string();
                        peers.insert(peer);
                        let _ = events.send(NetEvent::PeerJoined(label));
                        let restart = peer_tasks
                            .get(&peer)
                            .is_none_or(tokio::task::JoinHandle::is_finished);
                        if restart {
                            if let Some(task) = peer_tasks.remove(&peer) {
                                task.abort();
                            }
                            peer_tasks.insert(
                                peer,
                                spawn_peer(endpoint.clone(), peer, events.clone()),
                            );
                        }
                    }
                    Some(Ok(Event::NeighborDown(peer))) => {
                        let label = peer.fmt_short().to_string();
                        peers.remove(&peer);
                        if let Some(task) = peer_tasks.remove(&peer) {
                            task.abort();
                        }
                        let _ = events.send(NetEvent::PeerLeft(label));
                        if peers.is_empty() {
                            let _ = events.send(NetEvent::Status(String::new()));
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(error)) => {
                        let _ = events.send(NetEvent::Status(format!("网络事件失败: {error}")));
                    }
                    None => break,
                }
            }
        }
    }

    for task in peer_tasks.into_values() {
        task.abort();
    }
    router.shutdown().await?;
    endpoint.close().await;
    Ok(())
}

fn spawn_peer(
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
struct PingReply;

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

fn discovery_client(endpoint: &Endpoint) -> Result<PkarrRelayClient> {
    let relay_url = Url::parse(N0_DNS_PKARR_RELAY_PROD).context("invalid discovery relay url")?;
    Ok(PkarrRelayClient::new(
        relay_url,
        endpoint.tls_config().clone(),
        endpoint.dns_resolver()?.clone(),
    ))
}

async fn publish_self(
    relay: &PkarrRelayClient,
    network_key: &SecretKey,
    endpoint_id: EndpointId,
) -> Result<()> {
    let user_data = UserData::from_str(&endpoint_id.to_z32())
        .map_err(|error| anyhow!("endpoint id does not fit discovery record: {error}"))?;
    let info = EndpointInfo::new(network_key.public()).with_user_data(Some(user_data));
    let packet = info
        .to_pkarr_signed_packet(network_key, 30)
        .map_err(|error| anyhow!("failed to encode discovery record: {error}"))?;
    relay.publish(&packet).await?;
    Ok(())
}

async fn lookup_bootstrap(
    relay: &PkarrRelayClient,
    network_key: &SecretKey,
) -> Result<Option<EndpointId>> {
    let packet = match relay.resolve(network_key.public()).await {
        Ok(packet) => packet,
        Err(error) => {
            if error.to_string().contains("404") {
                return Ok(None);
            }
            return Err(error.into());
        }
    };
    let info = EndpointInfo::from_pkarr_signed_packet(&packet)
        .map_err(|error| anyhow!("failed to decode discovery record: {error}"))?;
    let Some(user_data) = info.user_data() else {
        return Ok(None);
    };
    let endpoint_id = EndpointId::from_z32(user_data.as_ref())
        .map_err(|error| anyhow!("invalid endpoint id in discovery record: {error}"))?;
    Ok(Some(endpoint_id))
}

fn topic_id(network_id: &str) -> TopicId {
    TopicId::from_bytes(blake3::derive_key(
        "lastorder.topic.v1",
        network_id.as_bytes(),
    ))
}

fn network_secret(network_id: &str) -> SecretKey {
    SecretKey::from_bytes(&blake3::derive_key(
        "lastorder.network.v1",
        network_id.as_bytes(),
    ))
}

#[cfg(test)]
mod tests {
    use anyhow::{Result, anyhow};
    use tokio::sync::mpsc;

    use super::{Link, NetEvent, Session, generate_network_id, network_secret, topic_id};

    #[test]
    fn generated_network_id_is_grouped_hex() -> Result<()> {
        let network_id = generate_network_id()?;
        let parts: Vec<_> = network_id.split('-').collect();
        assert_eq!(parts.len(), 8);
        assert!(parts.iter().all(|part| part.len() == 4));
        Ok(())
    }

    #[test]
    fn network_id_derivation_is_stable() {
        assert_eq!(topic_id("alpha"), topic_id("alpha"));
        assert_ne!(topic_id("alpha"), topic_id("beta"));
        assert_eq!(
            network_secret("alpha").public(),
            network_secret("alpha").public()
        );
        assert_ne!(
            network_secret("alpha").public(),
            network_secret("beta").public()
        );
    }

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
