use super::*;

impl Handle {
    pub async fn spawn(
        endpoint: Endpoint,
        tasks: TaskTracker,
        shutdown: CancellationToken,
        discovery: Discovery,
    ) -> Result<Self> {
        let files = crate::files::Files::new()
            .await
            .context("初始化文件存储失败")?;
        let (commands, receiver) = mpsc::channel(64);
        let (media_events, media_receiver) = mpsc::channel(32);
        let hub = Hub::new(media_events, tasks.clone(), shutdown.clone());
        let snapshot = Snapshot {
            event_seq: 0,
            endpoint_id: endpoint.id().to_string(),
            room: None,
            files: Default::default(),
            file_clients: Default::default(),
            file_errors: Default::default(),
            join_code: None,
            join_ticket: None,
            capture: None,
            subscription: None,
            connection: None,
            network_bitrate: None,
            error: None,
        };
        let (snapshots, view) = watch::channel(snapshot.clone());
        let handle = Self {
            commands,
            files,
            endpoint: endpoint.clone(),
            snapshots: view,
            hub: hub.clone(),
            shutdown: shutdown.clone(),
            tasks: tasks.clone(),
        };
        let actor = Actor {
            endpoint,
            state: snapshot,
            code: None,
            discovery,
            publisher: None,
            peers: HashMap::new(),
            upstream: None,
            rtc: HashMap::new(),
            share_deadline: None,
            downstream_budget: None,
            last_budget_sent: None,
            receipts: VecDeque::new(),
            snapshots,
            hub,
            tasks: tasks.clone(),
            shutdown,
            handle: handle.clone(),
        };
        tasks.spawn(actor.run(receiver, media_receiver));
        Ok(handle)
    }

    pub async fn call(&self, client: String, operation: Operation) -> Result<Value> {
        self.call_scoped(client, operation, CancellationToken::new(), None)
            .await
    }

    pub async fn call_scoped(
        &self,
        client: String,
        operation: Operation,
        cancellation: CancellationToken,
        idempotency_key: Option<String>,
    ) -> Result<Value> {
        let starting = matches!(operation, Operation::StartShare { .. });
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::Local {
                client: client.clone(),
                operation,
                cancellation: cancellation.clone(),
                idempotency_key,
                reply,
            })
            .await
            .context("应用已停止")?;
        let value = response.await.context("应用操作已取消")??;
        if !starting || value.get("state").and_then(Value::as_str) != Some("requesting") {
            return Ok(value);
        }
        let mut media: LocalMedia = serde_json::from_value(value)?;
        let mut snapshots = self.snapshots.clone();
        let result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err(fault("client_disconnected", "页面会话已断开", 401)),
            result = tokio::time::timeout(Duration::from_secs(10), snapshots.wait_for(|state| state.capture.as_ref().is_none_or(|capture| capture.id != media.id || capture.state != "requesting"))) => {
                match result {
                    Ok(Ok(state)) if state.capture.as_ref().is_some_and(|capture| capture.id == media.id) => { media.state = "preparing".into(); Ok(serde_json::to_value(&media)?) },
                    Ok(Ok(state)) => Err(fault("share_conflict", state.error.as_deref().unwrap_or("分享申请已取消"), 409)),
                    _ => Err(fault("share_timeout", "申请分享超时，请重试", 504)),
                }
            }
        };
        if result.is_err() {
            let _ = Box::pin(self.call(client, Operation::StopShare { id: media.share_id })).await;
        }
        result
    }

    pub async fn diagnostics(&self) -> Result<Value> {
        tokio::time::timeout(Duration::from_secs(5), async {
            let (reply, response) = oneshot::channel();
            self.commands
                .send(Command::Diagnostics { reply })
                .await
                .context("应用已停止")?;
            response.await.context("诊断读取已取消")
        })
        .await
        .context("诊断读取超时")?
    }

    pub fn detached(&self, peer: String, session: String, reason: DisconnectReason) {
        let handle = self.clone();
        self.tasks.spawn(async move {
            tokio::select! {
                _ = handle.shutdown.cancelled() => {},
                _ = handle.network(NetworkEvent::Detached { peer, session, reason }) => {},
            }
        });
    }

    pub async fn network(&self, event: NetworkEvent) -> Result<()> {
        self.commands
            .send(Command::Network(event))
            .await
            .context("应用已停止")
    }

    pub async fn attach(
        &self,
        connection: Connection,
        session: String,
        hello: Wire,
        sender: mpsc::Sender<Wire>,
        subscription: watch::Sender<Option<String>>,
        publishing: watch::Sender<Option<String>>,
    ) -> Result<Room> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::Attach {
                connection,
                session,
                hello,
                sender,
                subscription,
                publishing,
                reply,
            })
            .await
            .context("应用已停止")?;
        response.await.context("应用操作已取消")?
    }
}
