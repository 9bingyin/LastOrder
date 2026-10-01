use std::time::{Duration, Instant};

use iroh::endpoint::{Connection, PathId};

pub const MIN_BITRATE: u32 = 64_000;
pub const MAX_BITRATE: u32 = 64_000_000;

pub struct Estimator {
    bitrate: u32,
    path: Option<PathId>,
    delay: DelayTrend,
    buffer_capacity: usize,
    sent: u64,
    lost: u64,
    sampled: Instant,
}

impl Estimator {
    pub fn new(connection: &Connection) -> Self {
        let stats = connection.stats();
        Self {
            bitrate: MAX_BITRATE,
            path: None,
            delay: DelayTrend::default(),
            buffer_capacity: connection.datagram_send_buffer_space(),
            sent: stats.udp_tx.bytes,
            lost: stats.lost_bytes,
            sampled: Instant::now(),
        }
    }

    pub fn reset(&mut self, connection: &Connection) {
        let capacity = self.buffer_capacity;
        *self = Self::new(connection);
        self.buffer_capacity = self.buffer_capacity.max(capacity);
    }

    pub fn sample(&mut self, connection: &Connection, blocked: bool) -> u32 {
        let paths = connection.paths();
        let Some(path) = paths.iter().find(|path| path.is_selected()) else {
            self.bitrate = MIN_BITRATE;
            return self.bitrate;
        };
        if self.path != Some(path.id()) {
            self.path = Some(path.id());
            self.delay = DelayTrend::default();
        }
        let rtt = connection
            .rtt(path.id())
            .unwrap_or(Duration::from_millis(20));
        let rate = connection.congestion_state(path.id()).map(|state| {
            let metrics = state.metrics();
            metrics.pacing_rate.unwrap_or_else(|| {
                (metrics.congestion_window as f64 / rtt.as_secs_f64().max(0.001)) as u64
            })
        });
        let space = connection.datagram_send_buffer_space();
        self.buffer_capacity = self.buffer_capacity.max(space);
        let queued = self.buffer_capacity.saturating_sub(space);
        let stats = connection.stats();
        let sent = stats.udp_tx.bytes.saturating_sub(self.sent);
        let lost = stats.lost_bytes.saturating_sub(self.lost);
        self.sent = stats.udp_tx.bytes;
        self.lost = stats.lost_bytes;
        let now = Instant::now();
        let elapsed = now.duration_since(self.sampled);
        self.sampled = now;
        let congested = blocked
            || (sent > 0 && lost as f64 / sent as f64 > 0.02)
            || self.delay.rising(rtt, now, sent > 0)
            || rate.is_some_and(|rate| queued as f64 > rate as f64 * 0.1);
        self.bitrate = next_budget(self.bitrate, rate, congested, elapsed);
        self.bitrate
    }
}

#[derive(Default)]
struct DelayTrend {
    minimum: Option<Duration>,
    previous: Option<Duration>,
    started: Option<Instant>,
}

impl DelayTrend {
    fn rising(&mut self, rtt: Duration, now: Instant, active: bool) -> bool {
        if !active {
            return false;
        }
        if self
            .started
            .is_none_or(|started| now.duration_since(started) >= Duration::from_secs(10))
        {
            self.started = Some(now);
            self.minimum = Some(rtt);
            self.previous = Some(rtt);
            return false;
        }
        let minimum = self.minimum.unwrap_or(rtt).min(rtt);
        self.minimum = Some(minimum);
        // 基础延迟改变或陈旧 RTT 不应反复压低预算；只使用正在增长的延迟。
        let rising = rtt > minimum + (minimum / 2).max(Duration::from_millis(20))
            && self.previous.is_some_and(|previous| {
                rtt > previous + (previous / 20).max(Duration::from_millis(1))
            });
        self.previous = Some(rtt);
        rising
    }
}

fn next_budget(
    current: u32,
    bytes_per_second: Option<u64>,
    congested: bool,
    elapsed: Duration,
) -> u32 {
    // 只传递连续码率预算；分辨率、帧率和量化由浏览器决定。
    // 空闲/静态画面的实际发送量不是链路容量，不能据此持续降低预算。
    let value = if congested {
        let reduced = f64::from(current) * 0.85;
        bytes_per_second.map_or(reduced, |rate| reduced.min(rate as f64 * 8.0 * 0.85))
    } else {
        f64::from(current) * (1.0 + 0.15 * elapsed.as_secs_f64().min(2.0))
    };
    value
        .clamp(f64::from(MIN_BITRATE), f64::from(MAX_BITRATE))
        .round() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_higher_rtt_does_not_keep_degrading_and_baseline_expires() {
        let start = Instant::now();
        let mut trend = DelayTrend::default();
        assert!(!trend.rising(Duration::from_millis(20), start, true));
        assert!(trend.rising(
            Duration::from_millis(60),
            start + Duration::from_millis(500),
            true
        ));
        for tick in 2..20 {
            assert!(!trend.rising(
                Duration::from_millis(60),
                start + Duration::from_millis(tick * 500),
                true
            ));
        }
        assert!(!trend.rising(
            Duration::from_millis(60),
            start + Duration::from_secs(10),
            true
        ));
        assert!(!trend.rising(
            Duration::from_millis(80),
            start + Duration::from_millis(10_500),
            true
        ));
        assert!(!trend.rising(
            Duration::from_millis(200),
            start + Duration::from_secs(11),
            false
        ));
    }

    #[test]
    fn congestion_uses_continuous_transport_budget() {
        assert_eq!(
            next_budget(6_000_000, Some(287_123), true, Duration::from_secs(1)),
            1_952_436
        );
    }

    #[test]
    fn app_limited_rate_does_not_lower_the_budget() {
        assert_eq!(
            next_budget(6_000_000, Some(1_000), false, Duration::from_secs(1)),
            6_900_000
        );
    }

    #[test]
    fn budget_recovers_without_quality_steps() {
        assert_eq!(
            next_budget(1_234_567, None, false, Duration::from_secs(1)),
            1_419_752
        );
    }

    #[test]
    fn malformed_extreme_metrics_remain_bounded() {
        assert_eq!(
            next_budget(MIN_BITRATE, Some(0), true, Duration::from_secs(1)),
            MIN_BITRATE
        );
        assert_eq!(
            next_budget(MAX_BITRATE, Some(u64::MAX), false, Duration::MAX),
            MAX_BITRATE
        );
    }
}
