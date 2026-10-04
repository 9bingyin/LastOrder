use iroh::Watcher as _;

use super::*;

impl Actor {
    pub(super) fn diagnostics(&self) -> Value {
        let report = self.endpoint.net_report().get().map(|report| {
            json!({
                "udpV4": report.udp_v4,
                "udpV6": report.udp_v6,
                "mappingVariesByDestIpv4": report.mapping_varies_by_dest_ipv4,
                "mappingVariesByDestIpv6": report.mapping_varies_by_dest_ipv6,
                "globalV4": report.global_v4,
                "globalV6": report.global_v6,
                "preferredRelay": report.preferred_relay,
                "captivePortal": report.captive_portal,
                "relayLatencies": report.relay_latency.iter().map(|(probe, relay, latency)| json!({
                    "probe": format!("{probe:?}"),
                    "relay": relay,
                    "latencyMs": latency.as_secs_f64() * 1000.0,
                })).collect::<Vec<_>>(),
            })
        });
        let metrics = self.endpoint.metrics();
        let address = self.endpoint.addr();
        let connection = |peer: &RemotePeer, role: &str| {
            let connection = &peer.connection;
            let paths = connection.paths();
            json!({
                "role": role,
                "remoteId": connection.remote_id().to_string(),
                "session": peer.session,
                "stableId": connection.stable_id(),
                "side": format!("{:?}", connection.side()),
                "alpn": String::from_utf8_lossy(connection.alpn()),
                "closeReason": connection.close_reason().map(|reason| reason.to_string()),
                "maxDatagramSize": connection.max_datagram_size(),
                "datagramSendBufferSpace": connection.datagram_send_buffer_space(),
                "controlQueueCapacity": peer.sender.capacity(),
                "subscription": peer.subscription.borrow().clone(),
                "publishing": peer.publishing.borrow().clone(),
                "budget": peer.budget.as_ref().map(|budget| json!({"generation": budget.generation, "bitrate": budget.bitrate, "ageMs": budget.updated.elapsed().as_millis()})),
                "statsRaw": format!("{:#?}", connection.stats()),
                "paths": paths.iter().map(|path| json!({
                    "id": format!("{:?}", path.id()),
                    "selected": path.is_selected(),
                    "remoteAddress": format!("{:?}", path.remote_addr()),
                    "localAddress": format!("{:?}", path.local_addr()),
                    "rttMs": connection.rtt(path.id()).map(|rtt| rtt.as_secs_f64() * 1000.0),
                    "statsRaw": format!("{:#?}", path.stats()),
                    "congestion": connection.congestion_state(path.id()).map(|state| {
                        let metrics = state.metrics();
                        json!({"congestionWindow": metrics.congestion_window, "ssthresh": metrics.ssthresh, "pacingRate": metrics.pacing_rate, "sendQuantum": metrics.send_quantum})
                    }),
                })).collect::<Vec<_>>()
            })
        };
        let mut connections: Vec<_> = self
            .peers
            .values()
            .map(|peer| connection(peer, "member"))
            .collect();
        connections.sort_by(|a, b| a["remoteId"].as_str().cmp(&b["remoteId"].as_str()));
        if let Some(upstream) = &self.upstream {
            connections.insert(0, connection(upstream, "coordinator"));
        }
        let mut snapshot = self.state.clone();
        if let Some(code) = &snapshot.join_code {
            if let Some(room) = snapshot.room.as_mut()
                && room.id == *code
            {
                room.id = "[redacted]".into();
            }
            snapshot.join_code = Some("[redacted]".into());
        }
        if snapshot.join_ticket.is_some() {
            snapshot.join_ticket = Some("[redacted]".into());
        }
        snapshot.files.retain(|_, file| {
            file.publisher_id == snapshot.endpoint_id || file.recipient_id == snapshot.endpoint_id
        });
        for file in snapshot.files.values_mut() {
            file.room_id = "[redacted]".into();
            if file.blob_ticket.is_some() {
                file.blob_ticket = Some("[redacted]".into());
            }
        }
        json!({
            "version": env!("CARGO_PKG_VERSION"),
            "snapshot": snapshot,
            "endpointAddressRaw": format!("{:#?}", self.endpoint.addr()),
            "boundSockets": self.endpoint.bound_sockets(),
            "network": {
                "localIpAddresses": address.ip_addrs().map(ToString::to_string).collect::<Vec<_>>(),
                "relayUrls": address.relay_urls().map(ToString::to_string).collect::<Vec<_>>(),
                "report": report,
                "counters": {
                    "scope": "endpoint_lifetime",
                    "reports": metrics.net_report.reports.get(),
                    "holepunchAttempts": metrics.socket.holepunch_attempts.get(),
                    "directPathsOpened": metrics.socket.paths_direct.get(),
                    "relayPathsOpened": metrics.socket.paths_relay.get(),
                    "relayConnectionsFailed": metrics.socket.relay_conns_failed.get(),
                    "relayConnectionsRateLimited": metrics.socket.relay_conns_ratelimited.get(),
                },
                "portMapping": {
                    "scope": "endpoint_lifetime",
                    "attempts": metrics.net_report.portmap_attempts.get(),
                    "externalAddressUpdates": metrics.net_report.portmap_external_address_updated.get(),
                    "changeEvents": metrics.socket.actor_tick_portmap_changed.get(),
                },
            },
            "connections": connections,
            "localWebRtc": self.rtc.iter().map(|(id, peer)| (id.clone(), json!({
                "clientId": peer.client,
                "connectionState": peer.peer.pc.connection_state().to_string(),
                "iceConnectionState": peer.peer.pc.ice_connection_state().to_string(),
                "signalingState": peer.peer.pc.signaling_state().to_string(),
                "startupDeadlineRemainingMs": peer.deadline.map(|deadline| deadline.saturating_duration_since(Instant::now()).as_millis()),
            }))).collect::<HashMap<_, _>>(),
            "actor": {"coordinator": self.owner(), "cachedReceipts": self.receipts.len(), "mediaReceivers": self.hub.packets.receiver_count(), "queuedMediaPackets": self.hub.packets.len(), "shareDeadlineRemainingMs": self.share_deadline.map(|deadline| deadline.saturating_duration_since(Instant::now()).as_millis())}
        })
    }

    pub(super) fn ticket(&self) -> Option<String> {
        let room = self.state.room.as_ref()?;
        let code = RoomCode::parse(self.state.join_code.as_deref()?).ok()?;
        let address = if self.owner() {
            self.endpoint.addr()
        } else {
            let connection = &self.upstream.as_ref()?.connection;
            let paths = connection.paths();
            iroh::EndpointAddr::from(connection.remote_id())
                .with_addrs(paths.iter().map(|path| path.remote_addr().clone()))
        };
        if address.id.to_string() != room.owner_id {
            return None;
        }
        RoomTicket::create(&code, address).ok()
    }
}
