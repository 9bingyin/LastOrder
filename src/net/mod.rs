mod media;
use media::{receive_media, send_media};

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use iroh::{
    Endpoint, EndpointAddr,
    endpoint::{Connection, ConnectionError, RecvStream, SendStream},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use tokio::sync::{Semaphore, broadcast, mpsc, watch};
use tokio_util::sync::CancellationToken;
use webrtc::{
    rtp::packet::Packet,
    util::marshal::{Marshal, Unmarshal},
};

use crate::{
    app::{DisconnectReason, Handle, NetworkEvent},
    media::{
        Hub, MediaPacket,
        packet::{Reassembler, fragment, generation_bytes},
    },
    protocol::{ALPN, MAX_MESSAGE, Room, Wire, random_id},
};

pub mod budget;
pub mod discovery;

const TIMEOUT: Duration = Duration::from_secs(10);

struct MembershipLease {
    app: Handle,
    peer: String,
    session: String,
    connection: Connection,
}
impl Drop for MembershipLease {
    fn drop(&mut self) {
        let reason = match self.connection.close_reason() {
            Some(ConnectionError::ApplicationClosed(close))
                if close.error_code == crate::protocol::ROOM_CLOSED.into() =>
            {
                DisconnectReason::RoomClosed
            }
            _ => DisconnectReason::ConnectionLost,
        };
        self.app
            .detached(self.peer.clone(), self.session.clone(), reason);
    }
}

pub struct Transport {
    pub send: SendStream,
    pub recv: RecvStream,
    pub outgoing: mpsc::Receiver<Wire>,
    pub subscribed: watch::Receiver<Option<String>>,
    pub publishing: watch::Receiver<Option<String>>,
}

pub async fn write_message(send: &mut SendStream, message: &Wire) -> Result<()> {
    let bytes = serde_json::to_vec(message)?;
    if bytes.len() > MAX_MESSAGE {
        bail!("控制消息过大");
    }
    send.write_all(&u32::try_from(bytes.len())?.to_be_bytes())
        .await?;
    send.write_all(&bytes).await?;
    Ok(())
}

pub async fn read_message(recv: &mut RecvStream) -> Result<Wire> {
    let mut length = [0; 4];
    recv.read_exact(&mut length).await?;
    let length = usize::try_from(u32::from_be_bytes(length))?;
    if length == 0 || length > MAX_MESSAGE {
        bail!("控制消息长度无效");
    }
    let mut bytes = vec![0; length];
    recv.read_exact(&mut bytes).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[derive(Debug)]
struct Handler {
    app: Handle,
    connections: Semaphore,
}

impl ProtocolHandler for Handler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let Ok(_permit) = self.connections.try_acquire() else {
            connection.close(0u32.into(), b"connection limit");
            return Ok(());
        };
        let result = accept(self.app.clone(), connection.clone()).await;
        if connection.close_reason().is_none() {
            connection.close(0u32.into(), b"session ended");
        }
        result.map_err(|error| AcceptError::from_err(std::io::Error::other(error.to_string())))
    }
}

pub fn router(endpoint: &Endpoint, app: Handle) -> Router {
    Router::builder(endpoint.clone())
        .accept(
            ALPN,
            Handler {
                app,
                connections: Semaphore::new(16),
            },
        )
        .spawn()
}

async fn accept(app: Handle, connection: Connection) -> Result<()> {
    let (mut send, mut recv) = tokio::time::timeout(TIMEOUT, connection.accept_bi()).await??;
    let hello = tokio::time::timeout(TIMEOUT, read_message(&mut recv)).await??;
    let session = random_id()?;
    let _membership = MembershipLease {
        app: app.clone(),
        peer: connection.remote_id().to_string(),
        session: session.clone(),
        connection: connection.clone(),
    };
    let (tx, rx) = mpsc::channel(32);
    let (subscription, subscribed) = watch::channel(None);
    let (publishing, published) = watch::channel(None);
    let attached = app
        .attach(
            connection.clone(),
            session.clone(),
            hello,
            tx,
            subscription,
            publishing,
        )
        .await;
    let room = match attached {
        Ok(room) => room,
        Err(error) => {
            tokio::time::timeout(
                TIMEOUT,
                write_message(
                    &mut send,
                    &Wire::Error {
                        message: error.to_string(),
                    },
                ),
            )
            .await??;
            send.finish()?;
            let _ = tokio::time::timeout(Duration::from_secs(1), send.stopped()).await;
            return Ok(());
        }
    };
    tokio::time::timeout(TIMEOUT, write_message(&mut send, &Wire::Snapshot { room })).await??;
    serve(
        app,
        connection,
        session,
        Transport {
            send,
            recv,
            outgoing: rx,
            subscribed,
            publishing: published,
        },
        true,
    )
    .await;
    Ok(())
}

pub struct Joined {
    pub connection: Connection,
    pub session: String,
    pub room: Room,
    pub sender: mpsc::Sender<Wire>,
    pub subscription: watch::Sender<Option<String>>,
    pub publishing: watch::Sender<Option<String>>,
    pub transport: Transport,
}

pub async fn join(
    endpoint: &Endpoint,
    code: &crate::protocol::RoomCode,
    address: EndpointAddr,
    name: String,
) -> Result<Joined> {
    tokio::time::timeout(Duration::from_secs(25), async {
        let connection = endpoint.connect(address, ALPN).await?;
        let result = async {
            if connection.max_datagram_size().is_none() {
                bail!("对端不支持实时媒体 Datagram");
            }
            let (mut send, mut recv) = connection.open_bi().await?;
            write_message(
                &mut send,
                &Wire::Join {
                    version: 3,
                    room_id: code.id(),
                    capability: code.capability(),
                    name,
                },
            )
            .await?;
            let room = match read_message(&mut recv).await? {
                Wire::Snapshot { room }
                    if room.id == code.id()
                        && room.owner_id == connection.remote_id().to_string() =>
                {
                    room
                }
                Wire::Error { message } => bail!("{message}"),
                _ => bail!("房间握手响应无效"),
            };
            let (sender, outgoing) = mpsc::channel(32);
            let (subscription, subscribed) = watch::channel(None);
            let (publishing, published) = watch::channel(None);
            Ok(Joined {
                connection: connection.clone(),
                session: random_id()?,
                room,
                sender,
                subscription,
                publishing,
                transport: Transport {
                    send,
                    recv,
                    outgoing,
                    subscribed,
                    publishing: published,
                },
            })
        }
        .await;
        if result.is_err() {
            connection.close(0u32.into(), b"join rejected");
        }
        result
    })
    .await
    .context("加入房间超时")?
}

async fn receive_control(
    app: &Handle,
    peer: &str,
    session: &str,
    recv: &mut RecvStream,
) -> Result<()> {
    loop {
        let message = read_message(recv).await?;
        app.network(NetworkEvent::Message {
            peer: peer.into(),
            session: session.into(),
            message,
        })
        .await?;
    }
}

pub async fn serve(
    app: Handle,
    connection: Connection,
    session: String,
    transport: Transport,
    owner_side: bool,
) {
    let Transport {
        mut send,
        mut recv,
        mut outgoing,
        subscribed,
        publishing,
    } = transport;
    let (mut sending, mut receiving) = if owner_side {
        (subscribed, publishing)
    } else {
        (publishing, subscribed)
    };
    let peer = connection.remote_id().to_string();
    let _membership = MembershipLease {
        app: app.clone(),
        peer: peer.clone(),
        session: session.clone(),
        connection: connection.clone(),
    };
    let hub = app.hub.clone();
    let reader = receive_control(&app, &peer, &session, &mut recv);
    let writer = async {
        while let Some(message) = outgoing.recv().await {
            tokio::time::timeout(TIMEOUT, write_message(&mut send, &message)).await??;
        }
        Ok::<(), anyhow::Error>(())
    };
    let sender = send_media(
        connection.clone(),
        hub.clone(),
        &mut sending,
        app.shutdown.clone(),
        peer.clone(),
        session.clone(),
    );
    let receiver = receive_media(
        connection.clone(),
        hub,
        &mut receiving,
        app.shutdown.clone(),
    );
    tokio::select! {
        _ = app.shutdown.cancelled() => {},
        result = reader => { if let Err(error) = result { tracing::debug!(%error, %peer, "控制读取结束"); } },
        result = writer => { if let Err(error) = result { tracing::debug!(%error, %peer, "控制发送结束"); } },
        result = sender => { if let Err(error) = result { tracing::debug!(%error, %peer, "媒体发送结束"); } },
        result = receiver => { if let Err(error) = result { tracing::debug!(%error, %peer, "媒体接收结束"); } },
    }
    let code = if owner_side && app.shutdown.is_cancelled() {
        crate::protocol::ROOM_CLOSED
    } else {
        0
    };
    if connection.close_reason().is_none() {
        connection.close(code.into(), b"session ended");
    }
}
