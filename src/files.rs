use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use iroh::{
    Endpoint,
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};
use iroh_blobs::{
    BlobFormat, Hash,
    api::{TempTag, blobs::BlobStatus},
    protocol::Request,
    provider::{self, StreamPair, events::EventSender},
    store::{
        GcConfig,
        fs::{FsStore, options::Options},
    },
    ticket::BlobTicket,
};
use tempfile::TempDir;
use tokio::{sync::Semaphore, task::JoinSet};
use tokio_util::sync::CancellationToken;

use crate::{
    app::Handle,
    protocol::{FileState, SharedFile, Snapshot},
};

pub struct CachedFile {
    pub file: SharedFile,
    pub cancellation: CancellationToken,
    pub tag: TempTag,
    downloading: Semaphore,
}

pub struct Files {
    pub store: FsStore,
    cache: Mutex<HashMap<String, Arc<CachedFile>>>,
    directory: TempDir,
}
impl Files {
    pub async fn new() -> Result<Arc<Self>> {
        let directory = tempfile::Builder::new()
            .prefix("lastorder-files-")
            .tempdir()?;
        let mut options = Options::new(directory.path());
        options.gc = Some(GcConfig {
            interval: Duration::from_secs(30),
            add_protected: None,
        });
        let store = FsStore::load_with_opts(directory.path().join("blobs.db"), options).await?;
        Ok(Arc::new(Self {
            store,
            cache: Mutex::new(HashMap::new()),
            directory,
        }))
    }

    pub fn staging_file(&self) -> Result<tempfile::NamedTempFile> {
        Ok(tempfile::NamedTempFile::new_in(self.directory.path())?)
    }

    pub async fn import(
        &self,
        endpoint: &Endpoint,
        mut file: SharedFile,
        path: &Path,
    ) -> Result<SharedFile> {
        if tokio::fs::metadata(path).await?.len() != file.size {
            bail!("文件实际大小与邀请信息不一致");
        }
        let tag = self.store.blobs().add_path(path).temp_tag().await?;
        file.state = FileState::Ready;
        file.blob_ticket =
            Some(BlobTicket::new(endpoint.addr(), tag.hash(), BlobFormat::Raw).to_string());
        file.validate()?;
        let entry = Arc::new(CachedFile {
            file: file.clone(),
            tag,
            cancellation: CancellationToken::new(),
            downloading: Semaphore::new(1),
        });
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| anyhow::anyhow!("文件缓存不可用"))?;
        if let Some(previous) = cache.insert(file.id.clone(), entry) {
            previous.cancellation.cancel();
        }
        Ok(file)
    }

    pub fn cached(&self, file: &SharedFile) -> Option<Arc<CachedFile>> {
        self.cache
            .lock()
            .ok()?
            .get(&file.id)
            .filter(|entry| entry.file == *file && !entry.cancellation.is_cancelled())
            .cloned()
    }

    pub fn remove(&self, id: &str) {
        if let Ok(mut cache) = self.cache.lock()
            && let Some(entry) = cache.remove(id)
        {
            entry.cancellation.cancel();
        }
    }

    pub fn clear(&self) {
        if let Ok(mut cache) = self.cache.lock() {
            for (_, entry) in cache.drain() {
                entry.cancellation.cancel();
            }
        }
    }

    pub fn reconcile(&self, snapshot: &Snapshot) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.retain(|_, entry| {
                let keep = snapshot
                    .room
                    .as_ref()
                    .is_some_and(|room| room.id == entry.file.room_id)
                    && snapshot
                        .files
                        .get(&entry.file.id)
                        .is_some_and(|file| file.state.active() && file.same_offer(&entry.file));
                if !keep {
                    entry.cancellation.cancel();
                }
                keep
            });
        }
    }

    pub async fn download(
        &self,
        endpoint: &Endpoint,
        file: SharedFile,
        cancellation: CancellationToken,
    ) -> Result<Arc<CachedFile>> {
        let ticket = file.ticket()?;
        let cached = self.cached(&file);
        let entry = if let Some(entry) = cached {
            entry
        } else {
            let tag = self.store.tags().temp_tag(ticket.hash()).await?;
            let candidate = Arc::new(CachedFile {
                file,
                tag,
                cancellation: CancellationToken::new(),
                downloading: Semaphore::new(1),
            });
            let mut cache = self
                .cache
                .lock()
                .map_err(|_| anyhow::anyhow!("文件缓存不可用"))?;
            cache
                .entry(candidate.file.id.clone())
                .or_insert(candidate)
                .clone()
        };
        if entry.tag.hash() != ticket.hash() {
            bail!("文件连接信息已变化，请重新发起邀请");
        }
        let transfer = async {
            let _file_guard = entry.downloading.acquire().await?;
            if !self.store.blobs().has(ticket.hash()).await? {
                let connection = tokio::time::timeout(
                    Duration::from_secs(25),
                    endpoint.connect(ticket.addr().clone(), iroh_blobs::ALPN),
                )
                .await
                .context("连接文件发送者超时")?
                .context("无法连接文件发送者")?;
                let connection = FileConnection(connection);
                let (size, _) = tokio::time::timeout(
                    Duration::from_secs(15),
                    iroh_blobs::get::request::get_verified_size(&connection.0, &ticket.hash()),
                )
                .await
                .context("获取文件信息超时")?
                .context("无法读取文件信息")?;
                if size != entry.file.size {
                    bail!("文件实际大小与邀请信息不一致");
                }
                self.store
                    .remote()
                    .fetch(connection.0.clone(), ticket.hash())
                    .await
                    .context("下载失败，可重试继续下载")?;
            }
            if self.store.blobs().status(ticket.hash()).await?
                != (BlobStatus::Complete {
                    size: entry.file.size,
                })
            {
                bail!("文件校验失败");
            }
            Ok::<(), anyhow::Error>(())
        };
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => bail!("下载已取消"),
            _ = entry.cancellation.cancelled() => bail!("文件传输已结束"),
            result = transfer => result?,
        }
        Ok(entry)
    }

    pub async fn shutdown(&self) -> Result<()> {
        self.clear();
        self.store.shutdown().await?;
        Ok(())
    }
}

struct FileConnection(Connection);
impl Drop for FileConnection {
    fn drop(&mut self) {
        self.0.close(0u32.into(), b"file transfer ended");
    }
}

fn authorized_file(app: &Handle, room_id: &str, peer: &str, hash: Hash) -> Result<Arc<CachedFile>> {
    let state = app.snapshots.borrow();
    state
        .room
        .as_ref()
        .filter(|room| room.id == room_id && room.members.contains_key(peer))
        .context("接收对象已离开房间")?;
    state
        .files
        .values()
        .filter(|file| {
            file.room_id == room_id
                && file.publisher_id == state.endpoint_id
                && file.recipient_id == peer
                && file.state == FileState::Ready
                && file.ticket().is_ok_and(|ticket| ticket.hash() == hash)
        })
        .find_map(|file| app.files.cached(file))
        .context("接收对象尚未确认或文件已取消")
}

#[derive(Debug)]
pub struct FileProtocol {
    pub app: Handle,
}
impl ProtocolHandler for FileProtocol {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let connection = FileConnection(connection);
        let peer = connection.0.remote_id().to_string();
        let mut snapshots = self.app.snapshots.clone();
        let Some(room_id) = snapshots
            .borrow()
            .room
            .as_ref()
            .filter(|room| room.members.contains_key(&peer))
            .map(|room| room.id.clone())
        else {
            return Ok(());
        };
        let mut transfers = JoinSet::new();
        loop {
            tokio::select! {
                biased;
                _ = self.app.shutdown.cancelled() => break,
                changed = snapshots.changed() => {
                    if changed.is_err() || !snapshots.borrow().room.as_ref().is_some_and(|room| room.id == room_id && room.members.contains_key(&peer)) { break; }
                }
                Some(_) = transfers.join_next(), if !transfers.is_empty() => {},
                pair = connection.0.accept_bi() => {
                    let Ok((send, recv)) = pair else { break; };
                    let app = self.app.clone();
                    let peer = peer.clone();
                    let room_id = room_id.clone();
                    let connection_id = connection.0.stable_id() as u64;
                    transfers.spawn(async move {
                        let result = serve_file(app, peer, room_id, StreamPair::new(connection_id, recv, send, EventSender::DEFAULT)).await;
                        if let Err(error) = result { tracing::debug!(%error, "文件传输结束"); }
                    });
                }
            }
        }
        transfers.abort_all();
        while transfers.join_next().await.is_some() {}
        Ok(())
    }
    async fn shutdown(&self) {
        if let Err(error) = self.app.files.shutdown().await {
            tracing::warn!(%error, "文件存储关闭失败");
        }
    }
}

async fn serve_file(
    app: Handle,
    peer: String,
    room_id: String,
    mut pair: StreamPair,
) -> Result<()> {
    let request = tokio::time::timeout(Duration::from_secs(10), pair.read_request()).await??;
    let Request::Get(request) = request else {
        bail!("只允许接收普通文件");
    };
    if !request.ranges.is_blob() {
        bail!("不允许下载文件集合");
    }
    let hash = request.hash;
    let mut snapshots = app.snapshots.clone();
    let mut entry = authorized_file(&app, &room_id, &peer, hash)?;
    let serve = provider::handle_get(pair, app.files.store.clone().into(), request);
    tokio::pin!(serve);
    loop {
        tokio::select! {
            biased;
            _ = app.shutdown.cancelled() => bail!("应用已停止"),
            _ = entry.cancellation.cancelled() => { entry = authorized_file(&app, &room_id, &peer, hash)?; }
            changed = snapshots.changed() => {
                changed.context("房间已结束")?;
                entry = authorized_file(&app, &room_id, &peer, hash)?;
            }
            result = &mut serve => return Ok(result?),
        }
    }
}
