use super::*;

#[tokio::test]
async fn diagnostics_reports_connections_without_mutating_room_or_leaking_join_code() -> Result<()>
{
    let server = TestApp::start().await?;
    let guest = TestApp::start().await?;
    let code = server.create().await?;
    guest.join(code.clone()).await?;
    wait_state(&server.app, |state| {
        state
            .room
            .as_ref()
            .is_some_and(|room| room.members.len() == 2)
    })
    .await?;
    let staged = tempfile::NamedTempFile::new()?;
    tokio::fs::write(staged.path(), b"diagnostic file").await?;
    let offer: SharedFile = serde_json::from_value(
        server
            .app
            .call(
                "page".into(),
                Operation::OfferFile {
                    room_id: code.clone(),
                    recipient_id: guest.endpoint.id().to_string(),
                    name: "data.bin".into(),
                    size: 15,
                },
            )
            .await?,
    )?;
    wait_state(&guest.app, |state| state.files.contains_key(&offer.id)).await?;
    let accepted: SharedFile = serde_json::from_value(
        guest
            .app
            .call(
                "page".into(),
                Operation::ChangeFile {
                    room_id: code.clone(),
                    id: offer.id.clone(),
                    state: FileState::Accepted,
                },
            )
            .await?,
    )?;
    server
        .app
        .wait_file(&accepted, CancellationToken::new())
        .await?;
    let file = server
        .app
        .prepare_file(
            "page".into(),
            code.clone(),
            offer.id,
            staged.path(),
            CancellationToken::new(),
        )
        .await?;
    let revision = server
        .app
        .snapshots
        .borrow()
        .room
        .as_ref()
        .context("没有房间")?
        .revision;
    let debug = server.app.diagnostics().await?;
    assert_eq!(debug["snapshot"]["joinCode"], "[redacted]");
    assert!(!debug.to_string().contains(&code));
    assert!(
        !debug
            .to_string()
            .contains(file.blob_ticket.as_deref().context("没有文件 Ticket")?)
    );
    assert_eq!(server.app.shared_file(&code, &file.id)?, file);
    assert_eq!(
        debug["connections"][0]["remoteId"],
        guest.endpoint.id().to_string()
    );
    assert!(
        debug["connections"][0]["statsRaw"]
            .as_str()
            .is_some_and(|stats| stats.contains("udp_tx"))
    );
    assert_eq!(debug["snapshot"]["room"]["revision"], revision);
    assert_eq!(
        server.app.snapshots.borrow().join_code.as_deref(),
        Some(code.as_str())
    );
    assert_eq!(
        guest.app.diagnostics().await?["connections"][0]["role"],
        "coordinator"
    );
    guest.close().await?;
    server.close().await?;
    Ok(())
}

#[tokio::test]
async fn uuid_and_ticket_join_the_same_room_without_ticket_discovery_and_members_can_forward_it()
-> Result<()> {
    let discovery = Discovery::Memory(Arc::new(std::sync::Mutex::new(HashMap::new())));
    let server =
        TestApp::with_discovery(Endpoint::bind(presets::Minimal).await?, discovery.clone()).await?;
    let uuid_guest =
        TestApp::with_discovery(Endpoint::bind(presets::Minimal).await?, discovery).await?;
    let ticket_guest = TestApp::start().await?;
    let forwarded_guest = TestApp::start().await?;
    let code = server.create().await?;
    let ticket = server
        .app
        .snapshots
        .borrow()
        .join_ticket
        .clone()
        .context("没有 Ticket")?;
    uuid_guest.join(code.clone()).await?;
    assert!(ticket_guest.join(code.clone()).await.is_err());
    let wrong_room = RoomCode::generate()?;
    let forged = RoomTicket::create(&wrong_room, server.endpoint.addr())?;
    assert!(ticket_guest.join(forged).await.is_err());
    ticket_guest.join(ticket.clone()).await?;
    let forwarded = ticket_guest
        .app
        .snapshots
        .borrow()
        .join_ticket
        .clone()
        .context("成员没有 Ticket")?;
    let JoinTarget::Ticket {
        code: forwarded_code,
        address,
    } = JoinTarget::parse(&forwarded)?
    else {
        bail!("不是 Ticket")
    };
    assert_eq!(forwarded_code.id(), code);
    assert_eq!(address.id, server.endpoint.id());
    forwarded_guest.join(forwarded).await?;
    wait_state(&server.app, |state| {
        state
            .room
            .as_ref()
            .is_some_and(|room| room.members.len() == 4)
    })
    .await?;
    for guest in [&uuid_guest, &ticket_guest, &forwarded_guest] {
        assert_eq!(
            guest
                .app
                .snapshots
                .borrow()
                .room
                .as_ref()
                .context("没有房间")?
                .id,
            code
        );
    }
    let diagnostics = server.app.diagnostics().await?.to_string();
    assert!(!diagnostics.contains(&ticket));
    assert!(!diagnostics.contains(&code));
    forwarded_guest.close().await?;
    ticket_guest.close().await?;
    uuid_guest.close().await?;
    server.close().await?;
    Ok(())
}

#[tokio::test]
async fn joins_require_capability_and_media_requires_subscription() -> Result<()> {
    let owner = TestApp::start().await?;
    let viewer = TestApp::start().await?;
    let encoded = owner.create().await?;
    let code = RoomCode::parse(&encoded)?;
    let connection = viewer
        .endpoint
        .connect(owner.endpoint.addr(), crate::protocol::ALPN)
        .await?;
    let (mut send, mut recv) = connection.open_bi().await?;
    net::write_message(
        &mut send,
        &Wire::Join {
            version: crate::protocol::VERSION,
            room_id: code.id(),
            capability: secret()?,
            name: "成员".into(),
        },
    )
    .await?;
    assert!(matches!(
        net::read_message(&mut recv).await?,
        Wire::Error { .. }
    ));
    connection.close(0u32.into(), b"rejected test complete");
    assert_eq!(
        owner
            .app
            .snapshots
            .borrow()
            .room
            .as_ref()
            .context("没有房间")?
            .members
            .len(),
        1
    );
    viewer.join(encoded).await?;
    let capture: LocalMedia =
        serde_json::from_value(owner.app.call("page".into(), start_share()).await?)?;
    live(&owner, &capture).await?;
    wait_state(&viewer.app, |state| {
        state
            .room
            .as_ref()
            .and_then(|room| room.share.as_ref())
            .is_some_and(|share| share.state == ShareState::Live)
    })
    .await?;
    assert!(viewer.app.call("page".into(), start_share()).await.is_err());
    let mut unauthorized = viewer.app.hub.packets.subscribe();
    let generation = generation_bytes(&capture.generation)?;
    let packet = Packet {
        header: Header {
            version: 2,
            payload_type: 96,
            ..Default::default()
        },
        payload: Bytes::from_static(b"not subscribed"),
    };
    let _ = owner.app.hub.packets.send(MediaPacket {
        generation,
        packet,
        created: Instant::now(),
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(200), unauthorized.recv())
            .await
            .is_err()
    );
    let subscription = subscribe(&viewer, &capture).await?;
    assert_eq!(
        deliver(&owner, &viewer, &capture, 96).await?.packet.payload,
        Bytes::from_static(b"media payload")
    );
    viewer
        .app
        .call(
            "page".into(),
            Operation::Unsubscribe {
                id: subscription.id,
            },
        )
        .await?;
    owner
        .app
        .call(
            "page".into(),
            Operation::StopShare {
                id: capture.share_id,
            },
        )
        .await?;
    wait_state(&viewer.app, |state| {
        state.room.as_ref().is_some_and(|room| room.share.is_none())
    })
    .await?;
    viewer.close().await?;
    owner.close().await
}

#[tokio::test]
async fn explicit_room_close_is_distinct_from_connection_loss() -> Result<()> {
    let owner = TestApp::start().await?;
    let viewer = TestApp::start().await?;
    viewer.join(owner.create().await?).await?;
    owner.app.call("page".into(), Operation::Leave).await?;
    let state = wait_state(&viewer.app, |state| state.room.is_none()).await?;
    assert_eq!(state.error.as_deref(), Some("房间已结束"));
    viewer.close().await?;
    owner.close().await
}

#[tokio::test]
async fn cancelled_attach_rolls_back_membership() -> Result<()> {
    let owner = TestApp::start().await?;
    let viewer = TestApp::start().await?;
    let code = RoomCode::parse(&owner.create().await?)?;
    let connection = owner
        .endpoint
        .connect(viewer.endpoint.addr(), crate::protocol::ALPN)
        .await?;
    let (sender, _) = mpsc::channel(32);
    let (subscription, _) = watch::channel(None);
    let (publishing, _) = watch::channel(None);
    let (reply, response) = oneshot::channel();
    drop(response);
    owner
        .app
        .commands
        .send(Command::Attach {
            connection: connection.clone(),
            session: random_id()?,
            hello: Wire::Join {
                version: crate::protocol::VERSION,
                room_id: code.id(),
                capability: code.capability(),
                name: "成员".into(),
            },
            sender,
            subscription,
            publishing,
            reply,
        })
        .await?;
    tokio::time::timeout(Duration::from_secs(5), connection.closed()).await?;
    wait_state(&owner.app, |state| {
        state
            .room
            .as_ref()
            .is_some_and(|room| room.members.len() == 1)
    })
    .await?;
    let capture: LocalMedia =
        serde_json::from_value(owner.app.call("page".into(), start_share()).await?)?;
    owner
        .app
        .call("page".into(), Operation::ReleaseClient)
        .await?;
    assert!(
        owner
            .app
            .call(
                "page".into(),
                Operation::Answer {
                    peer_id: capture.rtc_peer_id,
                    sdp: String::new()
                }
            )
            .await
            .is_err()
    );
    viewer.close().await?;
    owner.close().await
}
