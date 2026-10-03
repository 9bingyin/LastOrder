use futures_util::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::protocol::{FileState, SharedFile};

use super::*;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Offer {
    room_id: String,
    recipient_id: String,
    name: String,
    size: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FileRoom {
    room_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Change {
    room_id: String,
    state: FileState,
}

async fn offer(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(body): Json<Offer>,
) -> Result<Json<Value>, ApiError> {
    let (client, cancellation) = api::client(&state, &headers)?;
    let value = state
        .app
        .call_scoped(
            client,
            Operation::OfferFile {
                room_id: body.room_id,
                recipient_id: body.recipient_id,
                name: body.name,
                size: body.size,
            },
            cancellation.clone(),
            None,
        )
        .await?;
    let file: SharedFile = serde_json::from_value(value).map_err(anyhow::Error::from)?;
    Ok(Json(json!(state.app.wait_file(&file, cancellation).await?)))
}

async fn change(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Change>,
) -> Result<Json<Value>, ApiError> {
    let (client, cancellation) = api::client(&state, &headers)?;
    let value = state
        .app
        .call_scoped(
            client,
            Operation::ChangeFile {
                room_id: body.room_id,
                id,
                state: body.state,
            },
            cancellation.clone(),
            None,
        )
        .await?;
    let file: SharedFile = serde_json::from_value(value).map_err(anyhow::Error::from)?;
    Ok(Json(json!(state.app.wait_file(&file, cancellation).await?)))
}

async fn upload(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::extract::Query(query): axum::extract::Query<FileRoom>,
    body: Body,
) -> Result<Json<Value>, ApiError> {
    let (client, cancellation) = api::client(&state, &headers)?;
    let offer = state.app.shared_file(&query.room_id, &id)?;
    if offer.publisher_id != state.app.endpoint.id().to_string()
        || offer.state != FileState::Accepted
    {
        return Err(anyhow::anyhow!("必须等待指定接收者确认").into());
    }
    if state.app.snapshots.borrow().file_clients.get(&id) != Some(&client) {
        return Err(anyhow::anyhow!("请在发送邀请的页面准备文件").into());
    }
    if headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|size| size != offer.size)
    {
        return Err(anyhow::anyhow!("文件大小与邀请信息不一致").into());
    }
    let staged = state.app.files.staging_file()?;
    let mut output = tokio::fs::File::from_std(staged.reopen().map_err(anyhow::Error::from)?);
    let mut stream = body.into_data_stream();
    let mut size = 0u64;
    let mut snapshots = state.app.snapshots.clone();
    let receive = async {
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(anyhow::Error::from)?;
            size = size
                .checked_add(u64::try_from(chunk.len()).map_err(anyhow::Error::from)?)
                .ok_or_else(|| anyhow::anyhow!("文件大小无法表示"))?;
            if size > offer.size {
                return Err(anyhow::anyhow!("文件大小与邀请信息不一致"));
            }
            output.write_all(&chunk).await?;
        }
        output.flush().await?;
        if size != offer.size {
            return Err(anyhow::anyhow!("文件上传不完整"));
        }
        Ok::<(), anyhow::Error>(())
    };
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => return Err(anyhow::anyhow!("页面会话已断开").into()),
        _ = state.app.shutdown.cancelled() => return Err(anyhow::anyhow!("应用已停止").into()),
        _ = snapshots.wait_for(|snapshot| snapshot.files.get(&id) != Some(&offer)) => return Err(anyhow::anyhow!("文件邀请已变化").into()),
        result = receive => result?,
    }
    drop(output);
    let file = state
        .app
        .prepare_file(client, query.room_id, id, staged.path(), cancellation)
        .await?;
    Ok(Json(json!(file)))
}

async fn download(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::extract::Query(query): axum::extract::Query<FileRoom>,
) -> Result<Response, ApiError> {
    let (client, cancellation) = api::client(&state, &headers)?;
    let entry = state
        .app
        .download_file(&client, &query.room_id, &id, cancellation.clone())
        .await?;
    let name: String = entry
        .file
        .name
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect();
    let size = entry.file.size;
    let mut reader = state.app.files.store.blobs().reader(entry.tag.hash());
    let shutdown = state.app.shutdown.clone();
    let stream = async_stream::stream! {
        let mut buffer = vec![0; 64 * 1024];
        loop {
            let count = tokio::select! {
                biased;
                _ = cancellation.cancelled() => Err(std::io::Error::other("页面会话已断开")),
                _ = entry.cancellation.cancelled() => Err(std::io::Error::other("文件传输已结束")),
                _ = shutdown.cancelled() => Err(std::io::Error::other("应用已停止")),
                result = reader.read(&mut buffer) => result,
            };
            match count {
                Ok(0) => break,
                Ok(count) => yield Ok(bytes::Bytes::copy_from_slice(&buffer[..count])),
                Err(error) => { yield Err(error); break; }
            }
        }
    };
    Response::builder()
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, size)
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename*=UTF-8''{name}"),
        )
        .body(Body::from_stream(stream))
        .map_err(anyhow::Error::from)
        .map_err(ApiError::from)
}

pub(super) fn routes() -> Router<WebState> {
    Router::new()
        .route("/files", post(offer))
        .route("/files/{id}", post(change).get(download))
        .route(
            "/files/{id}/content",
            post(upload).layer(DefaultBodyLimit::disable()),
        )
}
