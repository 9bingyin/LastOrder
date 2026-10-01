use super::*;

pub struct Peer {
    pub pc: Arc<RTCPeerConnection>,
    cancel: CancellationToken,
    negotiated: bool,
}

fn vp8() -> RTCRtpCodecCapability {
    RTCRtpCodecCapability {
        mime_type: "video/VP8".into(),
        clock_rate: 90_000,
        ..Default::default()
    }
}
fn opus() -> RTCRtpCodecCapability {
    RTCRtpCodecCapability {
        mime_type: "audio/opus".into(),
        clock_rate: 48_000,
        channels: 2,
        sdp_fmtp_line: "minptime=10;useinbandfec=1;stereo=1;maxaveragebitrate=128000".into(),
        ..Default::default()
    }
}

impl Peer {
    pub async fn new(
        id: String,
        generation: String,
        ingress: bool,
        audio: bool,
        hub: Arc<Hub>,
    ) -> Result<Self> {
        let mut engine = MediaEngine::default();
        for (payload_type, capability, kind) in [
            (VIDEO_PT, vp8(), RTPCodecType::Video),
            (AUDIO_PT, opus(), RTPCodecType::Audio),
        ] {
            engine.register_codec(
                RTCRtpCodecParameters {
                    capability,
                    payload_type,
                    ..Default::default()
                },
                kind,
            )?;
        }
        // 即使只连接本机，浏览器也需要 TWCC 反馈才能正确估计编码带宽。
        let registry = configure_twcc(
            configure_rtcp_reports(configure_nack(Registry::new(), &mut engine)),
            &mut engine,
        )?;
        let mut settings = SettingEngine::default();
        settings.set_network_types(vec![NetworkType::Udp4]);
        settings.set_include_loopback_candidate(true);
        settings.set_ip_filter(Box::new(|ip| ip.is_loopback()));
        let api = APIBuilder::new()
            .with_media_engine(engine)
            .with_interceptor_registry(registry)
            .with_setting_engine(settings)
            .build();
        let pc = Arc::new(api.new_peer_connection(RTCConfiguration::default()).await?);
        let cancel = hub.shutdown.child_token();
        let bytes = packet::generation_bytes(&generation)?;
        if ingress {
            let weak = Arc::downgrade(&pc);
            let token = cancel.clone();
            pc.on_track(Box::new(move |track, _, _| {
                let hub = hub.clone();
                let weak = weak.clone();
                let generation = generation.clone();
                let id = id.clone();
                let token = token.clone();
                Box::pin(async move {
                    let codec = track.codec().capability.mime_type;
                    let payload_type = if codec.eq_ignore_ascii_case("video/VP8") { VIDEO_PT }
                        else if audio && codec.eq_ignore_ascii_case("audio/opus") { AUDIO_PT }
                        else { return; };
                    if payload_type == VIDEO_PT && let Ok(mut source) = hub.source.lock() {
                        *source = Some(Source { generation: generation.clone(), pc: weak, ssrc: track.ssrc(), last_pli: Instant::now() - Duration::from_secs(1) });
                    }
                    let worker_hub = hub.clone();
                    hub.tasks.spawn(async move {
                        if payload_type == VIDEO_PT { let _ = worker_hub.events.send(MediaEvent::Ready(id.clone())).await; }
                        let mut pli = tokio::time::interval(Duration::from_secs(2));
                        loop {
                            tokio::select! {
                                _ = token.cancelled() => break,
                                _ = pli.tick(), if payload_type == VIDEO_PT => worker_hub.keyframe(&generation).await,
                                result = track.read_rtp() => match result {
                                    Ok((mut packet, _)) => {
                                        packet.header.extension = false;
                                        packet.header.extension_profile = 0;
                                        packet.header.extensions.clear();
                                        packet.header.payload_type = payload_type;
                                        let _ = worker_hub.packets.send(MediaPacket { generation: bytes, packet, created: Instant::now() });
                                    }
                                    Err(error) => {
                                        if payload_type == VIDEO_PT { let _ = worker_hub.events.send(MediaEvent::Failed { peer_id: id, message: format!("屏幕捕获连接已结束: {error}") }).await; }
                                        else { tracing::debug!(%error, "音频捕获已结束"); }
                                        break;
                                    }
                                }
                            }
                        }
                    });
                })
            }));
        } else {
            let mut tracks = vec![(VIDEO_PT, vp8(), "screen")];
            if audio {
                tracks.push((AUDIO_PT, opus(), "audio"));
            }
            for (payload_type, codec, track_id) in tracks {
                let track = Arc::new(TrackLocalStaticRTP::new(
                    codec,
                    track_id.into(),
                    "lastorder".into(),
                ));
                let sender = pc.add_track(track.clone()).await?;
                let mut packets = hub.packets.subscribe();
                let token = cancel.clone();
                let events = hub.events.clone();
                let generation_copy = generation.clone();
                hub.tasks.spawn(async move {
                    loop {
                        tokio::select! {
                            _ = token.cancelled() => break,
                            result = packets.recv() => match result {
                                Ok(packet) if packet.generation == bytes && packet.packet.header.payload_type == payload_type && packet.created.elapsed() < Duration::from_millis(150) => {
                                    let result = tokio::time::timeout(Duration::from_millis(100), track.write_rtp(&packet.packet)).await;
                                    if payload_type == VIDEO_PT && !matches!(result, Ok(Ok(_))) { let _ = events.try_send(MediaEvent::Keyframe(generation_copy.clone())); }
                                }
                                Err(broadcast::error::RecvError::Lagged(_)) if payload_type == VIDEO_PT => { let _ = events.try_send(MediaEvent::Keyframe(generation_copy.clone())); }
                                Err(broadcast::error::RecvError::Closed) => break,
                                _ => {}
                            }
                        }
                    }
                });
                let token = cancel.clone();
                let events = hub.events.clone();
                let generation = generation.clone();
                hub.tasks.spawn(async move {
                    let mut last = Instant::now() - Duration::from_secs(1);
                    loop {
                        tokio::select! {
                            _ = token.cancelled() => break,
                            result = sender.read_rtcp() => match result {
                                Ok((packets, _)) => {
                                    if payload_type == VIDEO_PT && packets.iter().any(|packet| packet.as_any().is::<PictureLossIndication>()) && last.elapsed() > Duration::from_millis(300) {
                                        last = Instant::now();
                                        let _ = events.try_send(MediaEvent::Keyframe(generation.clone()));
                                    }
                                }
                                Err(error) => { tracing::debug!(%error, "本机播放反馈结束"); break; }
                            }
                        }
                    }
                });
            }
        }
        Ok(Self {
            pc,
            cancel,
            negotiated: false,
        })
    }

    pub async fn answer(&mut self, sdp: String) -> Result<RTCSessionDescription> {
        if self.negotiated {
            bail!("此媒体连接已经协商，请重新创建");
        }
        if sdp.len() > 48 * 1024 || !sdp.to_lowercase().contains("vp8/90000") {
            bail!("仅支持 VP8 视频与 Opus 音频");
        }
        let result = tokio::time::timeout(Duration::from_secs(10), async {
            self.pc
                .set_remote_description(RTCSessionDescription::offer(sdp)?)
                .await?;
            let mut complete = self.pc.gathering_complete_promise().await;
            self.pc
                .set_local_description(self.pc.create_answer(None).await?)
                .await?;
            complete.recv().await;
            self.pc
                .local_description()
                .await
                .context("媒体连接没有本地描述")
        })
        .await
        .context("本机媒体协商超时")?;
        if result.is_ok() {
            self.negotiated = true;
        }
        result
    }

    pub async fn close(self) {
        self.cancel.cancel();
        if let Err(error) = self.pc.close().await {
            tracing::debug!(%error, "关闭本机媒体连接失败");
        }
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
