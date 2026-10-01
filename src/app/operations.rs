use super::*;

impl Actor {
    pub(super) async fn local(&mut self, client: &str, operation: Operation) -> Result<Value> {
        match operation {
            Operation::Create { name } => {
                self.require_idle()?;
                let name = validate_name(&name)?;
                let code = RoomCode::generate()?;
                self.discovery
                    .publish(&code, &self.endpoint)
                    .await
                    .map_err(|error| {
                        fault(
                            "discovery_unavailable",
                            &format!("发布房间入口失败: {error}"),
                            502,
                        )
                    })?;
                let id = self.endpoint.id().to_string();
                let room = Room {
                    id: code.id(),
                    owner_id: id.clone(),
                    revision: 1,
                    members: [(id.clone(), Member { id, name })].into(),
                    share: None,
                };
                self.state.join_code = Some(code.id());
                let publisher = self.shutdown.child_token();
                self.tasks.spawn(self.discovery.clone().refresh(
                    code.clone(),
                    self.endpoint.clone(),
                    publisher.clone(),
                ));
                self.publisher = Some(publisher);
                self.code = Some(code);
                self.state.room = Some(room.clone());
                self.state.error = None;
                Ok(json!({"room": room, "joinCode": self.state.join_code}))
            }
            Operation::Join { code, name } => {
                self.require_idle()?;
                let name = validate_name(&name)?;
                let target = JoinTarget::parse(&code).map_err(|_| {
                    fault(
                        "invalid_join_code",
                        "加入信息须为随机 UUID 或 LastOrder Ticket",
                        400,
                    )
                })?;
                let (code, address) = match target {
                    JoinTarget::Uuid(code) => {
                        let address = self.discovery.resolve(&code).await.map_err(|error| {
                            fault("room_unreachable", &format!("查找房间失败: {error}"), 502)
                        })?;
                        (code, address)
                    }
                    JoinTarget::Ticket { code, address } => (code, address),
                };
                if address.id == self.endpoint.id() {
                    return Err(fault("room_conflict", "不能加入本机已经结束的房间", 409));
                }
                let joined = net::join(&self.endpoint, &code, address, name)
                    .await
                    .map_err(|error| {
                        fault("room_unreachable", &format!("加入失败: {error}"), 502)
                    })?;
                self.state.room = Some(joined.room);
                self.state.join_code = Some(code.id());
                self.state.error = None;
                self.upstream = Some(RemotePeer {
                    connection: joined.connection.clone(),
                    session: joined.session.clone(),
                    sender: joined.sender,
                    subscription: joined.subscription,
                    subscription_id: None,
                    publishing: joined.publishing,
                    budget: None,
                });
                let app = self.handle.clone();
                self.tasks.spawn(net::serve(
                    app,
                    joined.connection,
                    joined.session,
                    joined.transport,
                    false,
                ));
                Ok(json!({"room": self.state.room}))
            }
            Operation::Leave => {
                self.leave().await;
                Ok(json!({}))
            }
            Operation::StartShare { profile, audio } => {
                let profile = profile
                    .validate()
                    .map_err(|_| fault("invalid_profile", "画质配置无效", 400))?;
                if self.state.capture.is_some() {
                    return Err(fault("share_conflict", "本机已有分享申请", 409));
                }
                let room = self.state.room.as_ref().context("未加入房间")?;
                if room.share.is_some() {
                    return Err(fault("share_conflict", "房间已有活动分享", 409));
                }
                let media = LocalMedia {
                    audio,
                    id: random_id()?,
                    share_id: random_id()?,
                    generation: random_id()?,
                    rtc_peer_id: random_id()?,
                    client_id: client.into(),
                    state: if self.owner() {
                        "preparing"
                    } else {
                        "requesting"
                    }
                    .into(),
                };
                let peer = Peer::new(
                    media.rtc_peer_id.clone(),
                    media.generation.clone(),
                    true,
                    audio,
                    self.hub.clone(),
                )
                .await?;
                self.rtc.insert(
                    media.rtc_peer_id.clone(),
                    LocalPeer {
                        peer,
                        client: client.into(),
                        deadline: Some(Instant::now() + Duration::from_secs(20)),
                    },
                );
                if self.owner() {
                    if let Some(room) = self.state.room.as_mut() {
                        room.share = Some(Share {
                            id: media.share_id.clone(),
                            generation: media.generation.clone(),
                            publisher_id: self.state.endpoint_id.clone(),
                            state: ShareState::Preparing,
                            profile,
                            audio,
                        });
                    }
                    self.share_deadline = Some(Instant::now() + Duration::from_secs(20));
                    self.broadcast();
                } else if self.upstream.as_ref().is_none_or(|upstream| {
                    upstream
                        .sender
                        .try_send(Wire::StartShare {
                            share_id: media.share_id.clone(),
                            generation: media.generation.clone(),
                            profile,
                            audio,
                        })
                        .is_err()
                }) {
                    if let Some(local) = self.rtc.remove(&media.rtc_peer_id) {
                        local.peer.close().await;
                    }
                    bail!("房间控制连接不可用");
                }
                self.state.capture = Some(media.clone());
                self.state.error = None;
                Ok(serde_json::to_value(media)?)
            }
            Operation::UpdateShare { id, profile } => {
                let profile = profile
                    .validate()
                    .map_err(|_| fault("invalid_profile", "画质配置无效", 400))?;
                let capture = self
                    .state
                    .capture
                    .as_ref()
                    .filter(|capture| capture.share_id == id && capture.client_id == client)
                    .ok_or_else(|| fault("permission_denied", "分享不属于此页面", 403))?;
                if self.owner() {
                    if let Some(share) = self
                        .state
                        .room
                        .as_mut()
                        .and_then(|room| room.share.as_mut())
                    {
                        share.profile = profile;
                    }
                    self.broadcast();
                } else {
                    self.upstream
                        .as_ref()
                        .context("房间已断开")?
                        .sender
                        .try_send(Wire::UpdateShare {
                            share_id: id,
                            generation: capture.generation.clone(),
                            profile,
                        })
                        .context("房间控制连接不可用")?;
                }
                Ok(json!({}))
            }
            Operation::StopShare { id } => {
                if let Some(capture) = &self.state.capture
                    && capture.share_id == id
                {
                    if capture.client_id != client {
                        return Err(fault("permission_denied", "分享属于另一个页面", 403));
                    }
                    self.stop_share().await;
                }
                Ok(json!({}))
            }
            Operation::Subscribe {
                share_id,
                generation,
            } => {
                if self.state.subscription.is_some() {
                    return Err(fault("subscription_conflict", "请先停止当前观看", 409));
                }
                let share = self
                    .state
                    .room
                    .as_ref()
                    .and_then(|room| room.share.as_ref())
                    .context("房间没有活动分享")?;
                if share.id != share_id
                    || share.generation != generation
                    || share.state != ShareState::Live
                {
                    return Err(fault("stale_generation", "分享已变化，请刷新状态", 409));
                }
                if share.publisher_id == self.state.endpoint_id {
                    return Err(fault("subscription_conflict", "请使用本机分享预览", 409));
                }
                let audio = share.audio;
                let media = LocalMedia {
                    audio,
                    id: random_id()?,
                    share_id,
                    generation,
                    rtc_peer_id: random_id()?,
                    client_id: client.into(),
                    state: "preparing".into(),
                };
                let peer = Peer::new(
                    media.rtc_peer_id.clone(),
                    media.generation.clone(),
                    false,
                    audio,
                    self.hub.clone(),
                )
                .await?;
                if let Some(upstream) = &self.upstream
                    && upstream
                        .sender
                        .try_send(Wire::Subscribe {
                            share_id: media.share_id.clone(),
                            generation: media.generation.clone(),
                            subscription_id: media.id.clone(),
                        })
                        .is_err()
                {
                    peer.close().await;
                    bail!("房间控制连接不可用");
                }
                self.rtc.insert(
                    media.rtc_peer_id.clone(),
                    LocalPeer {
                        peer,
                        client: client.into(),
                        deadline: Some(Instant::now() + Duration::from_secs(20)),
                    },
                );
                self.state.subscription = Some(media.clone());
                self.state.error = None;
                Ok(serde_json::to_value(media)?)
            }
            Operation::Unsubscribe { id } => {
                if let Some(subscription) = &self.state.subscription
                    && subscription.id == id
                {
                    if subscription.client_id != client {
                        return Err(fault("permission_denied", "订阅属于另一个页面", 403));
                    }
                    self.unsubscribe().await;
                }
                Ok(json!({}))
            }
            Operation::Answer { peer_id, sdp } => {
                let local = self
                    .rtc
                    .get_mut(&peer_id)
                    .ok_or_else(|| fault("peer_not_found", "媒体连接不存在", 404))?;
                if local.client != client {
                    return Err(fault("permission_denied", "媒体连接属于另一个页面", 403));
                }
                match local.peer.answer(sdp).await {
                    Ok(answer) => Ok(serde_json::to_value(answer)?),
                    Err(error) => {
                        self.release_peer(&peer_id).await;
                        Err(fault(
                            "media_negotiation_failed",
                            &format!("媒体协商失败: {error}"),
                            400,
                        ))
                    }
                }
            }
            Operation::Playing { id } => {
                let subscription = self
                    .state
                    .subscription
                    .as_mut()
                    .filter(|sub| sub.id == id && sub.client_id == client)
                    .ok_or_else(|| fault("subscription_not_found", "订阅不存在", 404))?;
                subscription.state = "playing".into();
                if let Some(peer) = self.rtc.get_mut(&subscription.rtc_peer_id) {
                    peer.deadline = None;
                }
                Ok(json!({}))
            }
            Operation::ReleaseClient => {
                self.receipts.retain(|receipt| receipt.client != client);
                if self
                    .state
                    .capture
                    .as_ref()
                    .is_some_and(|media| media.client_id == client)
                {
                    self.stop_share().await;
                }
                if self
                    .state
                    .subscription
                    .as_ref()
                    .is_some_and(|media| media.client_id == client)
                {
                    self.unsubscribe().await;
                }
                Ok(json!({}))
            }
        }
    }
}
