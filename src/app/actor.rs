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
        self.refresh_budget();
        self.state.join_ticket = self.ticket();
        self.state.event_seq = self.state.event_seq.saturating_add(1);
        self.snapshots.send_replace(self.state.clone());
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
