mod budget;
mod relay;
mod room;
mod sharing;

use anyhow::{Context, Result};
use bytes::Bytes;
use iroh::{Endpoint, endpoint::presets, protocol::Router};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use webrtc::rtp::{header::Header, packet::Packet};

use super::*;
use crate::{
    media::{MediaPacket, packet::generation_bytes},
    protocol::secret,
};

struct TestApp {
    endpoint: Endpoint,
    app: Handle,
    shutdown: CancellationToken,
    tasks: TaskTracker,
    router: Router,
}
impl TestApp {
    async fn start() -> Result<Self> {
        Ok(Self::new(Endpoint::bind(presets::Minimal).await?))
    }
    fn new(endpoint: Endpoint) -> Self {
        static RECORDS: std::sync::LazyLock<Discovery> = std::sync::LazyLock::new(|| {
            Discovery::Memory(Arc::new(std::sync::Mutex::new(HashMap::new())))
        });
        Self::with_discovery(endpoint, RECORDS.clone())
    }
    fn with_discovery(endpoint: Endpoint, discovery: Discovery) -> Self {
        let shutdown = CancellationToken::new();
        let tasks = TaskTracker::new();
        let app = Handle::spawn(endpoint.clone(), tasks.clone(), shutdown.clone(), discovery);
        let router = net::router(&endpoint, app.clone());
        Self {
            endpoint,
            app,
            shutdown,
            tasks,
            router,
        }
    }
    async fn close(self) -> Result<()> {
        self.shutdown.cancel();
        self.tasks.close();
        tokio::time::timeout(Duration::from_secs(5), self.tasks.wait()).await?;
        self.router.shutdown().await?;
        self.endpoint.close().await;
        Ok(())
    }
    async fn create(&self) -> Result<String> {
        self.app
            .call(
                "page".into(),
                Operation::Create {
                    name: "成员".into(),
                },
            )
            .await?;
        self.app
            .snapshots
            .borrow()
            .join_code
            .clone()
            .context("没有加入码")
    }
    async fn join(&self, code: String) -> Result<()> {
        self.app
            .call(
                "page".into(),
                Operation::Join {
                    code,
                    name: "成员".into(),
                },
            )
            .await?;
        Ok(())
    }
}

fn start_share() -> Operation {
    Operation::StartShare {
        profile: VideoProfile::default(),
        audio: true,
    }
}
async fn wait_state(app: &Handle, predicate: impl Fn(&Snapshot) -> bool) -> Result<Snapshot> {
    let mut snapshots = app.snapshots.clone();
    Ok(
        tokio::time::timeout(Duration::from_secs(5), snapshots.wait_for(predicate))
            .await??
            .clone(),
    )
}
async fn live(server: &TestApp, capture: &LocalMedia) -> Result<()> {
    server
        .app
        .hub
        .events
        .send(MediaEvent::Ready(capture.rtc_peer_id.clone()))
        .await?;
    Ok(())
}
async fn subscribe(server: &TestApp, capture: &LocalMedia) -> Result<LocalMedia> {
    wait_state(&server.app, |state| {
        state
            .room
            .as_ref()
            .and_then(|room| room.share.as_ref())
            .is_some_and(|share| share.state == ShareState::Live)
    })
    .await?;
    Ok(serde_json::from_value(
        server
            .app
            .call(
                "page".into(),
                Operation::Subscribe {
                    share_id: capture.share_id.clone(),
                    generation: capture.generation.clone(),
                },
            )
            .await?,
    )?)
}
async fn deliver(
    source: &TestApp,
    target: &TestApp,
    capture: &LocalMedia,
    payload_type: u8,
) -> Result<MediaPacket> {
    let generation = generation_bytes(&capture.generation)?;
    let packet = Packet {
        header: Header {
            version: 2,
            payload_type,
            sequence_number: 1,
            timestamp: 3000,
            ssrc: u32::from(payload_type),
            ..Default::default()
        },
        payload: Bytes::from_static(b"media payload"),
    };
    let mut received = target.app.hub.packets.subscribe();
    Ok(tokio::time::timeout(Duration::from_secs(10), async {
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        loop {
            tokio::select! {
                _ = tick.tick() => { let _ = source.app.hub.packets.send(MediaPacket { generation, packet: packet.clone(), created: Instant::now() }); },
                result = received.recv() => match result {
                    Ok(packet) if packet.generation == generation && packet.packet.header.payload_type == payload_type => break Ok(packet),
                    Err(error) => break Err(error),
                    _ => {},
                },
            }
        }
    }).await??)
}
