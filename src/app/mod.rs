mod actor;
mod budget;
mod diagnostics;
mod files;
mod handle;
mod lifecycle;
mod network;
mod operations;

use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use iroh::{Endpoint, endpoint::Connection};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use crate::{
    media::{Hub, MediaEvent, Peer},
    net::{self, discovery::Discovery},
    protocol::{
        FileState, JoinTarget, LinkInfo, LocalMedia, Member, QualityMode, Room, RoomCode,
        RoomTicket, Share, ShareState, SharedFile, Snapshot, VideoProfile, Wire, random_id,
        secret_matches, validate_name,
    },
};

#[derive(Debug)]
pub struct Fault {
    pub code: &'static str,
    pub message: String,
    pub status: u16,
}
impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Fault {}
#[cfg(test)]
mod tests;

fn fault(code: &'static str, message: &str, status: u16) -> anyhow::Error {
    Fault {
        code,
        message: message.into(),
        status,
    }
    .into()
}

#[derive(Clone, PartialEq, Eq)]
pub enum Operation {
    Create {
        name: String,
    },
    Join {
        code: String,
        name: String,
    },
    Leave,
    StartShare {
        profile: VideoProfile,
        audio: bool,
    },
    UpdateShare {
        id: String,
        profile: VideoProfile,
    },
    StopShare {
        id: String,
    },
    Subscribe {
        share_id: String,
        generation: String,
    },
    Unsubscribe {
        id: String,
    },
    Answer {
        peer_id: String,
        sdp: String,
    },
    Playing {
        id: String,
    },
    OfferFile {
        room_id: String,
        recipient_id: String,
        name: String,
        size: u64,
    },
    ReadyFile {
        file: SharedFile,
    },
    ChangeFile {
        room_id: String,
        id: String,
        state: FileState,
    },
    ReleaseClient,
}

#[derive(Clone, Copy)]
pub enum DisconnectReason {
    RoomClosed,
    ConnectionLost,
}

pub enum NetworkEvent {
    Message {
        peer: String,
        session: String,
        message: Wire,
    },
    Detached {
        peer: String,
        session: String,
        reason: DisconnectReason,
    },
}

enum Command {
    Local {
        client: String,
        operation: Operation,
        cancellation: CancellationToken,
        idempotency_key: Option<String>,
        reply: oneshot::Sender<Result<Value>>,
    },
    Attach {
        connection: Connection,
        session: String,
        hello: Wire,
        sender: mpsc::Sender<Wire>,
        subscription: watch::Sender<Option<String>>,
        publishing: watch::Sender<Option<String>>,
        reply: oneshot::Sender<Result<Room>>,
    },
    Network(NetworkEvent),
    Diagnostics {
        reply: oneshot::Sender<Value>,
    },
}

#[derive(Clone)]
pub struct Handle {
    commands: mpsc::Sender<Command>,
    pub snapshots: watch::Receiver<Snapshot>,
    pub hub: Arc<Hub>,
    pub files: Arc<crate::files::Files>,
    pub endpoint: Endpoint,
    pub shutdown: CancellationToken,
    tasks: TaskTracker,
}
impl std::fmt::Debug for Handle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Handle").finish_non_exhaustive()
    }
}

struct RemotePeer {
    connection: Connection,
    session: String,
    sender: mpsc::Sender<Wire>,
    subscription: watch::Sender<Option<String>>,
    subscription_id: Option<String>,
    publishing: watch::Sender<Option<String>>,
    budget: Option<LinkBudget>,
}
struct LinkBudget {
    generation: String,
    bitrate: u32,
    updated: Instant,
}
impl LinkBudget {
    fn current(&self, generation: &str) -> Option<u32> {
        (self.generation == generation && self.updated.elapsed() < Duration::from_secs(3))
            .then_some(self.bitrate)
    }
}
struct LocalPeer {
    peer: Peer,
    client: String,
    deadline: Option<Instant>,
}
struct Receipt {
    client: String,
    key: String,
    operation: Operation,
    value: Value,
    created: Instant,
}

struct Actor {
    endpoint: Endpoint,
    state: Snapshot,
    code: Option<RoomCode>,
    discovery: Discovery,
    publisher: Option<CancellationToken>,
    peers: HashMap<String, RemotePeer>,
    upstream: Option<RemotePeer>,
    rtc: HashMap<String, LocalPeer>,
    share_deadline: Option<Instant>,
    downstream_budget: Option<LinkBudget>,
    last_budget_sent: Option<LinkBudget>,
    receipts: VecDeque<Receipt>,
    snapshots: watch::Sender<Snapshot>,
    hub: Arc<Hub>,
    tasks: TaskTracker,
    shutdown: CancellationToken,
    handle: Handle,
}
