use std::path::Path;

use super::*;

impl Handle {
    pub fn shared_file(&self, room_id: &str, id: &str) -> Result<SharedFile> {
        let snapshot = self.snapshots.borrow();
        snapshot
            .room
            .as_ref()
            .filter(|room| room.id == room_id)
            .context("房间已变化")?;
        snapshot
            .files
            .get(id)
            .filter(|file| file.room_id == room_id)
            .cloned()
            .ok_or_else(|| fault("file_not_found", "文件邀请不存在", 404))
    }

    pub async fn wait_file(
        &self,
        file: &SharedFile,
        cancellation: CancellationToken,
    ) -> Result<SharedFile> {
        let mut snapshots = self.snapshots.clone();
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => bail!("页面会话已断开"),
            _ = self.shutdown.cancelled() => bail!("应用已停止"),
            result = snapshots.wait_for(|state| {
                state.room.as_ref().is_none_or(|room| room.id != file.room_id)
                    || state.files.get(&file.id).is_some_and(|current| current.state.reaches(file.state) || !current.state.active())
                    || state.file_errors.contains_key(&file.id)
            }) => {
                let state = result?;
                let current = state.files.get(&file.id).filter(|current| current.same_offer(file) && current.state.reaches(file.state))
                    .context(state.file_errors.get(&file.id).cloned().unwrap_or_else(|| "文件邀请已失效".into()))?;
                Ok(current.clone())
            }
        }
    }

    pub async fn prepare_file(
        &self,
        client: String,
        room_id: String,
        id: String,
        path: &Path,
        cancellation: CancellationToken,
    ) -> Result<SharedFile> {
        let offer = self.shared_file(&room_id, &id)?;
        if offer.publisher_id != self.endpoint.id().to_string()
            || offer.state != FileState::Accepted
        {
            return Err(fault(
                "permission_denied",
                "需要指定接收者确认后才能准备文件",
                403,
            ));
        }
        if self.snapshots.borrow().file_clients.get(&id) != Some(&client) {
            return Err(fault(
                "permission_denied",
                "请在发送邀请的页面准备文件",
                403,
            ));
        }
        let mut snapshots = self.snapshots.clone();
        let file = tokio::select! {
            biased;
            _ = cancellation.cancelled() => bail!("页面会话已断开"),
            _ = self.shutdown.cancelled() => bail!("应用已停止"),
            _ = snapshots.wait_for(|state| state.files.get(&id) != Some(&offer)) => bail!("文件邀请已变化"),
            result = self.files.import(&self.endpoint, offer.clone(), path) => result?,
        };
        self.call_scoped(
            client,
            Operation::ReadyFile { file: file.clone() },
            cancellation.clone(),
            None,
        )
        .await?;
        self.wait_file(&file, cancellation).await
    }

    pub async fn download_file(
        &self,
        client: &str,
        room_id: &str,
        id: &str,
        cancellation: CancellationToken,
    ) -> Result<Arc<crate::files::CachedFile>> {
        let file = self.shared_file(room_id, id)?;
        if file.recipient_id != self.endpoint.id().to_string()
            || file.state != FileState::Ready
            || self
                .snapshots
                .borrow()
                .file_clients
                .get(id)
                .map(String::as_str)
                != Some(client)
        {
            return Err(fault(
                "permission_denied",
                "只能由确认接收的页面下载此文件",
                403,
            ));
        }
        let mut snapshots = self.snapshots.clone();
        tokio::select! {
            biased;
            _ = self.shutdown.cancelled() => bail!("应用已停止"),
            _ = snapshots.wait_for(|state| state.files.get(id) != Some(&file)) => bail!("文件传输已结束"),
            result = self.files.download(&self.endpoint, file.clone(), cancellation) => result,
        }
    }
}

impl Actor {
    fn file_room(&self, room_id: &str) -> Result<&Room> {
        self.state
            .room
            .as_ref()
            .filter(|room| room.id == room_id)
            .ok_or_else(|| fault("room_conflict", "房间已变化", 409))
    }

    pub(super) fn offer_file(
        &mut self,
        client: &str,
        room_id: String,
        recipient_id: String,
        name: String,
        size: u64,
    ) -> Result<Value> {
        let file = SharedFile {
            id: random_id()?,
            room_id,
            name,
            size,
            publisher_id: self.state.endpoint_id.clone(),
            recipient_id,
            state: FileState::Offered,
            blob_ticket: None,
        };
        self.validate_offer(&file.publisher_id, &file)?;
        if self.owner() {
            self.add_file(&file.publisher_id.clone(), file.clone())?;
        } else {
            self.send_upstream(Wire::OfferFile { file: file.clone() })?;
        }
        self.state
            .file_clients
            .insert(file.id.clone(), client.into());
        Ok(json!(file))
    }

    fn validate_offer(&self, publisher: &str, file: &SharedFile) -> Result<()> {
        file.validate()?;
        let room = self.file_room(&file.room_id)?;
        if file.publisher_id != publisher
            || !room.members.contains_key(publisher)
            || !room.members.contains_key(&file.recipient_id)
            || file.state != FileState::Offered
        {
            bail!("文件邀请来源或接收对象无效");
        }
        Ok(())
    }

    pub(super) fn add_file(&mut self, publisher: &str, file: SharedFile) -> Result<()> {
        self.validate_offer(publisher, &file)?;
        if self.state.files.contains_key(&file.id) {
            bail!("文件邀请标识已被占用");
        }
        self.state.files.insert(file.id.clone(), file.clone());
        self.send_file(&file);
        Ok(())
    }

    pub(super) fn ready_file(&mut self, file: SharedFile) -> Result<Value> {
        if file.publisher_id != self.state.endpoint_id || self.handle.files.cached(&file).is_none()
        {
            return Err(fault("permission_denied", "文件未在本机准备完成", 403));
        }
        self.state.file_errors.remove(&file.id);
        if self.owner() {
            self.apply_ready(&self.state.endpoint_id.clone(), file)?;
        } else {
            file.validate()?;
            self.send_upstream(Wire::ReadyFile { file })?;
        }
        Ok(json!({}))
    }

    pub(super) fn apply_ready(&mut self, peer: &str, file: SharedFile) -> Result<()> {
        file.validate()?;
        self.file_room(&file.room_id)?;
        let previous = self.state.files.get(&file.id).context("文件邀请不存在")?;
        if peer != file.publisher_id
            || !previous.same_offer(&file)
            || previous.state != FileState::Accepted
            || file.state != FileState::Ready
        {
            bail!("接收对象尚未确认或邀请已经变化");
        }
        self.state.files.insert(file.id.clone(), file.clone());
        self.send_file(&file);
        Ok(())
    }

    fn changed_file(
        &self,
        peer: &str,
        room_id: &str,
        id: &str,
        state: FileState,
    ) -> Result<SharedFile> {
        self.file_room(room_id)?;
        let file = self
            .state
            .files
            .get(id)
            .filter(|file| file.room_id == room_id)
            .context("文件邀请不存在")?;
        let permitted = match state {
            FileState::Accepted | FileState::Rejected => {
                peer == file.recipient_id && file.state == FileState::Offered
            }
            FileState::Completed => peer == file.recipient_id && file.state == FileState::Ready,
            FileState::Cancelled => {
                (peer == file.publisher_id || peer == file.recipient_id) && file.state.active()
            }
            _ => false,
        };
        if !permitted {
            return Err(fault("permission_denied", "无权变更此文件邀请", 403));
        }
        let mut changed = file.clone();
        changed.state = state;
        if state != FileState::Ready {
            changed.blob_ticket = None;
        }
        Ok(changed)
    }

    pub(super) fn change_file(
        &mut self,
        room_id: &str,
        id: &str,
        state: FileState,
    ) -> Result<Value> {
        let file = self.changed_file(&self.state.endpoint_id, room_id, id, state)?;
        self.state.file_errors.remove(id);
        if self.owner() {
            self.apply_change(&self.state.endpoint_id.clone(), room_id, id, state)?;
        } else {
            self.send_upstream(Wire::ChangeFile {
                file_id: id.into(),
                state,
            })?;
        }
        Ok(json!(file))
    }

    pub(super) fn apply_change(
        &mut self,
        peer: &str,
        room_id: &str,
        id: &str,
        state: FileState,
    ) -> Result<()> {
        let file = self.changed_file(peer, room_id, id, state)?;
        self.state.files.insert(id.into(), file.clone());
        self.send_file(&file);
        Ok(())
    }

    fn send_file(&self, file: &SharedFile) {
        for id in [&file.publisher_id, &file.recipient_id] {
            if let Some(peer) = self.peers.get(id)
                && peer
                    .sender
                    .try_send(Wire::FileTransfer { file: file.clone() })
                    .is_err()
            {
                peer.connection.close(0u32.into(), b"control queue full");
            }
        }
    }

    pub(super) fn cancel_peer_files(&mut self, peer: &str) {
        let affected: Vec<_> = self
            .state
            .files
            .values()
            .filter(|file| {
                file.state.active() && (file.publisher_id == peer || file.recipient_id == peer)
            })
            .cloned()
            .collect();
        for mut file in affected {
            file.state = FileState::Cancelled;
            file.blob_ticket = None;
            self.state.files.insert(file.id.clone(), file.clone());
            self.send_file(&file);
        }
    }

    pub(super) fn reconcile_file_transfers(&mut self) {
        let missing: Vec<_> = self
            .state
            .files
            .values()
            .filter(|file| file.state.active())
            .filter(|file| {
                self.state.room.as_ref().is_none_or(|room| {
                    room.id != file.room_id
                        || !room.members.contains_key(&file.publisher_id)
                        || !room.members.contains_key(&file.recipient_id)
                })
            })
            .map(|file| file.id.clone())
            .collect();
        for id in missing {
            if let Some(file) = self.state.files.get_mut(&id) {
                file.state = FileState::Cancelled;
                file.blob_ticket = None;
            }
        }
    }

    pub(super) fn release_file_client(&mut self, client: &str) {
        let pending: Vec<_> = self
            .state
            .file_clients
            .iter()
            .filter(|(id, owner)| owner.as_str() == client && !self.state.files.contains_key(*id))
            .map(|(id, _)| id.clone())
            .collect();
        for id in pending {
            let _ = self.send_upstream(Wire::ChangeFile {
                file_id: id,
                state: FileState::Cancelled,
            });
        }
        let files: Vec<_> = self
            .state
            .file_clients
            .iter()
            .filter(|(_, owner)| owner.as_str() == client)
            .filter_map(|(id, _)| {
                self.state
                    .files
                    .get(id)
                    .filter(|file| file.state.active())
                    .cloned()
            })
            .collect();
        for file in files {
            let _ = self.change_file(&file.room_id, &file.id, FileState::Cancelled);
        }
        self.state.file_clients.retain(|_, owner| owner != client);
    }
}
