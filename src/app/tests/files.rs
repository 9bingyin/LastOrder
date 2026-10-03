use super::*;
use futures_util::StreamExt;
use iroh_blobs::{
    get::request::{GetBlobItem, get_blob},
    protocol::{ChunkRanges, ChunkRangesExt, GetRequest},
};

async fn offer(
    sender: &TestApp,
    receiver: &TestApp,
    room: &str,
    name: &str,
    size: u64,
) -> Result<SharedFile> {
    let file: SharedFile = serde_json::from_value(
        sender
            .app
            .call(
                "page".into(),
                Operation::OfferFile {
                    room_id: room.into(),
                    recipient_id: receiver.endpoint.id().to_string(),
                    name: name.into(),
                    size,
                },
            )
            .await?,
    )?;
    sender
        .app
        .wait_file(&file, CancellationToken::new())
        .await?;
    wait_state(&receiver.app, |state| {
        state.files.get(&file.id) == Some(&file)
    })
    .await?;
    Ok(file)
}
async fn change(app: &TestApp, file: &SharedFile, state: FileState) -> Result<SharedFile> {
    let changed: SharedFile = serde_json::from_value(
        app.app
            .call(
                "page".into(),
                Operation::ChangeFile {
                    room_id: file.room_id.clone(),
                    id: file.id.clone(),
                    state,
                },
            )
            .await?,
    )?;
    app.app.wait_file(&changed, CancellationToken::new()).await
}
async fn cannot_fetch(app: &TestApp, publisher: &TestApp, hash: iroh_blobs::Hash) -> Result<()> {
    let connection = app
        .endpoint
        .connect(publisher.endpoint.addr(), iroh_blobs::ALPN)
        .await?;
    assert!(
        tokio::time::timeout(
            Duration::from_secs(5),
            get_blob(connection.clone(), hash).bytes()
        )
        .await?
        .is_err()
    );
    connection.close(0u32.into(), b"test finished");
    Ok(())
}

#[tokio::test]
async fn targeted_invitation_requires_acceptance_and_only_the_recipient_receives_verified_data()
-> Result<()> {
    let owner = TestApp::start().await?;
    let sender = TestApp::start().await?;
    let receiver = TestApp::start().await?;
    let other = TestApp::start().await?;
    let result: Result<()> = async {
        let room = owner.create().await?;
        sender.join(room.clone()).await?;
        receiver.join(room.clone()).await?;
        other.join(room.clone()).await?;
        wait_state(&sender.app, |state| {
            state
                .room
                .as_ref()
                .is_some_and(|room| room.members.len() == 4)
        })
        .await?;
        let staged = tempfile::NamedTempFile::new()?;
        let payload: Vec<u8> = (0..256 * 1024).map(|index| (index % 251) as u8).collect();
        tokio::fs::write(staged.path(), &payload).await?;
        let seeded = sender
            .app
            .files
            .store
            .blobs()
            .add_slice(&payload)
            .temp_tag()
            .await?;
        let file = offer(
            &sender,
            &receiver,
            &room,
            "定向发送.bin",
            payload.len() as u64,
        )
        .await?;
        assert!(
            owner.app.snapshots.borrow().files.is_empty(),
            "协调者不能收到其他两人的文件邀请"
        );
        assert!(other.app.snapshots.borrow().files.is_empty());
        assert!(file.blob_ticket.is_none());
        assert!(
            sender
                .app
                .prepare_file(
                    "page".into(),
                    room.clone(),
                    file.id.clone(),
                    staged.path(),
                    CancellationToken::new()
                )
                .await
                .is_err()
        );
        cannot_fetch(&receiver, &sender, seeded.hash()).await?;
        assert!(
            owner
                .app
                .call(
                    "page".into(),
                    Operation::ChangeFile {
                        room_id: room.clone(),
                        id: file.id.clone(),
                        state: FileState::Accepted
                    }
                )
                .await
                .is_err()
        );
        let acceptance = Operation::ChangeFile {
            room_id: room.clone(),
            id: file.id.clone(),
            state: FileState::Accepted,
        };
        let (first, duplicate) = tokio::join!(
            receiver.app.call("page".into(), acceptance.clone()),
            receiver.app.call("other-page".into(), acceptance)
        );
        let accepted: SharedFile = serde_json::from_value(first?)?;
        assert!(duplicate.is_err(), "另一页面不能重复处理已确认的邀请");
        receiver
            .app
            .wait_file(&accepted, CancellationToken::new())
            .await?;
        sender
            .app
            .wait_file(&accepted, CancellationToken::new())
            .await?;
        cannot_fetch(&receiver, &sender, seeded.hash()).await?;
        let ready = sender
            .app
            .prepare_file(
                "page".into(),
                room.clone(),
                file.id.clone(),
                staged.path(),
                CancellationToken::new(),
            )
            .await?;
        wait_state(&receiver.app, |state| {
            state.files.get(&file.id) == Some(&ready)
        })
        .await?;
        cannot_fetch(&other, &sender, seeded.hash()).await?;
        cannot_fetch(&owner, &sender, seeded.hash()).await?;
        assert!(
            receiver
                .app
                .download_file("other-page", &room, &file.id, CancellationToken::new())
                .await
                .is_err()
        );
        let (first, second) = tokio::try_join!(
            receiver
                .app
                .download_file("page", &room, &file.id, CancellationToken::new()),
            receiver
                .app
                .download_file("page", &room, &file.id, CancellationToken::new()),
        )?;
        for entry in [&first, &second] {
            assert_eq!(
                receiver
                    .app
                    .files
                    .store
                    .blobs()
                    .get_bytes(entry.tag.hash())
                    .await?
                    .as_ref(),
                payload
            );
        }
        let completed = change(&receiver, &ready, FileState::Completed).await?;
        sender
            .app
            .wait_file(&completed, CancellationToken::new())
            .await?;
        assert!(first.cancellation.is_cancelled());
        cannot_fetch(&receiver, &sender, seeded.hash()).await?;
        Ok(())
    }
    .await;
    other.close().await?;
    receiver.close().await?;
    sender.close().await?;
    owner.close().await?;
    result
}

#[tokio::test]
async fn rejecting_cancelling_and_leaving_stop_pending_or_ready_transfers() -> Result<()> {
    let owner = TestApp::start().await?;
    let receiver = TestApp::start().await?;
    let result: Result<()> = async {
        let room = owner.create().await?;
        receiver.join(room.clone()).await?;
        let rejected = offer(&owner, &receiver, &room, "reject.bin", 0).await?;
        let rejected = change(&receiver, &rejected, FileState::Rejected).await?;
        assert_eq!(
            owner
                .app
                .wait_file(&rejected, CancellationToken::new())
                .await?
                .state,
            FileState::Rejected
        );
        let staged = tempfile::NamedTempFile::new()?;
        assert!(
            owner
                .app
                .prepare_file(
                    "page".into(),
                    room.clone(),
                    rejected.id,
                    staged.path(),
                    CancellationToken::new()
                )
                .await
                .is_err()
        );
        let pending = offer(&owner, &receiver, &room, "cancel.bin", 0).await?;
        owner
            .app
            .call("page".into(), Operation::ReleaseClient)
            .await?;
        wait_state(&receiver.app, |state| {
            state
                .files
                .get(&pending.id)
                .is_some_and(|file| file.state == FileState::Cancelled)
        })
        .await?;
        let offered = offer(&owner, &receiver, &room, "empty.bin", 0).await?;
        let accepted = change(&receiver, &offered, FileState::Accepted).await?;
        owner
            .app
            .wait_file(&accepted, CancellationToken::new())
            .await?;
        let ready = owner
            .app
            .prepare_file(
                "page".into(),
                room.clone(),
                offered.id.clone(),
                staged.path(),
                CancellationToken::new(),
            )
            .await?;
        receiver
            .app
            .wait_file(&ready, CancellationToken::new())
            .await?;
        let entry = receiver
            .app
            .download_file("page", &room, &ready.id, CancellationToken::new())
            .await?;
        assert!(
            receiver
                .app
                .files
                .store
                .blobs()
                .get_bytes(entry.tag.hash())
                .await?
                .is_empty()
        );
        receiver.app.call("page".into(), Operation::Leave).await?;
        wait_state(&owner.app, |state| {
            state
                .files
                .get(&ready.id)
                .is_some_and(|file| file.state == FileState::Cancelled)
        })
        .await?;
        assert!(owner.app.files.cached(&ready).is_none());
        Ok(())
    }
    .await;
    receiver.close().await?;
    owner.close().await?;
    result
}

#[tokio::test]
async fn confirmed_transfer_resumes_verified_partial_data_after_an_interrupted_attempt()
-> Result<()> {
    let owner = TestApp::start().await?;
    let receiver = TestApp::start().await?;
    let result: Result<()> = async {
        let room = owner.create().await?;
        receiver.join(room.clone()).await?;
        let staged = tempfile::NamedTempFile::new()?;
        let payload: Vec<u8> = (0..1024 * 1024).map(|index| (index % 251) as u8).collect();
        tokio::fs::write(staged.path(), &payload).await?;
        let file = offer(
            &owner,
            &receiver,
            &room,
            "partial.bin",
            payload.len() as u64,
        )
        .await?;
        let accepted = change(&receiver, &file, FileState::Accepted).await?;
        owner
            .app
            .wait_file(&accepted, CancellationToken::new())
            .await?;
        let ready = owner
            .app
            .prepare_file(
                "page".into(),
                room.clone(),
                file.id,
                staged.path(),
                CancellationToken::new(),
            )
            .await?;
        receiver
            .app
            .wait_file(&ready, CancellationToken::new())
            .await?;
        let hash = ready.ticket()?.hash();
        let _pin = receiver.app.files.store.tags().temp_tag(hash).await?;
        let connection = receiver
            .endpoint
            .connect(owner.endpoint.addr(), iroh_blobs::ALPN)
            .await?;
        receiver
            .app
            .files
            .store
            .remote()
            .execute_get(
                connection.clone(),
                GetRequest::blob_ranges(hash, ChunkRanges::chunks(0..128)),
            )
            .await?;
        connection.close(0u32.into(), b"partial download stopped");
        let present = receiver
            .app
            .files
            .store
            .remote()
            .local(hash)
            .await?
            .local_bytes();
        assert!(present > 0 && present < ready.size);
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert!(
            receiver
                .app
                .download_file("page", &room, &ready.id, cancellation)
                .await
                .is_err()
        );
        let entry = receiver
            .app
            .download_file("page", &room, &ready.id, CancellationToken::new())
            .await?;
        assert_eq!(
            receiver
                .app
                .files
                .store
                .blobs()
                .get_bytes(entry.tag.hash())
                .await?
                .as_ref(),
            payload
        );
        Ok(())
    }
    .await;
    receiver.close().await?;
    owner.close().await?;
    result
}

#[tokio::test]
async fn recipient_cancellation_revokes_an_inflight_blob_stream() -> Result<()> {
    let owner = TestApp::start().await?;
    let transport = iroh::endpoint::QuicTransportConfig::builder()
        .stream_receive_window(iroh::endpoint::VarInt::from_u32(16 * 1024))
        .receive_window(iroh::endpoint::VarInt::from_u32(64 * 1024))
        .build();
    let receiver = TestApp::new(
        Endpoint::builder(presets::Minimal)
            .transport_config(transport)
            .bind()
            .await?,
    )
    .await?;
    let result: Result<()> = async {
        let room = owner.create().await?;
        receiver.join(room.clone()).await?;
        let staged = tempfile::NamedTempFile::new()?;
        tokio::fs::write(staged.path(), vec![42; 256 * 1024]).await?;
        let file = offer(&owner, &receiver, &room, "cancel.bin", 256 * 1024).await?;
        let accepted = change(&receiver, &file, FileState::Accepted).await?;
        owner
            .app
            .wait_file(&accepted, CancellationToken::new())
            .await?;
        let ready = owner
            .app
            .prepare_file(
                "page".into(),
                room.clone(),
                file.id,
                staged.path(),
                CancellationToken::new(),
            )
            .await?;
        receiver
            .app
            .wait_file(&ready, CancellationToken::new())
            .await?;
        let connection = receiver
            .endpoint
            .connect(owner.endpoint.addr(), iroh_blobs::ALPN)
            .await?;
        let mut download = get_blob(connection.clone(), ready.ticket()?.hash());
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(5), download.next()).await?,
            Some(GetBlobItem::Item(_))
        ));
        let cancelled = change(&receiver, &ready, FileState::Cancelled).await?;
        owner
            .app
            .wait_file(&cancelled, CancellationToken::new())
            .await?;
        assert!(
            tokio::time::timeout(Duration::from_secs(5), download.bytes())
                .await?
                .is_err()
        );
        connection.close(0u32.into(), b"test finished");
        Ok(())
    }
    .await;
    receiver.close().await?;
    owner.close().await?;
    result
}

#[tokio::test]
async fn invitations_have_no_size_count_or_aggregate_quota() -> Result<()> {
    let owner = TestApp::start().await?;
    let receiver = TestApp::start().await?;
    let result: Result<()> = async {
        let room = owner.create().await?;
        receiver.join(room.clone()).await?;
        for _ in 0..20 {
            offer(
                &owner,
                &receiver,
                &room,
                "large.bin",
                12 * 1024 * 1024 * 1024,
            )
            .await?;
        }
        assert_eq!(receiver.app.snapshots.borrow().files.len(), 20);
        assert!(
            receiver
                .app
                .snapshots
                .borrow()
                .files
                .values()
                .all(
                    |file| file.size == 12 * 1024 * 1024 * 1024 && file.state == FileState::Offered
                )
        );
        let last = owner
            .app
            .snapshots
            .borrow()
            .files
            .values()
            .last()
            .cloned()
            .context("邀请不存在")?;
        let accepted = change(&receiver, &last, FileState::Accepted).await?;
        assert_eq!(
            owner
                .app
                .wait_file(&accepted, CancellationToken::new())
                .await?
                .state,
            FileState::Accepted
        );
        Ok(())
    }
    .await;
    receiver.close().await?;
    owner.close().await?;
    result
}
