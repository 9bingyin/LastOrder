use super::*;

impl Actor {
    pub(super) async fn attach(
        &mut self,
        connection: Connection,
        session: String,
        hello: Wire,
        sender: mpsc::Sender<Wire>,
        subscription: watch::Sender<Option<String>>,
        publishing: watch::Sender<Option<String>>,
    ) -> Result<Room> {
        self.require_owner()?;
        let Wire::Join {
            version,
            room_id,
            capability,
            name,
        } = hello
        else {
            bail!("加入握手无效");
        };
        let code = self.code.as_ref().context("房间不存在")?;
        if version != crate::protocol::VERSION
            || room_id != code.id()
            || !secret_matches(&capability, &code.capability())
        {
            bail!("加入凭证无效");
        }
        if connection.max_datagram_size().is_none() {
            bail!("对端不支持实时媒体 Datagram");
        }
        let name = validate_name(&name)?;
        let id = connection.remote_id().to_string();
        if self.peers.contains_key(&id)
            && self
                .state
                .room
                .as_ref()
                .and_then(|room| room.share.as_ref())
                .is_some_and(|share| share.publisher_id == id)
        {
            self.end_share().await;
        }
        self.cancel_peer_files(&id);
        let room = self.state.room.as_mut().context("房间不存在")?;
        room.members.insert(
            id.clone(),
            Member {
                id: id.clone(),
                name,
            },
        );
        if let Some(old) = self.peers.insert(
            id,
            RemotePeer {
                connection,
                session,
                sender,
                subscription,
                subscription_id: None,
                publishing,
                budget: None,
            },
        ) {
            old.connection.close(0u32.into(), b"session replaced");
        }
        self.broadcast();
        self.state.room.clone().context("房间不存在")
    }

    pub(super) async fn network(&mut self, event: NetworkEvent) {
        match event {
            NetworkEvent::DiscoveryUpdated { room_id, error } => {
                if self.owner() && self.state.join_code.as_deref() == Some(room_id.as_str()) {
                    self.state.discovery_error = error;
                }
            }
            NetworkEvent::Detached {
                peer,
                session,
                reason,
            } => {
                if self
                    .peers
                    .get(&peer)
                    .is_some_and(|entry| entry.session == session)
                {
                    self.peers.remove(&peer);
                    self.cancel_peer_files(&peer);
                    if self
                        .state
                        .room
                        .as_ref()
                        .and_then(|room| room.share.as_ref())
                        .is_some_and(|share| share.publisher_id == peer)
                    {
                        self.end_share().await;
                    }
                    if let Some(room) = self.state.room.as_mut() {
                        room.members.remove(&peer);
                    }
                    self.broadcast();
                } else if self
                    .upstream
                    .as_ref()
                    .is_some_and(|entry| entry.session == session)
                {
                    self.leave().await;
                    self.state.error = Some(
                        match reason {
                            DisconnectReason::RoomClosed => "房间已结束",
                            DisconnectReason::ConnectionLost => "房间连接已断开，请重新加入",
                        }
                        .into(),
                    );
                }
            }
            NetworkEvent::Message {
                peer,
                session,
                message,
            } => {
                if self.owner() {
                    if !self
                        .peers
                        .get(&peer)
                        .is_some_and(|entry| entry.session == session)
                    {
                        return;
                    }
                    match message {
                        Wire::StartShare {
                            share_id,
                            generation,
                            profile,
                            audio,
                        } => {
                            let valid = crate::protocol::valid_id(&share_id)
                                && crate::protocol::valid_id(&generation)
                                && profile.validate().is_ok();
                            let existing = self
                                .state
                                .room
                                .as_ref()
                                .and_then(|room| room.share.as_ref());
                            let same = existing.is_some_and(|share| {
                                share.publisher_id == peer
                                    && share.id == share_id
                                    && share.generation == generation
                            });
                            if valid && (existing.is_none() || same) {
                                if !same {
                                    if let Some(room) = self.state.room.as_mut() {
                                        room.share = Some(Share {
                                            id: share_id.clone(),
                                            generation: generation.clone(),
                                            publisher_id: peer.clone(),
                                            state: ShareState::Preparing,
                                            profile,
                                            audio,
                                        });
                                    }
                                    self.share_deadline =
                                        Some(Instant::now() + Duration::from_secs(20));
                                    self.broadcast();
                                }
                                if let Some(remote) = self.peers.get(&peer) {
                                    remote.publishing.send_replace(Some(generation.clone()));
                                    if remote
                                        .sender
                                        .try_send(Wire::ShareGranted {
                                            share_id,
                                            generation,
                                        })
                                        .is_err()
                                    {
                                        remote.connection.close(0u32.into(), b"control queue full");
                                    }
                                }
                            } else if let Some(remote) = self.peers.get(&peer)
                                && remote
                                    .sender
                                    .try_send(Wire::ShareRejected {
                                        share_id,
                                        message: if valid {
                                            "房间已有活动分享"
                                        } else {
                                            "分享参数无效"
                                        }
                                        .into(),
                                    })
                                    .is_err()
                            {
                                remote.connection.close(0u32.into(), b"control queue full");
                            }
                        }
                        Wire::ShareReady {
                            share_id,
                            generation,
                        } => {
                            if let Some(share) = self
                                .state
                                .room
                                .as_mut()
                                .and_then(|room| room.share.as_mut())
                                .filter(|share| {
                                    share.publisher_id == peer
                                        && share.id == share_id
                                        && share.generation == generation
                                })
                            {
                                share.state = ShareState::Live;
                                self.share_deadline = None;
                                self.broadcast();
                            }
                        }
                        Wire::StopShare {
                            share_id,
                            generation,
                        } => {
                            if self
                                .state
                                .room
                                .as_ref()
                                .and_then(|room| room.share.as_ref())
                                .is_some_and(|share| {
                                    share.publisher_id == peer
                                        && share.id == share_id
                                        && share.generation == generation
                                })
                            {
                                self.end_share().await;
                            }
                        }
                        Wire::UpdateShare {
                            share_id,
                            generation,
                            profile,
                        } => {
                            if profile.validate().is_ok()
                                && let Some(share) = self
                                    .state
                                    .room
                                    .as_mut()
                                    .and_then(|room| room.share.as_mut())
                                    .filter(|share| {
                                        share.publisher_id == peer
                                            && share.id == share_id
                                            && share.generation == generation
                                    })
                            {
                                share.profile = profile;
                                self.broadcast();
                            }
                        }
                        Wire::Subscribe {
                            share_id,
                            generation,
                            subscription_id,
                        } => {
                            let valid = crate::protocol::valid_id(&subscription_id)
                                && self
                                    .state
                                    .room
                                    .as_ref()
                                    .and_then(|room| room.share.as_ref())
                                    .is_some_and(|share| {
                                        share.id == share_id
                                            && share.generation == generation
                                            && share.state == ShareState::Live
                                            && share.publisher_id != peer
                                    });
                            if let Some(remote) = self.peers.get_mut(&peer) {
                                if valid {
                                    remote.subscription_id = Some(subscription_id.clone());
                                    remote.subscription.send_replace(Some(generation.clone()));
                                    if remote
                                        .sender
                                        .try_send(Wire::Subscribed {
                                            share_id,
                                            generation: generation.clone(),
                                            subscription_id,
                                        })
                                        .is_err()
                                    {
                                        remote.connection.close(0u32.into(), b"control queue full");
                                    }
                                    self.request_keyframe(&generation).await;
                                } else {
                                    if remote
                                        .sender
                                        .try_send(Wire::SubscriptionRejected {
                                            subscription_id,
                                            message: "分享已结束或变化".into(),
                                        })
                                        .is_err()
                                    {
                                        remote.connection.close(0u32.into(), b"control queue full");
                                    }
                                }
                            }
                        }
                        Wire::Unsubscribe { subscription_id } => {
                            if let Some(remote) = self.peers.get_mut(&peer)
                                && remote.subscription_id.as_ref() == Some(&subscription_id)
                            {
                                remote.subscription.send_replace(None);
                                remote.subscription_id = None;
                            }
                        }
                        Wire::Keyframe { generation } => {
                            if self.peers.get(&peer).is_some_and(|remote| {
                                remote.subscription.borrow().as_ref() == Some(&generation)
                            }) {
                                self.request_keyframe(&generation).await;
                            }
                        }
                        Wire::OfferFile { file } | Wire::ReadyFile { file } => {
                            let id = file.id.clone();
                            let result = if file.state == FileState::Offered {
                                self.add_file(&peer, file)
                            } else {
                                self.apply_ready(&peer, file)
                            };
                            if let Err(error) = result
                                && let Some(remote) = self.peers.get(&peer)
                                && remote
                                    .sender
                                    .try_send(Wire::FileRejected {
                                        current: self
                                            .state
                                            .files
                                            .get(&id)
                                            .filter(|file| {
                                                file.publisher_id == peer
                                                    || file.recipient_id == peer
                                            })
                                            .cloned()
                                            .map(Box::new),
                                        file_id: id,
                                        message: error.to_string(),
                                    })
                                    .is_err()
                            {
                                remote.connection.close(0u32.into(), b"control queue full");
                            }
                        }
                        Wire::ChangeFile { file_id, state } => {
                            let room_id = self
                                .state
                                .room
                                .as_ref()
                                .map(|room| room.id.clone())
                                .unwrap_or_default();
                            if let Err(error) = self.apply_change(&peer, &room_id, &file_id, state)
                                && let Some(remote) = self.peers.get(&peer)
                                && remote
                                    .sender
                                    .try_send(Wire::FileRejected {
                                        current: self
                                            .state
                                            .files
                                            .get(&file_id)
                                            .filter(|file| {
                                                file.publisher_id == peer
                                                    || file.recipient_id == peer
                                            })
                                            .cloned()
                                            .map(Box::new),
                                        file_id,
                                        message: error.to_string(),
                                    })
                                    .is_err()
                            {
                                remote.connection.close(0u32.into(), b"control queue full");
                            }
                        }
                        Wire::Leave => {
                            if let Some(remote) = self.peers.get(&peer) {
                                remote.connection.close(0u32.into(), b"member left");
                            }
                        }
                        _ => {
                            if let Some(remote) = self.peers.get(&peer) {
                                remote.connection.close(0u32.into(), b"invalid message");
                            }
                        }
                    }
                } else if self
                    .upstream
                    .as_ref()
                    .is_some_and(|entry| entry.session == session)
                {
                    match message {
                        Wire::Snapshot { room } => {
                            if self.state.room.as_ref().is_some_and(|old| {
                                room.id == old.id
                                    && room.owner_id == old.owner_id
                                    && room.revision >= old.revision
                            }) {
                                if self.state.subscription.as_ref().is_some_and(|sub| {
                                    !room.share.as_ref().is_some_and(|share| {
                                        share.id == sub.share_id
                                            && share.generation == sub.generation
                                    })
                                }) {
                                    self.unsubscribe().await;
                                }
                                let lost_capture =
                                    self.state.capture.as_ref().is_some_and(|capture| {
                                        capture.state != "requesting"
                                            && !room.share.as_ref().is_some_and(|share| {
                                                share.id == capture.share_id
                                                    && share.generation == capture.generation
                                            })
                                    });
                                self.state.room = Some(room);
                                if lost_capture {
                                    self.stop_share().await;
                                }
                            }
                        }
                        Wire::ShareGranted {
                            share_id,
                            generation,
                        } => {
                            if let Some(capture) = self.state.capture.as_mut().filter(|capture| {
                                capture.share_id == share_id && capture.generation == generation
                            }) {
                                if capture.state == "requesting" {
                                    capture.state = "preparing".into();
                                }
                                if let Some(upstream) = &self.upstream {
                                    upstream.publishing.send_replace(Some(generation));
                                }
                                for receipt in &mut self.receipts {
                                    if receipt.value.get("shareId").and_then(Value::as_str)
                                        == Some(&share_id)
                                    {
                                        receipt.value["state"] = json!("preparing");
                                    }
                                }
                            } else {
                                let _ = self.send_upstream(Wire::StopShare {
                                    share_id,
                                    generation,
                                });
                            }
                        }
                        Wire::ShareRejected { share_id, message } => {
                            if self
                                .state
                                .capture
                                .as_ref()
                                .is_some_and(|capture| capture.share_id == share_id)
                            {
                                self.stop_share().await;
                                self.receipts.retain(|receipt| {
                                    receipt.value.get("shareId").and_then(Value::as_str)
                                        != Some(&share_id)
                                });
                                self.state.error = Some(message);
                            }
                        }
                        Wire::NetworkBudget {
                            generation,
                            bitrate,
                        } => {
                            if (net::budget::MIN_BITRATE..=net::budget::MAX_BITRATE)
                                .contains(&bitrate)
                                && self
                                    .state
                                    .room
                                    .as_ref()
                                    .and_then(|room| room.share.as_ref())
                                    .is_some_and(|share| {
                                        share.generation == generation
                                            && share.publisher_id == self.state.endpoint_id
                                            && share.profile.mode == QualityMode::Auto
                                    })
                                && self
                                    .state
                                    .capture
                                    .as_ref()
                                    .is_some_and(|capture| capture.generation == generation)
                            {
                                self.downstream_budget = Some(LinkBudget {
                                    generation,
                                    bitrate,
                                    updated: Instant::now(),
                                });
                            }
                        }
                        Wire::Keyframe { generation } => {
                            if self
                                .state
                                .capture
                                .as_ref()
                                .is_some_and(|capture| capture.generation == generation)
                            {
                                self.hub.keyframe(&generation).await;
                            }
                        }
                        Wire::Subscribed {
                            share_id,
                            generation,
                            subscription_id,
                        } => {
                            if self.state.subscription.as_ref().is_some_and(|sub| {
                                sub.id == subscription_id
                                    && sub.share_id == share_id
                                    && sub.generation == generation
                            }) && let Some(upstream) = self.upstream.as_ref()
                            {
                                upstream.subscription.send_replace(Some(generation));
                            }
                        }
                        Wire::Closed => {
                            self.leave().await;
                            self.state.error = Some("房间已结束".into());
                        }
                        Wire::SubscriptionRejected {
                            subscription_id,
                            message,
                        } => {
                            if self
                                .state
                                .subscription
                                .as_ref()
                                .is_some_and(|sub| sub.id == subscription_id)
                            {
                                self.unsubscribe().await;
                                self.state.error = Some(message);
                            }
                        }
                        Wire::FileTransfer { file } => {
                            if file.validate().is_ok()
                                && self
                                    .state
                                    .room
                                    .as_ref()
                                    .is_some_and(|room| room.id == file.room_id)
                                && (file.publisher_id == self.state.endpoint_id
                                    || file.recipient_id == self.state.endpoint_id)
                                && self
                                    .state
                                    .files
                                    .get(&file.id)
                                    .map_or(file.state == FileState::Offered, |previous| {
                                        previous.accepts_update(&file)
                                    })
                            {
                                self.state.files.insert(file.id.clone(), file);
                            } else if let Some(upstream) = &self.upstream {
                                upstream
                                    .connection
                                    .close(0u32.into(), b"invalid file invitation");
                            }
                        }
                        Wire::FileRejected {
                            file_id,
                            message,
                            current,
                        } => {
                            self.handle.files.remove(&file_id);
                            self.state.file_errors.insert(file_id.clone(), message);
                            if let Some(file) = current.filter(|file| {
                                file.id == file_id
                                    && file.validate().is_ok()
                                    && self
                                        .state
                                        .files
                                        .get(&file_id)
                                        .is_none_or(|previous| previous.same_offer(file))
                                    && (file.publisher_id == self.state.endpoint_id
                                        || file.recipient_id == self.state.endpoint_id)
                            }) {
                                self.state.files.insert(file_id, *file);
                            } else if !self.state.files.contains_key(&file_id) {
                                self.state.file_clients.remove(&file_id);
                            }
                        }
                        Wire::Error { message } => {
                            self.state.error = Some(message);
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}
