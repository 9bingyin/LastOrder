use super::*;

pub(super) async fn send_media(
    connection: Connection,
    hub: Arc<Hub>,
    subscribed: &mut watch::Receiver<Option<String>>,
    shutdown: CancellationToken,
    peer: String,
    session: String,
) -> Result<()> {
    let mut packets = hub.packets.subscribe();
    let mut packet_id = 0u32;
    let mut meter = budget::Estimator::new(&connection);
    let mut sampled_generation = None;
    let mut blocked = false;
    let mut sample = tokio::time::interval(Duration::from_millis(500));
    sample.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return Ok(()),
            changed = subscribed.changed() => { changed?; },
            _ = sample.tick() => {
                let current = subscribed.borrow().clone();
                if current != sampled_generation {
                    meter.reset(&connection);
                    sampled_generation.clone_from(&current);
                    blocked = false;
                }
                if let Some(generation) = current {
                    let bitrate = meter.sample(&connection, blocked);
                    blocked = false;
                    let _ = hub.events.try_send(crate::media::MediaEvent::Budget {
                        peer: peer.clone(), session: session.clone(), generation, bitrate,
                    });
                }
            },
            result = packets.recv() => match result {
                Ok(packet) => {
                    let current = subscribed.borrow().clone();
                    let Some(generation) = current else { continue; };
                    if generation_bytes(&generation)? != packet.generation || packet.created.elapsed() > Duration::from_millis(100) { continue; }
                    let mtu = connection.max_datagram_size().context("实时媒体 Datagram 不可用")?;
                    let bytes = packet.packet.marshal()?;
                    let parts = fragment(packet.generation, packet_id, &bytes, mtu)?;
                    packet_id = packet_id.wrapping_add(1);
                    if parts.iter().map(Bytes::len).sum::<usize>() > connection.datagram_send_buffer_space() {
                        blocked = true;
                        let _ = hub.events.try_send(crate::media::MediaEvent::Keyframe(generation));
                        continue;
                    }
                    for data in parts {
                        if connection.send_datagram(data).is_err() { let _ = hub.events.try_send(crate::media::MediaEvent::Keyframe(generation.clone())); break; }
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let generation = subscribed.borrow().clone();
                    if let Some(generation) = generation { let _ = hub.events.try_send(crate::media::MediaEvent::Keyframe(generation)); }
                }
                Err(broadcast::error::RecvError::Closed) => return Ok(()),
            }
        }
    }
}

pub(super) async fn receive_media(
    connection: Connection,
    hub: Arc<Hub>,
    subscribed: &mut watch::Receiver<Option<String>>,
    shutdown: CancellationToken,
) -> Result<()> {
    let mut reassembler = Reassembler::default();
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return Ok(()),
            changed = subscribed.changed() => { changed?; reassembler = Reassembler::default(); },
            data = connection.read_datagram() => {
                let data = data?;
                let current = subscribed.borrow().clone();
                let Some(generation) = current else { continue; };
                let generation = generation_bytes(&generation)?;
                if let Some(mut data) = reassembler.push(data, generation, Instant::now())
                    && let Ok(packet) = Packet::unmarshal(&mut data)
                    && [crate::media::VIDEO_PT, crate::media::AUDIO_PT].contains(&packet.header.payload_type)
                {
                    let _ = hub.packets.send(MediaPacket { generation, packet, created: Instant::now() });
                }
            }
        }
    }
}
