use super::*;

impl Actor {
    pub(super) async fn media_event(&mut self, event: MediaEvent) {
        match event {
            MediaEvent::Ready(id) => {
                if let Some(capture) = self
                    .state
                    .capture
                    .as_mut()
                    .filter(|capture| capture.rtc_peer_id == id)
                {
                    capture.state = "live".into();
                    if let Some(peer) = self.rtc.get_mut(&id) {
                        peer.deadline = None;
                    }
                    let share_id = capture.share_id.clone();
                    let generation = capture.generation.clone();
                    if self.owner() {
                        if let Some(share) = self
                            .state
                            .room
                            .as_mut()
                            .and_then(|room| room.share.as_mut())
                        {
                            share.state = ShareState::Live;
                        }
                        self.share_deadline = None;
                        self.broadcast();
                    } else {
                        let _ = self.send_upstream(Wire::ShareReady {
                            share_id,
                            generation,
                        });
                    }
                }
            }
            MediaEvent::Failed { peer_id, message } => {
                if self.rtc.contains_key(&peer_id) {
                    self.release_peer(&peer_id).await;
                    self.state.error = Some(message);
                }
            }
            MediaEvent::Keyframe(generation) => {
                self.request_keyframe(&generation).await;
            }
            MediaEvent::Budget {
                peer,
                session,
                generation,
                bitrate,
            } => {
                let remote = if self.owner() {
                    self.peers.get_mut(&peer).filter(|remote| {
                        remote.session == session
                            && remote.subscription.borrow().as_ref() == Some(&generation)
                    })
                } else {
                    self.upstream.as_mut().filter(|remote| {
                        remote.session == session
                            && remote.connection.remote_id().to_string() == peer
                            && remote.publishing.borrow().as_ref() == Some(&generation)
                    })
                };
                if let Some(remote) = remote {
                    remote.budget = Some(LinkBudget {
                        generation,
                        bitrate,
                        updated: Instant::now(),
                    });
                }
            }
        }
    }

    pub(super) async fn release_peer(&mut self, id: &str) {
        if self
            .state
            .capture
            .as_ref()
            .is_some_and(|media| media.rtc_peer_id == id)
        {
            self.stop_share().await;
        }
        if self
            .state
            .subscription
            .as_ref()
            .is_some_and(|media| media.rtc_peer_id == id)
        {
            self.unsubscribe().await;
        }
    }

    pub(super) async fn request_keyframe(&self, generation: &str) {
        let Some(share) = self
            .state
            .room
            .as_ref()
            .and_then(|room| room.share.as_ref())
            .filter(|share| share.generation == generation)
        else {
            return;
        };
        if share.publisher_id == self.state.endpoint_id {
            self.hub.keyframe(generation).await;
        } else if self.owner() {
            if let Some(peer) = self.peers.get(&share.publisher_id) {
                let _ = peer.sender.try_send(Wire::Keyframe {
                    generation: generation.into(),
                });
            }
        } else if let Some(upstream) = &self.upstream {
            let _ = upstream.sender.try_send(Wire::Keyframe {
                generation: generation.into(),
            });
        }
    }

    pub(super) async fn stop_share(&mut self) {
        let Some(capture) = self.state.capture.take() else {
            return;
        };
        if let Some(peer) = self.rtc.remove(&capture.rtc_peer_id) {
            peer.peer.close().await;
        }
        if self.owner() {
            self.end_share().await;
        } else if let Some(upstream) = &self.upstream {
            upstream.publishing.send_replace(None);
            let _ = self.send_upstream(Wire::StopShare {
                share_id: capture.share_id,
                generation: capture.generation,
            });
        }
    }

    pub(super) async fn end_share(&mut self) {
        self.share_deadline = None;
        self.unsubscribe().await;
        for peer in self.peers.values_mut() {
            peer.subscription.send_replace(None);
            peer.publishing.send_replace(None);
            peer.subscription_id = None;
        }
        if let Some(room) = self.state.room.as_mut() {
            room.share = None;
        }
        self.broadcast();
    }

    pub(super) async fn unsubscribe(&mut self) {
        if let Some(subscription) = self.state.subscription.take() {
            if let Some(upstream) = &self.upstream {
                upstream.subscription.send_replace(None);
                let _ = self.send_upstream(Wire::Unsubscribe {
                    subscription_id: subscription.id,
                });
            }
            if let Some(peer) = self.rtc.remove(&subscription.rtc_peer_id) {
                peer.peer.close().await;
            }
        }
    }

    pub(super) async fn leave(&mut self) {
        self.state.files.clear();
        self.state.file_clients.clear();
        self.state.file_errors.clear();
        self.handle.files.clear();
        if let Some(publisher) = self.publisher.take() {
            publisher.cancel();
        }
        #[cfg(test)]
        if let Some(code) = &self.code {
            self.discovery.forget(code);
        }
        self.stop_share().await;
        self.unsubscribe().await;
        if self.owner() {
            self.end_share().await;
        }
        for (_, peer) in self.peers.drain() {
            let _ = peer.sender.try_send(Wire::Closed);
            peer.connection
                .close(crate::protocol::ROOM_CLOSED.into(), b"room closed");
        }
        if let Some(upstream) = self.upstream.take() {
            upstream.connection.close(0u32.into(), b"member left");
        }
        self.code = None;
        self.state.room = None;
        self.state.join_code = None;
        self.state.connection = None;
        self.state.network_bitrate = None;
        self.downstream_budget = None;
        self.last_budget_sent = None;
        self.emit();
    }

    pub(super) async fn tick(&mut self) {
        if self
            .share_deadline
            .is_some_and(|deadline| deadline <= Instant::now())
        {
            self.stop_share().await;
            self.end_share().await;
            self.state.error = Some("媒体启动超时，请重试".into());
        }
        let expired: Vec<_> = self
            .rtc
            .iter()
            .filter(|(_, peer)| {
                peer.deadline
                    .is_some_and(|deadline| deadline <= Instant::now())
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            self.release_peer(&id).await;
            self.state.error = Some("媒体启动超时，请重试".into());
        }
        self.state.connection = self.upstream.as_ref().map(|peer| {
            let paths = peer.connection.paths();
            let selected = paths.iter().find(|path| path.is_selected());
            let path = selected
                .as_ref()
                .map(|path| {
                    if path.is_ip() {
                        "direct"
                    } else if path.is_relay() {
                        "relay"
                    } else {
                        "unknown"
                    }
                })
                .unwrap_or("down");
            let rtt_ms = selected
                .and_then(|path| peer.connection.rtt(path.id()))
                .map(|rtt| u64::try_from(rtt.as_millis()).unwrap_or(u64::MAX))
                .unwrap_or(0);
            let stats = peer.connection.stats();
            LinkInfo {
                path: path.into(),
                rtt_ms,
                sent_bytes: stats.udp_tx.bytes,
                received_bytes: stats.udp_rx.bytes,
            }
        });
        self.emit();
    }
}
