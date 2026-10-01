use super::events::events;
use super::*;

fn client(state: &WebState, headers: &HeaderMap) -> Result<(String, CancellationToken), ApiError> {
    let session = headers
        .get("x-client-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|id| {
            state
                .clients
                .lock()
                .ok()
                .and_then(|clients| clients.get(id).map(|token| (id.to_owned(), token.clone())))
        });
    session.ok_or_else(|| ApiError {
        status: StatusCode::UNAUTHORIZED,
        code: "client_disconnected",
        message: "请先连接本地事件服务".into(),
    })
}

async fn operate(
    state: WebState,
    headers: HeaderMap,
    operation: Operation,
) -> Result<Json<Value>, ApiError> {
    let (client, cancellation) = client(&state, &headers)?;
    let key = headers
        .get("idempotency-key")
        .map(|value| {
            value
                .to_str()
                .ok()
                .filter(|key| !key.is_empty() && key.len() <= 128)
                .map(str::to_owned)
                .ok_or_else(|| ApiError {
                    status: StatusCode::BAD_REQUEST,
                    code: "invalid_idempotency_key",
                    message: "幂等键须为 1–128 个字符".into(),
                })
        })
        .transpose()?;
    Ok(Json(
        state
            .app
            .call_scoped(client, operation, cancellation, key)
            .await?,
    ))
}

async fn status(State(state): State<WebState>) -> Json<Snapshot> {
    Json(state.app.snapshots.borrow().clone())
}

async fn debug(State(state): State<WebState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(state.app.diagnostics().await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Name {
    display_name: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Join {
    join_code: String,
    display_name: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Subscribe {
    share_id: String,
    generation: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Offer {
    sdp: String,
}

async fn create_room(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(body): Json<Name>,
) -> Result<Json<Value>, ApiError> {
    operate(
        state,
        headers,
        Operation::Create {
            name: body.display_name,
        },
    )
    .await
}
async fn join_room(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(body): Json<Join>,
) -> Result<Json<Value>, ApiError> {
    operate(
        state,
        headers,
        Operation::Join {
            code: body.join_code,
            name: body.display_name,
        },
    )
    .await
}
async fn leave_room(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    operate(state, headers, Operation::Leave).await
}
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ShareOptions {
    #[serde(default)]
    profile: VideoProfile,
    #[serde(default)]
    audio: bool,
}
async fn start_share(
    State(state): State<WebState>,
    headers: HeaderMap,
    body: Option<Json<ShareOptions>>,
) -> Result<Json<Value>, ApiError> {
    let body = body.map(|Json(body)| body).unwrap_or_default();
    operate(
        state,
        headers,
        Operation::StartShare {
            profile: body.profile,
            audio: body.audio,
        },
    )
    .await
}
async fn update_share(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(profile): Json<VideoProfile>,
) -> Result<Json<Value>, ApiError> {
    operate(state, headers, Operation::UpdateShare { id, profile }).await
}
async fn stop_share(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    operate(state, headers, Operation::StopShare { id }).await
}
async fn subscribe(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(body): Json<Subscribe>,
) -> Result<Json<Value>, ApiError> {
    operate(
        state,
        headers,
        Operation::Subscribe {
            share_id: body.share_id,
            generation: body.generation,
        },
    )
    .await
}
async fn unsubscribe(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    operate(state, headers, Operation::Unsubscribe { id }).await
}
async fn playing(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    operate(state, headers, Operation::Playing { id }).await
}
async fn offer(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Offer>,
) -> Result<Json<Value>, ApiError> {
    operate(
        state,
        headers,
        Operation::Answer {
            peer_id: id,
            sdp: body.sdp,
        },
    )
    .await
}

pub(super) fn routes() -> Router<WebState> {
    Router::new()
        .route("/status", get(status))
        .route("/stats", get(status))
        .route("/debug", get(debug))
        .route("/room", post(create_room).delete(leave_room))
        .route("/room/join", post(join_room))
        .route("/shares", post(start_share))
        .route("/shares/{id}", axum::routing::delete(stop_share))
        .route("/shares/{id}/quality", post(update_share))
        .route("/subscriptions", post(subscribe))
        .route("/subscriptions/{id}", axum::routing::delete(unsubscribe))
        .route("/subscriptions/{id}/ready", post(playing))
        .route("/rtc/{id}/offer", post(offer))
        .route("/events", get(events))
}
