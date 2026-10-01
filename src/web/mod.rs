mod access;
mod api;
mod events;
#[cfg(test)]
mod tests;

use access::{authorize, local_request};

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json, Router,
    body::Body,
    extract::{
        DefaultBodyLimit, Path, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, Request, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use rust_embed::RustEmbed;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tower_http::{
    services::{ServeDir, ServeFile},
    set_header::SetResponseHeaderLayer,
};

use crate::{
    app::{Fault, Handle, Operation},
    protocol::{Snapshot, VideoProfile, random_id, secret_matches},
};

#[derive(RustEmbed)]
#[folder = "frontend/dist/"]
struct EmbeddedAssets;

async fn embedded_handler(uri: axum::http::Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if !path.is_empty()
        && let Some(file) = EmbeddedAssets::get(path)
    {
        return (
            [(header::CONTENT_TYPE, file.metadata.mimetype())],
            file.data,
        )
            .into_response();
    }
    match EmbeddedAssets::get("index.html") {
        Some(file) => (
            [(header::CONTENT_TYPE, file.metadata.mimetype())],
            file.data,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Clone)]
pub struct WebState {
    pub app: Handle,
    pub token: String,
    pub authority: String,
    clients: Arc<Mutex<HashMap<String, CancellationToken>>>,
}

impl WebState {
    pub fn new(app: Handle, token: String, port: u16) -> Self {
        Self {
            app,
            token,
            authority: format!("127.0.0.1:{port}"),
            clients: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

pub fn router(state: WebState, frontend: Option<PathBuf>) -> Router {
    let api = api::routes()
        .layer(DefaultBodyLimit::max(64 * 1024))
        .route_layer(middleware::from_fn_with_state(state.clone(), authorize));
    let app = Router::new().nest("/api/v1", api);
    let app = if let Some(dir) = frontend {
        app.route_service("/debug", ServeFile::new(dir.join("index.html")))
            .fallback_service(ServeDir::new(dir))
    } else {
        app.fallback(embedded_handler)
    };
    app.layer(SetResponseHeaderLayer::if_not_present(header::CACHE_CONTROL, header::HeaderValue::from_static("no-store")))
        .layer(SetResponseHeaderLayer::if_not_present(header::X_CONTENT_TYPE_OPTIONS, header::HeaderValue::from_static("nosniff")))
        .layer(SetResponseHeaderLayer::if_not_present(header::HeaderName::from_static("referrer-policy"), header::HeaderValue::from_static("no-referrer")))
        .layer(SetResponseHeaderLayer::if_not_present(header::HeaderName::from_static("content-security-policy"), header::HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; media-src 'self' blob:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'")))
        .layer(middleware::from_fn_with_state(state.clone(), local_request))
        .with_state(state)
}

struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({"error": {"code": self.code, "message": self.message, "retryable": self.status.is_server_error()}}))).into_response()
    }
}
impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        if let Some(fault) = error.downcast_ref::<Fault>() {
            Self {
                status: StatusCode::from_u16(fault.status).unwrap_or(StatusCode::BAD_REQUEST),
                code: fault.code,
                message: fault.message.clone(),
            }
        } else {
            tracing::warn!(%error, "本地操作失败");
            Self {
                status: StatusCode::BAD_REQUEST,
                code: "operation_failed",
                message: error.to_string(),
            }
        }
    }
}
