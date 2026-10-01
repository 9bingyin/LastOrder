use std::time::Instant;

use super::*;

pub(super) async fn events(
    State(state): State<WebState>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let client_id = random_id()?;
    {
        let clients = state
            .clients
            .lock()
            .map_err(|_| anyhow::anyhow!("本地会话状态不可用"))?;
        if clients.len() >= 8 {
            return Err(ApiError {
                status: StatusCode::TOO_MANY_REQUESTS,
                code: "client_limit",
                message: "最多打开 8 个本地页面".into(),
            });
        }
    }
    Ok(ws
        .protocols(["lastorder"])
        .max_message_size(4096)
        .on_upgrade(move |socket| socket_session(state, client_id, socket)))
}

fn snapshot_message(snapshot: &Snapshot, client: &str) -> Message {
    Message::Text(
        json!({"type": "snapshot", "clientId": client, "data": snapshot})
            .to_string()
            .into(),
    )
}

async fn send_socket(state: &WebState, socket: &mut WebSocket, message: Message) -> bool {
    tokio::select! {
        _ = state.app.shutdown.cancelled() => false,
        result = tokio::time::timeout(Duration::from_secs(5), socket.send(message)) => matches!(result, Ok(Ok(()))),
    }
}

async fn socket_session(state: WebState, client: String, mut socket: WebSocket) {
    let registered = state.clients.lock().is_ok_and(|mut clients| {
        if clients.len() >= 8 {
            return false;
        }
        clients.insert(client.clone(), state.app.shutdown.child_token());
        true
    });
    if !registered {
        return;
    }
    let mut snapshots: watch::Receiver<Snapshot> = state.app.snapshots.clone();
    let snapshot = snapshots.borrow_and_update().clone();
    if send_socket(&state, &mut socket, snapshot_message(&snapshot, &client)).await {
        let mut heartbeat = tokio::time::interval(Duration::from_secs(5));
        let mut last_seen = Instant::now();
        loop {
            tokio::select! {
                _ = state.app.shutdown.cancelled() => break,
                changed = snapshots.changed() => {
                    if changed.is_err() { break; }
                    let snapshot = snapshots.borrow_and_update().clone();
                    if !send_socket(&state, &mut socket, snapshot_message(&snapshot, &client)).await { break; }
                }
                message = socket.recv() => match message {
                    Some(Ok(Message::Pong(_) | Message::Text(_))) => last_seen = Instant::now(),
                    Some(Ok(Message::Ping(data))) => { last_seen = Instant::now(); if !send_socket(&state, &mut socket, Message::Pong(data)).await { break; } },
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {},
                },
                _ = heartbeat.tick() => {
                    if last_seen.elapsed() > Duration::from_secs(15) || !send_socket(&state, &mut socket, Message::Ping(vec![].into())).await { break; }
                }
            }
        }
    }
    if let Ok(mut clients) = state.clients.lock()
        && let Some(token) = clients.remove(&client)
    {
        token.cancel();
    }
    if !state.app.shutdown.is_cancelled() {
        let _ = state.app.call(client, Operation::ReleaseClient).await;
    }
}
