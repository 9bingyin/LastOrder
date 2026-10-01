use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use iroh::{EndpointAddr, SecretKey};
use iroh_tickets::{ParseError, Ticket, endpoint::EndpointTicket};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use uuid::{Uuid, Variant, Version};

pub const ALPN: &[u8] = b"lastorder/3";
pub const ROOM_CLOSED: u32 = 1;
pub const MAX_MEMBERS: usize = 5;
pub const MAX_MESSAGE: usize = 64 * 1024;
pub const MAX_JOIN_INPUT: usize = 4096;

pub fn random_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).context("生成随机标识失败")?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub fn secret() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).context("生成访问凭证失败")?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

pub fn secret_matches(left: &str, right: &str) -> bool {
    left.as_bytes().ct_eq(right.as_bytes()).into()
}

#[derive(Clone)]
pub struct RoomCode(Uuid);

impl RoomCode {
    pub fn generate() -> Result<Self> {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes).context("生成房间加入码失败")?;
        Ok(Self(uuid::Builder::from_random_bytes(bytes).into_uuid()))
    }

    pub fn parse(value: &str) -> Result<Self> {
        let value = value.trim();
        if value.len() != 36 {
            bail!("加入码须为 UUID");
        }
        let uuid = Uuid::parse_str(value).context("加入码无效")?;
        if uuid.get_version() != Some(Version::Random) || uuid.get_variant() != Variant::RFC4122 {
            bail!("加入码须为随机 UUID");
        }
        Ok(Self(uuid))
    }

    pub fn id(&self) -> String {
        self.0.to_string()
    }

    pub fn discovery_key(&self) -> SecretKey {
        SecretKey::from_bytes(&blake3::derive_key(
            "lastorder.room.discovery.v1",
            self.0.as_bytes(),
        ))
    }

    pub fn capability(&self) -> String {
        URL_SAFE_NO_PAD.encode(blake3::derive_key(
            "lastorder.room.authentication.v1",
            self.0.as_bytes(),
        ))
    }
}

pub enum JoinTarget {
    Uuid(RoomCode),
    Ticket {
        code: RoomCode,
        address: EndpointAddr,
    },
}
impl JoinTarget {
    pub fn parse(value: &str) -> Result<Self> {
        let value = value.trim();
        if value.len() > MAX_JOIN_INPUT {
            bail!("加入信息过长");
        }
        if value.len() == 36 {
            return Ok(Self::Uuid(RoomCode::parse(value)?));
        }
        let ticket = RoomTicket::decode_string(&value.to_ascii_lowercase())
            .context("LastOrder Ticket 无效")?;
        Ok(Self::Ticket {
            code: ticket.code,
            address: ticket.address,
        })
    }
}

pub struct RoomTicket {
    code: RoomCode,
    address: EndpointAddr,
}
impl RoomTicket {
    pub fn create(code: &RoomCode, address: EndpointAddr) -> Result<String> {
        if address.addrs.len() > 16 {
            bail!("Ticket 地址过多");
        }
        let value = Self {
            code: code.clone(),
            address,
        }
        .encode_string();
        if value.len() > MAX_JOIN_INPUT {
            bail!("Ticket 过长");
        }
        Ok(value)
    }
}
impl Ticket for RoomTicket {
    const KIND: &'static str = "lastorder";

    fn encode_bytes(&self) -> Vec<u8> {
        let mut bytes = vec![1];
        bytes.extend_from_slice(self.code.0.as_bytes());
        bytes.extend(EndpointTicket::new(self.address.clone()).encode_bytes());
        bytes
    }

    fn decode_bytes(bytes: &[u8]) -> std::result::Result<Self, ParseError> {
        if bytes.len() < 18 || bytes.len() > MAX_JOIN_INPUT * 5 / 8 || bytes[0] != 1 {
            return Err(ParseError::verification_failed(
                "invalid LastOrder ticket version or size",
            ));
        }
        let uuid = Uuid::from_slice(&bytes[1..17])
            .map_err(|_| ParseError::verification_failed("invalid room UUID"))?;
        let code = RoomCode::parse(&uuid.to_string())
            .map_err(|_| ParseError::verification_failed("room UUID must be v4"))?;
        let endpoint = EndpointTicket::decode_bytes(&bytes[17..])?;
        let address = endpoint.endpoint_addr().clone();
        if address.addrs.len() > 16 {
            return Err(ParseError::verification_failed(
                "too many endpoint addresses",
            ));
        }
        let ticket = Self { code, address };
        if ticket.encode_bytes() != bytes {
            return Err(ParseError::verification_failed(
                "non-canonical LastOrder ticket",
            ));
        }
        Ok(ticket)
    }
}

pub fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn validate_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 32 || name.chars().any(char::is_control) {
        bail!("昵称须为 1–32 个字符，不能包含控制字符");
    }
    Ok(name.to_owned())
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QualityMode {
    #[default]
    Original,
    Auto,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VideoProfile {
    #[serde(default)]
    pub mode: QualityMode,
    pub height: u16,
    pub fps: u16,
    pub bitrate: u32,
}
impl Default for VideoProfile {
    fn default() -> Self {
        Self {
            mode: QualityMode::Original,
            height: 1080,
            fps: 30,
            bitrate: 6_000_000,
        }
    }
}
impl VideoProfile {
    pub fn validate(self) -> Result<Self> {
        if ![480, 720, 1080, 1440, 2160].contains(&self.height)
            || !(15..=60).contains(&self.fps)
            || !(500_000..=30_000_000).contains(&self.bitrate)
        {
            bail!("画质配置无效");
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Share {
    pub profile: VideoProfile,
    pub audio: bool,
    pub id: String,
    pub generation: String,
    pub publisher_id: String,
    pub state: ShareState,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ShareState {
    Preparing,
    Live,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Member {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Room {
    pub id: String,
    pub owner_id: String,
    pub revision: u64,
    pub members: BTreeMap<String, Member>,
    pub share: Option<Share>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalMedia {
    pub audio: bool,
    pub id: String,
    pub share_id: String,
    pub generation: String,
    pub rtc_peer_id: String,
    pub client_id: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub event_seq: u64,
    pub endpoint_id: String,
    pub room: Option<Room>,
    pub join_code: Option<String>,
    pub join_ticket: Option<String>,
    pub capture: Option<LocalMedia>,
    pub subscription: Option<LocalMedia>,
    pub connection: Option<LinkInfo>,
    pub network_bitrate: Option<u32>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkInfo {
    pub path: String,
    pub rtt_ms: u64,
    pub sent_bytes: u64,
    pub received_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Wire {
    Join {
        version: u8,
        room_id: String,
        capability: String,
        name: String,
    },
    Snapshot {
        room: Room,
    },
    StartShare {
        share_id: String,
        generation: String,
        profile: VideoProfile,
        audio: bool,
    },
    ShareGranted {
        share_id: String,
        generation: String,
    },
    ShareRejected {
        share_id: String,
        message: String,
    },
    ShareReady {
        share_id: String,
        generation: String,
    },
    StopShare {
        share_id: String,
        generation: String,
    },
    UpdateShare {
        share_id: String,
        generation: String,
        profile: VideoProfile,
    },
    Subscribe {
        share_id: String,
        generation: String,
        subscription_id: String,
    },
    Subscribed {
        share_id: String,
        generation: String,
        subscription_id: String,
    },
    Unsubscribe {
        subscription_id: String,
    },
    SubscriptionRejected {
        subscription_id: String,
        message: String,
    },
    Keyframe {
        generation: String,
    },
    NetworkBudget {
        generation: String,
        bitrate: u32,
    },
    Leave,
    Closed,
    Error {
        message: String,
    },
}

#[cfg(test)]
mod tests;
