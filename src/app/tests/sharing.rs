use super::*;

#[tokio::test]
async fn local_commands_are_idempotent_and_page_scoped() -> Result<()> {
    let server = TestApp::start().await?;
    let app = &server.app;
    let request = Operation::Create {
        name: "成员".into(),
    };
    let first = app
        .call_scoped(
            "page".into(),
            request.clone(),
            CancellationToken::new(),
            Some("create".into()),
        )
        .await?;
    let second = app
        .call_scoped(
            "page".into(),
            request,
            CancellationToken::new(),
            Some("create".into()),
        )
        .await?;
    assert_eq!(first, second);
    let error = app
        .call_scoped(
            "page".into(),
            Operation::Create {
                name: "不同参数".into(),
            },
            CancellationToken::new(),
            Some("create".into()),
        )
        .await
        .err()
        .context("重复幂等键未被拒绝")?;
    let conflict = error.downcast_ref::<Fault>().context("未返回幂等冲突")?;
    assert_eq!(conflict.code, "idempotency_conflict");
    assert_eq!(conflict.status, 409);
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(
        app.call_scoped("page".into(), start_share(), cancelled, None)
            .await
            .is_err()
    );
    assert!(app.snapshots.borrow().capture.is_none());
    let capture: LocalMedia =
        serde_json::from_value(app.call("page".into(), start_share()).await?)?;
    assert!(
        app.call(
            "other".into(),
            Operation::StopShare {
                id: capture.share_id
            }
        )
        .await
        .is_err()
    );
    app.call("page".into(), Operation::ReleaseClient).await?;
    assert!(app.snapshots.borrow().capture.is_none());
    assert!(app.snapshots.borrow().room.is_some());
    server.close().await
}

#[tokio::test]
async fn any_member_can_publish_audio_and_video_to_other_members() -> Result<()> {
    let coordinator = TestApp::start().await?;
    let publisher = TestApp::start().await?;
    let viewer = TestApp::start().await?;
    let code = coordinator.create().await?;
    publisher.join(code.clone()).await?;
    viewer.join(code).await?;
    let preparing: LocalMedia =
        serde_json::from_value(publisher.app.call("page".into(), start_share()).await?)?;
    assert_eq!(preparing.state, "preparing");
    publisher
        .app
        .call("page".into(), Operation::ReleaseClient)
        .await?;
    assert!(publisher.app.snapshots.borrow().capture.is_none());
    wait_state(&coordinator.app, |state| {
        state.room.as_ref().is_some_and(|room| room.share.is_none())
    })
    .await?;
    wait_state(&publisher.app, |state| {
        state.room.as_ref().is_some_and(|room| room.share.is_none()) && state.capture.is_none()
    })
    .await?;
    let capture: LocalMedia =
        serde_json::from_value(publisher.app.call("page".into(), start_share()).await?)?;
    assert_ne!(capture.generation, preparing.generation);
    live(&publisher, &capture).await?;
    let local_sub = subscribe(&coordinator, &capture).await?;
    let remote_sub = subscribe(&viewer, &capture).await?;
    assert!(local_sub.audio && remote_sub.audio);
    for payload_type in [96, 111] {
        let local = deliver(&publisher, &coordinator, &capture, payload_type).await?;
        let remote = deliver(&publisher, &viewer, &capture, payload_type).await?;
        assert_eq!(local.packet.header.payload_type, payload_type);
        assert_eq!(remote.packet.header.payload_type, payload_type);
        assert_eq!(remote.packet.payload, Bytes::from_static(b"media payload"));
    }
    let profile = VideoProfile {
        height: 720,
        fps: 60,
        bitrate: 3_000_000,
        ..Default::default()
    };
    assert!(
        viewer
            .app
            .call(
                "page".into(),
                Operation::UpdateShare {
                    id: capture.share_id.clone(),
                    profile
                }
            )
            .await
            .is_err()
    );
    publisher
        .app
        .call(
            "page".into(),
            Operation::UpdateShare {
                id: capture.share_id.clone(),
                profile,
            },
        )
        .await?;
    wait_state(&viewer.app, |state| {
        state
            .room
            .as_ref()
            .and_then(|room| room.share.as_ref())
            .is_some_and(|share| share.profile == profile)
    })
    .await?;
    publisher
        .app
        .call("page".into(), Operation::ReleaseClient)
        .await?;
    assert!(publisher.app.snapshots.borrow().capture.is_none());
    wait_state(&coordinator.app, |state| {
        state.room.as_ref().is_some_and(|room| room.share.is_none()) && state.subscription.is_none()
    })
    .await?;
    wait_state(&viewer.app, |state| state.subscription.is_none()).await?;
    let next: LocalMedia =
        serde_json::from_value(viewer.app.call("page".into(), start_share()).await?)?;
    assert_ne!(next.generation, capture.generation);
    viewer.close().await?;
    wait_state(&coordinator.app, |state| {
        state.room.as_ref().is_some_and(|room| room.share.is_none())
    })
    .await?;
    publisher.close().await?;
    coordinator.close().await
}

#[tokio::test]
async fn concurrent_share_requests_reserve_only_one_slot() -> Result<()> {
    let coordinator = TestApp::start().await?;
    let first = TestApp::start().await?;
    let second = TestApp::start().await?;
    let code = coordinator.create().await?;
    first.join(code.clone()).await?;
    second.join(code).await?;
    let (a, b, c) = tokio::join!(
        coordinator.app.call("page".into(), start_share()),
        first.app.call("page".into(), start_share()),
        second.app.call("page".into(), start_share())
    );
    assert_eq!(
        [a.is_ok(), b.is_ok(), c.is_ok()]
            .into_iter()
            .filter(|ok| *ok)
            .count(),
        1
    );
    first.close().await?;
    second.close().await?;
    coordinator.close().await
}

#[tokio::test]
async fn replacing_a_live_publisher_session_frees_the_share_slot() -> Result<()> {
    let coordinator = TestApp::start().await?;
    let publisher = TestApp::start().await?;
    let other = TestApp::start().await?;
    let encoded = coordinator.create().await?;
    publisher.join(encoded.clone()).await?;
    other.join(encoded.clone()).await?;
    let capture: LocalMedia =
        serde_json::from_value(publisher.app.call("page".into(), start_share()).await?)?;
    live(&publisher, &capture).await?;
    wait_state(&coordinator.app, |state| {
        state
            .room
            .as_ref()
            .and_then(|room| room.share.as_ref())
            .is_some_and(|share| share.state == ShareState::Live)
    })
    .await?;
    let code = RoomCode::parse(&encoded)?;
    let replacement = net::join(
        &publisher.endpoint,
        &code,
        coordinator.endpoint.addr(),
        "新会话".into(),
    )
    .await?;
    assert!(replacement.room.share.is_none());
    wait_state(&other.app, |state| {
        state.room.as_ref().is_some_and(|room| room.share.is_none())
    })
    .await?;
    let next: LocalMedia =
        serde_json::from_value(other.app.call("page".into(), start_share()).await?)?;
    assert_ne!(next.share_id, capture.share_id);
    replacement.connection.close(0u32.into(), b"test complete");
    other.close().await?;
    publisher.close().await?;
    coordinator.close().await
}

#[tokio::test]
async fn a_full_stop_queue_disconnects_and_releases_the_live_slot() -> Result<()> {
    let coordinator = TestApp::start().await?;
    let member = TestApp::start().await?;
    let code = RoomCode::parse(&coordinator.create().await?)?;
    let mut joined = net::join(
        &member.endpoint,
        &code,
        coordinator.endpoint.addr(),
        "成员".into(),
    )
    .await?;
    let capture = LocalMedia {
        audio: true,
        id: random_id()?,
        share_id: random_id()?,
        generation: random_id()?,
        rtc_peer_id: random_id()?,
        client_id: "page".into(),
        state: "live".into(),
    };
    net::write_message(
        &mut joined.transport.send,
        &Wire::StartShare {
            share_id: capture.share_id.clone(),
            generation: capture.generation.clone(),
            profile: VideoProfile::default(),
            audio: true,
        },
    )
    .await?;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if matches!(
                net::read_message(&mut joined.transport.recv).await?,
                Wire::ShareGranted { .. }
            ) {
                break Ok::<_, anyhow::Error>(());
            }
        }
    })
    .await??;
    net::write_message(
        &mut joined.transport.send,
        &Wire::ShareReady {
            share_id: capture.share_id.clone(),
            generation: capture.generation.clone(),
        },
    )
    .await?;
    let room = wait_state(&coordinator.app, |state| {
        state
            .room
            .as_ref()
            .and_then(|room| room.share.as_ref())
            .is_some_and(|share| share.state == ShareState::Live)
    })
    .await?
    .room;
    for _ in 0..32 {
        joined.sender.try_send(Wire::Keyframe {
            generation: capture.generation.clone(),
        })?;
    }
    let state = Snapshot {
        event_seq: 0,
        endpoint_id: member.endpoint.id().to_string(),
        room,
        join_code: Some(code.id()),
        join_ticket: None,
        capture: Some(capture),
        subscription: None,
        connection: None,
        network_bitrate: None,
        error: None,
    };
    let (snapshots, _) = watch::channel(state.clone());
    let mut actor = Actor {
        endpoint: member.endpoint.clone(),
        state,
        code: None,
        discovery: Discovery::Memory(Arc::new(std::sync::Mutex::new(HashMap::new()))),
        publisher: None,
        peers: HashMap::new(),
        upstream: Some(RemotePeer {
            connection: joined.connection.clone(),
            session: joined.session,
            sender: joined.sender,
            subscription: joined.subscription,
            subscription_id: None,
            publishing: joined.publishing,
            budget: None,
        }),
        rtc: HashMap::new(),
        share_deadline: None,
        downstream_budget: None,
        last_budget_sent: None,
        receipts: VecDeque::new(),
        snapshots,
        hub: member.app.hub.clone(),
        tasks: member.tasks.clone(),
        shutdown: member.shutdown.clone(),
        handle: member.app.clone(),
    };
    actor.stop_share().await;
    assert!(joined.connection.close_reason().is_some());
    wait_state(&coordinator.app, |state| {
        state.room.as_ref().is_some_and(|room| room.share.is_none())
    })
    .await?;
    member.close().await?;
    coordinator.close().await
}

#[tokio::test]
async fn remote_share_success_responses_are_idempotent() -> Result<()> {
    let coordinator = TestApp::start().await?;
    let member = TestApp::start().await?;
    member.join(coordinator.create().await?).await?;
    let first = member
        .app
        .call_scoped(
            "page".into(),
            start_share(),
            CancellationToken::new(),
            Some("start".into()),
        )
        .await?;
    let second = member
        .app
        .call_scoped(
            "page".into(),
            start_share(),
            CancellationToken::new(),
            Some("start".into()),
        )
        .await?;
    assert_eq!(first, second);
    member.close().await?;
    coordinator.close().await
}
