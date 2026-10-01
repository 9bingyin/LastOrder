mod peer;
pub use peer::Peer;

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use tokio::sync::{broadcast, mpsc};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use webrtc::{
    api::{
        APIBuilder,
        interceptor_registry::{configure_nack, configure_rtcp_reports, configure_twcc},
        media_engine::MediaEngine,
        setting_engine::SettingEngine,
    },
    ice::network_type::NetworkType,
    interceptor::registry::Registry,
    peer_connection::{
        RTCPeerConnection, configuration::RTCConfiguration,
        sdp::session_description::RTCSessionDescription,
    },
    rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication,
    rtp::packet::Packet,
    rtp_transceiver::rtp_codec::{RTCRtpCodecCapability, RTCRtpCodecParameters, RTPCodecType},
    track::track_local::{TrackLocalWriter, track_local_static_rtp::TrackLocalStaticRTP},
};

pub mod packet;

pub const VIDEO_PT: u8 = 96;
pub const AUDIO_PT: u8 = 111;

#[derive(Clone)]
pub struct MediaPacket {
    pub generation: [u8; 16],
    pub packet: Packet,
    pub created: Instant,
}

pub enum MediaEvent {
    Ready(String),
    Failed {
        peer_id: String,
        message: String,
    },
    Keyframe(String),
    Budget {
        peer: String,
        session: String,
        generation: String,
        bitrate: u32,
    },
}

pub struct Hub {
    pub packets: broadcast::Sender<MediaPacket>,
    source: Mutex<Option<Source>>,
    pub events: mpsc::Sender<MediaEvent>,
    tasks: TaskTracker,
    shutdown: CancellationToken,
}

struct Source {
    generation: String,
    pc: std::sync::Weak<RTCPeerConnection>,
    ssrc: u32,
    last_pli: Instant,
}

impl Hub {
    pub fn new(
        events: mpsc::Sender<MediaEvent>,
        tasks: TaskTracker,
        shutdown: CancellationToken,
    ) -> Arc<Self> {
        let (packets, _) = broadcast::channel(512);
        Arc::new(Self {
            packets,
            source: Mutex::new(None),
            events,
            tasks,
            shutdown,
        })
    }

    pub async fn keyframe(&self, generation: &str) {
        let source = self.source.lock().ok().and_then(|mut source| {
            let source = source.as_mut()?;
            if source.generation != generation
                || source.last_pli.elapsed() < Duration::from_millis(300)
            {
                return None;
            }
            source.last_pli = Instant::now();
            Some((source.pc.upgrade()?, source.ssrc))
        });
        if let Some((pc, ssrc)) = source
            && let Err(error) = pc
                .write_rtcp(&[Box::new(PictureLossIndication {
                    sender_ssrc: 0,
                    media_ssrc: ssrc,
                })])
                .await
        {
            tracing::debug!(%error, "请求关键帧失败");
        }
    }
}
