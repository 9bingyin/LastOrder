use super::*;

impl Actor {
    pub(super) fn refresh_budget(&mut self) {
        self.state.network_bitrate = None;
        let Some(share) = self
            .state
            .room
            .as_ref()
            .and_then(|room| room.share.as_ref())
            .filter(|share| share.profile.mode == QualityMode::Auto)
        else {
            self.last_budget_sent = None;
            return;
        };
        let maximum = share
            .profile
            .bitrate
            .saturating_add(if share.audio { 128_000 } else { 0 });
        let downstream = if self.owner() {
            self.peers
                .values()
                .filter(|peer| peer.subscription.borrow().as_ref() == Some(&share.generation))
                .filter_map(|peer| peer.budget.as_ref()?.current(&share.generation))
                .min()
                .unwrap_or(maximum)
        } else {
            self.downstream_budget
                .as_ref()
                .and_then(|budget| budget.current(&share.generation))
                .unwrap_or(maximum)
        }
        .min(maximum);
        if self.owner() && share.publisher_id != self.state.endpoint_id {
            let send = self.last_budget_sent.as_ref().is_none_or(|old| {
                old.generation != share.generation
                    || old.bitrate != downstream
                    || old.updated.elapsed() >= Duration::from_secs(1)
            });
            if send
                && let Some(peer) = self.peers.get(&share.publisher_id)
                && peer
                    .sender
                    .try_send(Wire::NetworkBudget {
                        generation: share.generation.clone(),
                        bitrate: downstream,
                    })
                    .is_ok()
            {
                self.last_budget_sent = Some(LinkBudget {
                    generation: share.generation.clone(),
                    bitrate: downstream,
                    updated: Instant::now(),
                });
            }
        }
        if self
            .state
            .capture
            .as_ref()
            .is_some_and(|capture| capture.generation == share.generation)
            && share.publisher_id == self.state.endpoint_id
        {
            let upstream = self
                .upstream
                .as_ref()
                .and_then(|peer| peer.budget.as_ref())
                .and_then(|budget| budget.current(&share.generation))
                .unwrap_or(maximum);
            self.state.network_bitrate = Some(downstream.min(upstream));
        }
    }
}
