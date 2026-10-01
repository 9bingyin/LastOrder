use super::*;

#[derive(Clone, Debug)]
struct RateLimitedTransport {
    bytes_per_second: Arc<std::sync::atomic::AtomicU64>,
}
impl iroh::endpoint::ControllerFactory for RateLimitedTransport {
    fn build(self: Arc<Self>, _now: Instant, _mtu: u16) -> Box<dyn iroh::endpoint::Controller> {
        Box::new(self.as_ref().clone())
    }
}
impl iroh::endpoint::Controller for RateLimitedTransport {
    fn on_congestion_event(
        &mut self,
        _now: Instant,
        _sent: Instant,
        _persistent: bool,
        _ecn: bool,
        _lost: u64,
        _pn: u64,
    ) {
    }
    fn on_mtu_update(&mut self, _mtu: u16) {}
    fn window(&self) -> u64 {
        64 * 1024
    }
    fn initial_window(&self) -> u64 {
        self.window()
    }
    fn clone_box(&self) -> Box<dyn iroh::endpoint::Controller> {
        Box::new(self.clone())
    }
    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any> {
        self
    }
    fn metrics(&self) -> iroh::endpoint::ControllerMetrics {
        let mut metrics = iroh::endpoint::ControllerMetrics::default();
        metrics.congestion_window = self.window();
        metrics.pacing_rate = Some(
            self.bytes_per_second
                .load(std::sync::atomic::Ordering::Relaxed),
        );
        metrics.send_quantum = Some(1200);
        metrics
    }
}

#[tokio::test]
async fn real_iroh_egress_pressure_reduces_and_recovers_the_remote_publisher_budget() -> Result<()>
{
    let rate = Arc::new(std::sync::atomic::AtomicU64::new(128_000));
    let transport = iroh::endpoint::QuicTransportConfig::builder()
        .congestion_controller_factory(Arc::new(RateLimitedTransport {
            bytes_per_second: rate.clone(),
        }))
        .build();
    let endpoint = Endpoint::builder(presets::Minimal)
        .transport_config(transport)
        .bind()
        .await?;
    let coordinator = TestApp::new(endpoint);
    let publisher = TestApp::start().await?;
    let viewer = TestApp::start().await?;
    let code = coordinator.create().await?;
    publisher.join(code.clone()).await?;
    viewer.join(code).await?;
    let capture: LocalMedia = serde_json::from_value(
        publisher
            .app
            .call(
                "page".into(),
                Operation::StartShare {
                    profile: VideoProfile {
                        mode: QualityMode::Auto,
                        ..Default::default()
                    },
                    audio: false,
                },
            )
            .await?,
    )?;
    live(&publisher, &capture).await?;
    let subscription = subscribe(&viewer, &capture).await?;
    viewer
        .app
        .call(
            "page".into(),
            Operation::Playing {
                id: subscription.id,
            },
        )
        .await?;
    let generation = generation_bytes(&capture.generation)?;
    let handle = publisher.app.clone();
    publisher.tasks.spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut previous = Instant::now();
        let mut credit = 0.0f64;
        let mut sequence = 0u16;
        loop {
            tokio::select! {
                _ = handle.shutdown.cancelled() => break,
                _ = tick.tick() => {
                    let now = Instant::now();
                    let bitrate = handle.snapshots.borrow().network_bitrate.unwrap_or(6_000_000).min(6_000_000);
                    credit = (credit + f64::from(bitrate) * now.duration_since(previous).as_secs_f64()).min(32_000.0);
                    previous = now;
                    if credit < 8_000.0 { continue; }
                    credit -= 8_000.0;
                    sequence = sequence.wrapping_add(1);
                    let _ = handle.hub.packets.send(MediaPacket {
                        generation, created: now,
                        packet: Packet {
                            header: Header { version: 2, payload_type: 96, sequence_number: sequence, ssrc: 1, ..Default::default() },
                            payload: Bytes::from(vec![0; 1000]),
                        },
                    });
                }
            }
        }
    });
    let limited = wait_state(&publisher.app, |state| {
        state.network_bitrate.is_some_and(|value| value < 2_000_000)
    })
    .await?;
    let low = limited.network_bitrate.context("missing budget")?;
    rate.store(2_000_000, std::sync::atomic::Ordering::Relaxed);
    let mut snapshots = publisher.app.snapshots.clone();
    tokio::time::timeout(
        Duration::from_secs(12),
        snapshots.wait_for(|state| {
            state
                .network_bitrate
                .is_some_and(|value| f64::from(value) > f64::from(low) * 1.3)
        }),
    )
    .await??;
    publisher
        .app
        .call(
            "page".into(),
            Operation::UpdateShare {
                id: capture.share_id,
                profile: VideoProfile::default(),
            },
        )
        .await?;
    wait_state(&publisher.app, |state| state.network_bitrate.is_none()).await?;
    viewer.close().await?;
    publisher.close().await?;
    coordinator.close().await
}

fn budget_actor(app: &TestApp) -> Result<Actor> {
    let code = RoomCode::generate()?;
    let id = app.endpoint.id().to_string();
    let capture = LocalMedia {
        audio: false,
        id: random_id()?,
        share_id: random_id()?,
        generation: random_id()?,
        rtc_peer_id: random_id()?,
        client_id: "page".into(),
        state: "live".into(),
    };
    let state = Snapshot {
        event_seq: 0,
        endpoint_id: id.clone(),
        room: Some(Room {
            id: code.id(),
            owner_id: id.clone(),
            revision: 0,
            members: Default::default(),
            share: Some(Share {
                id: capture.share_id.clone(),
                generation: capture.generation.clone(),
                publisher_id: id,
                state: ShareState::Live,
                audio: false,
                profile: VideoProfile {
                    mode: QualityMode::Auto,
                    ..Default::default()
                },
            }),
        }),
        join_code: Some(code.id()),
        join_ticket: None,
        capture: Some(capture),
        subscription: None,
        connection: None,
        network_bitrate: None,
        error: None,
    };
    let (snapshots, _) = watch::channel(state.clone());
    Ok(Actor {
        endpoint: app.endpoint.clone(),
        state,
        code: Some(code),
        discovery: Discovery::Memory(Arc::new(std::sync::Mutex::new(HashMap::new()))),
        publisher: None,
        peers: HashMap::new(),
        upstream: None,
        rtc: HashMap::new(),
        share_deadline: None,
        downstream_budget: None,
        last_budget_sent: None,
        receipts: VecDeque::new(),
        snapshots,
        hub: app.app.hub.clone(),
        tasks: app.tasks.clone(),
        shutdown: app.shutdown.clone(),
        handle: app.app.clone(),
    })
}

fn budget_peer(
    connection: Connection,
    generation: &str,
    bitrate: u32,
) -> (RemotePeer, mpsc::Receiver<Wire>) {
    let (sender, receiver) = mpsc::channel(32);
    let (subscription, _) = watch::channel(Some(generation.to_owned()));
    let (publishing, _) = watch::channel(None);
    (
        RemotePeer {
            connection,
            session: "budget-session".into(),
            sender,
            subscription,
            subscription_id: Some("subscription".into()),
            publishing,
            budget: Some(LinkBudget {
                generation: generation.into(),
                bitrate,
                updated: Instant::now(),
            }),
        },
        receiver,
    )
}

#[tokio::test]
async fn automatic_budget_tracks_the_slowest_active_watcher_and_ignores_stale_samples() -> Result<()>
{
    let source = TestApp::start().await?;
    let viewer = TestApp::start().await?;
    let connection = source
        .endpoint
        .connect(viewer.endpoint.addr(), crate::protocol::ALPN)
        .await?;
    let mut actor = budget_actor(&source)?;
    let generation = actor
        .state
        .capture
        .as_ref()
        .context("capture")?
        .generation
        .clone();
    let (fast, _) = budget_peer(connection.clone(), &generation, 4_321_987);
    let (slow, _) = budget_peer(connection, &generation, 1_234_567);
    actor.peers.insert("fast".into(), fast);
    actor.peers.insert("slow".into(), slow);
    actor.refresh_budget();
    assert_eq!(actor.state.network_bitrate, Some(1_234_567));
    actor
        .peers
        .get_mut("slow")
        .context("slow peer")?
        .subscription
        .send_replace(None);
    actor.refresh_budget();
    assert_eq!(actor.state.network_bitrate, Some(4_321_987));
    actor
        .peers
        .get_mut("fast")
        .context("fast peer")?
        .budget
        .as_mut()
        .context("budget")?
        .updated = Instant::now() - Duration::from_secs(4);
    actor.refresh_budget();
    assert_eq!(actor.state.network_bitrate, Some(6_000_000));
    actor
        .state
        .room
        .as_mut()
        .context("room")?
        .share
        .as_mut()
        .context("share")?
        .profile
        .mode = QualityMode::Original;
    actor.refresh_budget();
    assert_eq!(actor.state.network_bitrate, None);
    viewer.close().await?;
    source.close().await
}

#[tokio::test]
async fn coordinator_forwards_the_budget_to_the_actual_publisher() -> Result<()> {
    let coordinator = TestApp::start().await?;
    let publisher = TestApp::start().await?;
    let connection = coordinator
        .endpoint
        .connect(publisher.endpoint.addr(), crate::protocol::ALPN)
        .await?;
    let mut actor = budget_actor(&coordinator)?;
    let generation = actor
        .state
        .capture
        .as_ref()
        .context("capture")?
        .generation
        .clone();
    let publisher_id = publisher.endpoint.id().to_string();
    actor.state.capture = None;
    actor
        .state
        .room
        .as_mut()
        .context("room")?
        .share
        .as_mut()
        .context("share")?
        .publisher_id = publisher_id.clone();
    let (remote, mut received) = budget_peer(connection.clone(), &generation, 6_000_000);
    remote.subscription.send_replace(None);
    actor.peers.insert(publisher_id, remote);
    let (viewer, _) = budget_peer(connection, &generation, 2_345_678);
    actor.peers.insert("viewer".into(), viewer);
    actor.refresh_budget();
    assert!(
        matches!(received.recv().await, Some(Wire::NetworkBudget { generation: value, bitrate: 2_345_678 }) if value == generation)
    );
    assert_eq!(actor.state.network_bitrate, None);
    publisher.close().await?;
    coordinator.close().await
}

#[tokio::test]
async fn publisher_combines_uplink_and_downstream_budgets_with_session_and_generation_checks()
-> Result<()> {
    let source = TestApp::start().await?;
    let coordinator = TestApp::start().await?;
    let connection = source
        .endpoint
        .connect(coordinator.endpoint.addr(), crate::protocol::ALPN)
        .await?;
    let mut actor = budget_actor(&source)?;
    actor.code = None;
    let generation = actor
        .state
        .capture
        .as_ref()
        .context("capture")?
        .generation
        .clone();
    let (upstream, _) = budget_peer(connection, &generation, 3_456_789);
    actor.upstream = Some(upstream);
    actor.refresh_budget();
    assert_eq!(actor.state.network_bitrate, Some(3_456_789));
    let message = Wire::NetworkBudget {
        generation: generation.clone(),
        bitrate: 1_234_567,
    };
    actor
        .network(NetworkEvent::Message {
            peer: coordinator.endpoint.id().to_string(),
            session: "stale-session".into(),
            message: message.clone(),
        })
        .await;
    actor.refresh_budget();
    assert_eq!(actor.state.network_bitrate, Some(3_456_789));
    actor
        .network(NetworkEvent::Message {
            peer: coordinator.endpoint.id().to_string(),
            session: "budget-session".into(),
            message,
        })
        .await;
    actor.refresh_budget();
    assert_eq!(actor.state.network_bitrate, Some(1_234_567));
    actor
        .network(NetworkEvent::Message {
            peer: coordinator.endpoint.id().to_string(),
            session: "budget-session".into(),
            message: Wire::NetworkBudget {
                generation: random_id()?,
                bitrate: 64_000,
            },
        })
        .await;
    actor.refresh_budget();
    assert_eq!(actor.state.network_bitrate, Some(1_234_567));
    coordinator.close().await?;
    source.close().await
}
