use super::*;

impl Actor {
    pub(super) async fn run(
        mut self,
        mut commands: mpsc::Receiver<Command>,
        mut media: mpsc::Receiver<MediaEvent>,
    ) {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                biased;
                _ = self.shutdown.cancelled() => break,
                Some(command) = commands.recv() => match command {
                    Command::Local { client, operation, cancellation, idempotency_key, reply } => {
                        if cancellation.is_cancelled() {
                            let _ = reply.send(Err(fault("client_disconnected", "页面会话已断开", 401)));
                            continue;
                        }
                        self.receipts.retain(|receipt| receipt.created.elapsed() < Duration::from_secs(60));
                        if let Some(receipt) = idempotency_key.as_ref().and_then(|key| self.receipts.iter().find(|receipt| receipt.client == client && &receipt.key == key)) {
                            let result = if receipt.operation == operation { Ok(receipt.value.clone()) } else { Err(fault("idempotency_conflict", "同一幂等键不能用于不同操作", 409)) };
                            let _ = reply.send(result);
                            continue;
                        }
                        let recorded = operation.clone();
                        let mut result = self.local(&client, operation).await;
                        if cancellation.is_cancelled() {
                            let _ = self.local(&client, Operation::ReleaseClient).await;
                            result = Err(fault("client_disconnected", "页面会话已断开", 401));
                        }
                        if let (Some(key), Ok(value)) = (idempotency_key, &result) {
                            if self.receipts.len() >= 128 { self.receipts.pop_front(); }
                            self.receipts.push_back(Receipt { client, key, operation: recorded, value: value.clone(), created: Instant::now() });
                        }
                        self.emit();
                        let _ = reply.send(result);
                    }
                    Command::Attach { connection, session, hello, sender, subscription, publishing, reply } => {
                        let peer = connection.remote_id().to_string();
                        let session_id = session.clone();
                        let result = self.attach(connection, session, hello, sender, subscription, publishing).await;
                        self.emit();
                        if reply.send(result).is_err() {
                            self.network(NetworkEvent::Detached { peer, session: session_id, reason: DisconnectReason::ConnectionLost }).await;
                            self.emit();
                        }
                    }
                    Command::Network(event) => { self.network(event).await; self.emit(); }
                    Command::Diagnostics { reply } => { let _ = reply.send(self.diagnostics()); }
                },
                Some(event) = media.recv() => { self.media_event(event).await; self.emit(); },
                _ = tick.tick() => { self.tick().await; },
                else => break,
            }
        }
        self.leave().await;
    }

    pub(super) fn emit(&mut self) {
        self.reconcile_file_transfers();
        self.handle.files.reconcile(&self.state);
        self.refresh_budget();
        self.state.join_ticket = self.ticket();
        self.state.event_seq = self.state.event_seq.saturating_add(1);
        let mut snapshot = self.state.clone();
        snapshot.files.retain(|_, file| {
            file.publisher_id == snapshot.endpoint_id || file.recipient_id == snapshot.endpoint_id
        });
        log_changes(&self.snapshots.borrow(), &snapshot);
        self.snapshots.send_replace(snapshot);
    }

    pub(super) fn owner(&self) -> bool {
        self.code.is_some()
    }

    pub(super) fn require_owner(&self) -> Result<()> {
        if !self.owner() {
            return Err(fault("permission_denied", "此节点不负责房间协调", 403));
        }
        Ok(())
    }

    pub(super) fn send_upstream(&self, message: Wire) -> Result<()> {
        let upstream = self.upstream.as_ref().context("房间已断开")?;
        if upstream.sender.try_send(message).is_err() {
            upstream
                .connection
                .close(0u32.into(), b"control queue full");
            bail!("房间控制连接不可用");
        }
        Ok(())
    }

    pub(super) fn require_idle(&self) -> Result<()> {
        if self.state.room.is_some() {
            return Err(fault("room_conflict", "请先离开当前房间", 409));
        }
        Ok(())
    }

    pub(super) fn broadcast(&mut self) {
        if let Some(room) = self.state.room.as_mut() {
            room.revision = room.revision.saturating_add(1);
            let message = Wire::Snapshot { room: room.clone() };
            for peer in self.peers.values() {
                if peer.sender.try_send(message.clone()).is_err() {
                    peer.connection.close(0u32.into(), b"control queue full");
                }
            }
        }
    }
}

fn log_changes(prev: &Snapshot, next: &Snapshot) {
    let member = |room: &Room, id: &str| {
        room.members
            .get(id)
            .map_or(id, |member| member.name.as_str())
            .to_owned()
    };
    let live = |room: &Room| {
        room.share
            .as_ref()
            .filter(|share| share.state == ShareState::Live)
            .map(|share| (share.id.clone(), share.publisher_id.clone()))
    };
    match (&prev.room, &next.room) {
        (None, Some(room)) if room.owner_id == next.endpoint_id => {
            tracing::info!("已创建房间");
        }
        (None, Some(room)) => tracing::info!("已加入房间，共 {} 人", room.members.len()),
        (Some(_), None) => tracing::info!("已离开房间"),
        (Some(old), Some(room)) if old.id == room.id => {
            for (id, joined) in &room.members {
                if !old.members.contains_key(id) {
                    tracing::info!("{} 加入了房间", joined.name);
                }
            }
            for (id, left) in &old.members {
                if !room.members.contains_key(id) {
                    tracing::info!("{} 离开了房间", left.name);
                }
            }
            let (before, after) = (live(old), live(room));
            if before != after {
                if let Some((_, publisher)) = &before {
                    tracing::info!("{} 结束了分享", member(old, publisher));
                }
                if let Some((_, publisher)) = &after {
                    tracing::info!("{} 开始分享", member(room, publisher));
                }
            }
        }
        _ => {}
    }
    match (&prev.subscription, &next.subscription) {
        (None, Some(_)) => tracing::info!("开始观看"),
        (Some(_), None) => tracing::info!("停止观看"),
        _ => {}
    }
    if let Some(link) = &next.connection
        && prev.connection.as_ref().map(|link| &link.path) != Some(&link.path)
    {
        let path = match link.path.as_str() {
            "direct" => "直连",
            "relay" => "中继",
            "down" => "已断开",
            _ => "未知",
        };
        tracing::info!("连接方式：{path}");
    }
    for (id, file) in &next.files {
        if prev.files.get(id).map(|old| old.state) == Some(file.state) {
            continue;
        }
        let direction = if file.publisher_id == next.endpoint_id {
            "发送"
        } else {
            "接收"
        };
        let state = match file.state {
            FileState::Offered => "等待对方接收",
            FileState::Accepted => "已接受",
            FileState::Ready => "传输中",
            FileState::Completed => "已完成",
            FileState::Rejected => "已拒绝",
            FileState::Cancelled => "已取消",
        };
        tracing::info!("{direction}文件 {}：{state}", file.name);
    }
    if let Some(error) = &next.error
        && prev.error.as_ref() != Some(error)
    {
        tracing::warn!("{error}");
    }
}
