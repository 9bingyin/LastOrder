use std::{collections::HashSet, str::FromStr, time::Duration};

use anyhow::{Context, Result, anyhow};
use futures_util::StreamExt;
use iroh::{
    Endpoint, EndpointAddr, EndpointId, SecretKey,
    address_lookup::{EndpointInfo, N0_DNS_PKARR_RELAY_PROD, PkarrRelayClient, UserData},
    endpoint::{Connection, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use iroh_gossip::{Gossip, TopicId, api::Event};
use iroh_ping::Ping;
use tokio::sync::{mpsc, watch};
use url::Url;

const DISCOVERY_INTERVAL: Duration = Duration::from_secs(3);
const PING_INTERVAL: Duration = Duration::from_secs(2);

pub enum NetEvent {
    Ready { endpoint_id: String },
    Status(String),
    PeerJoined(String),
    PeerLeft(String),
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
    let _ = events.send(NetEvent::Status("正在上线".to_string()));
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
    let ping = Ping::new();
    let router = Router::builder(endpoint.clone())
        .accept(iroh_gossip::ALPN, gossip.clone())
        .accept(iroh_ping::ALPN, PingReply)
        .spawn();
    let topic = topic_id(&network_id);
    let (sender, mut receiver) = gossip.subscribe(topic, Vec::new()).await?.split();
    let relay = discovery_client(&endpoint)?;
    let network_key = network_secret(&network_id);
    let mut discovery = tokio::time::interval(DISCOVERY_INTERVAL);
    let mut ping_interval = tokio::time::interval(PING_INTERVAL);
    let mut announced: Option<EndpointId> = None;
    let mut peers: HashSet<EndpointId> = HashSet::new();
    let mut ping_task: Option<tokio::task::JoinHandle<()>> = None;

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
                        if announced != Some(peer) {
                            announced = Some(peer);
                            let _ = events.send(NetEvent::Status(format!(
                                "正在连接 {}",
                                peer.fmt_short()
                            )));
                        }
                        if let Err(error) = sender.join_peers(vec![peer]).await {
                            let _ = events.send(NetEvent::Status(format!("连接失败: {error}")));
                        }
                    }
                    Ok(_) => {
                        if peers.is_empty() {
                            let _ = events.send(NetEvent::Status(
                                "等待同一网络的其他节点".to_string(),
                            ));
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
            _ = ping_interval.tick() => {
                let busy = ping_task.as_ref().is_some_and(|task| !task.is_finished());
                if !busy && !peers.is_empty() {
                    ping_task = Some(spawn_pings(
                        endpoint.clone(),
                        ping.clone(),
                        peers.iter().copied().collect(),
                        events.clone(),
                    ));
                }
            }
            event = receiver.next() => {
                match event {
                    Some(Ok(Event::NeighborUp(peer))) => {
                        let label = peer.fmt_short().to_string();
                        peers.insert(peer);
                        let _ = events.send(NetEvent::PeerJoined(label));
                        let _ = events.send(NetEvent::Status("已加入私有网络".to_string()));
                    }
                    Some(Ok(Event::NeighborDown(peer))) => {
                        let label = peer.fmt_short().to_string();
                        peers.remove(&peer);
                        let _ = events.send(NetEvent::PeerLeft(label));
                        if peers.is_empty() {
                            let _ = events.send(NetEvent::Status(
                                "等待同一网络的其他节点".to_string(),
                            ));
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

    if let Some(task) = ping_task {
        task.abort();
    }
    router.shutdown().await?;
    endpoint.close().await;
    Ok(())
}

fn spawn_pings(
    endpoint: Endpoint,
    ping: Ping,
    peers: Vec<EndpointId>,
    events: mpsc::UnboundedSender<NetEvent>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        for peer in peers {
            let rtt = ping.ping(&endpoint, EndpointAddr::from(peer)).await.ok();
            let _ = events.send(NetEvent::Latency {
                peer: peer.fmt_short().to_string(),
                rtt,
            });
        }
    })
}

/// Answers `iroh-ping` without writing to the terminal.
///
/// `iroh_ping::Ping` measures the round trip, but its built-in handler prints
/// each accepted connection to stdout.
#[derive(Debug)]
struct PingReply;

impl ProtocolHandler for PingReply {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let (mut send, mut recv) = connection
            .accept_bi()
            .await
            .map_err(AcceptError::from_err)?;
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
        connection.closed().await;
        Ok(())
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

    use super::{NetEvent, Session, generate_network_id, network_secret, topic_id};

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
            loop {
                tokio::select! {
                    event = rx_a.recv() => {
                        note_event(event, &mut notes, &mut joined[0], &mut measured[0])?;
                    }
                    event = rx_b.recv() => {
                        note_event(event, &mut notes, &mut joined[1], &mut measured[1])?;
                    }
                }
                if joined == [true, true] && measured == [true, true] {
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
                "peers with the same network id did not find each other or report latency: {}",
                notes.join(" | ")
            )),
        }
    }

    fn note_event(
        event: Option<NetEvent>,
        notes: &mut Vec<String>,
        joined: &mut bool,
        measured: &mut bool,
    ) -> Result<()> {
        match event {
            Some(NetEvent::PeerJoined(_)) => *joined = true,
            Some(NetEvent::Latency { rtt: Some(_), .. }) => *measured = true,
            Some(NetEvent::Status(status) | NetEvent::Failed(status)) => notes.push(status),
            Some(_) => {}
            None => return Err(anyhow!("network session closed")),
        }
        Ok(())
    }
}
