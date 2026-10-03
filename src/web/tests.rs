use std::{net::Ipv4Addr, time::Duration};

use anyhow::{Context, Result};
use iroh::{Endpoint, RelayMode, endpoint::presets};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use super::*;
use crate::net::discovery::Discovery;

async fn get(port: u16, path: &str, headers: &str) -> Result<(u16, String, Value)> {
    let (status, head, body) = http(port, "GET", path, headers, &[]).await?;
    Ok((status, head, serde_json::from_slice(&body)?))
}

async fn http(
    port: u16,
    method: &str,
    path: &str,
    headers: &str,
    body: &[u8],
) -> Result<(u16, String, Vec<u8>)> {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await?;
        let length = if headers.to_ascii_lowercase().contains("content-length:") {
            String::new()
        } else {
            format!("Content-Length: {}\r\n", body.len())
        };
        socket
            .write_all(
                format!("{method} {path} HTTP/1.1\r\nConnection: close\r\n{length}{headers}\r\n")
                    .as_bytes(),
            )
            .await?;
        socket
            .write_all(body)
            .await
            .with_context(|| format!("{method} {path}: 请求体写入失败"))?;
        let mut bytes = Vec::new();
        socket.read_to_end(&mut bytes).await?;
        let boundary = bytes
            .windows(4)
            .position(|bytes| bytes == b"\r\n\r\n")
            .context("HTTP 响应不完整")?;
        let head = std::str::from_utf8(&bytes[..boundary])?;
        let body = &bytes[boundary + 4..];
        let status = head
            .split_whitespace()
            .nth(1)
            .context("缺少 HTTP 状态")?
            .parse()?;
        Ok((status, head.to_ascii_lowercase(), body.to_vec()))
    })
    .await
    .context("HTTP 测试超时")?
}

#[tokio::test]
async fn http_access_requires_credentials_and_exact_local_origin() -> Result<()> {
    let endpoint = Endpoint::builder(presets::Minimal)
        .bind_addr((Ipv4Addr::LOCALHOST, 0))?
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await?;
    let tasks = TaskTracker::new();
    let shutdown = CancellationToken::new();
    let app = Handle::spawn(
        endpoint.clone(),
        tasks.clone(),
        shutdown.clone(),
        Discovery::Memory(Default::default()),
    )
    .await?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let port = listener.local_addr()?.port();
    let files = app.files.clone();
    let state = WebState::new(app, "test-token".into(), port);
    let server_shutdown = shutdown.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, router(state, None))
            .with_graceful_shutdown(server_shutdown.cancelled_owned())
            .await
    });
    let result: Result<()> = async {
        let host = format!("Host: 127.0.0.1:{port}\r\n");
        let auth = "Authorization: Bearer test-token\r\n";
        let cases = [
            (host.clone(), 403),
            (format!("{host}Authorization: Bearer wrong-token\r\n"), 403),
            (format!("{host}{auth}Origin: https://evil.example\r\n"), 403),
            (format!("Host: evil.example\r\n{auth}"), 403),
            (auth.to_owned(), 403),
            (format!("{host}{auth}Sec-Fetch-Site: cross-site\r\n"), 403),
            (format!("{host}{auth}Sec-Fetch-Site: same-site\r\n"), 403),
            (format!("{host}{auth}"), 200),
            (
                format!(
                    "{host}{auth}Origin: http://127.0.0.1:{port}\r\nSec-Fetch-Site: same-origin\r\n"
                ),
                200,
            ),
            (format!("{host}{auth}Sec-Fetch-Site: none\r\n"), 200),
        ];
        for path in ["/api/v1/status", "/api/v1/debug"] {
            for (headers, expected) in &cases {
                let (status, head, body) = get(port, path, headers).await?;
                assert_eq!(status, *expected, "{path}: {headers}");
                if status == 403 {
                    assert_eq!(body["error"]["code"], "permission_denied");
                } else {
                    assert!(head.contains("cache-control: no-store"));
                    assert!(head.contains("x-content-type-options: nosniff"));
                    assert!(head.contains("referrer-policy: no-referrer"));
                    assert!(head.contains("frame-ancestors 'none'"));
                    if path.ends_with("status") {
                        assert!(body["room"].is_null());
                    } else {
                        assert!(body["connections"].is_array());
                    }
                }
            }
        }
        Ok(())
    }
    .await;
    shutdown.cancel();
    tasks.close();
    tasks.wait().await;
    endpoint.close().await;
    server.await??;
    files.shutdown().await?;
    result
}

#[tokio::test]
async fn file_http_invites_before_acceptance_then_streams_verified_binary_content() -> Result<()> {
    let tasks = TaskTracker::new();
    let shutdown = CancellationToken::new();
    let discovery = Discovery::Memory(Default::default());
    let assets = tempfile::tempdir()?;
    tokio::fs::write(assets.path().join("index.html"), b"files-page").await?;
    let mut nodes = Vec::new();
    for _ in 0..2 {
        let endpoint = Endpoint::builder(presets::Minimal)
            .bind_addr((Ipv4Addr::LOCALHOST, 0))?
            .relay_mode(RelayMode::Disabled)
            .bind()
            .await?;
        let app = Handle::spawn(
            endpoint.clone(),
            tasks.clone(),
            shutdown.clone(),
            discovery.clone(),
        )
        .await?;
        let protocol = crate::net::router(&endpoint, app.clone());
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let port = listener.local_addr()?.port();
        let state = WebState::new(app.clone(), "test-token".into(), port);
        state
            .clients
            .lock()
            .map_err(|_| anyhow::anyhow!("客户端表不可用"))?
            .insert("page".into(), CancellationToken::new());
        let stopped = shutdown.clone();
        let path = assets.path().to_path_buf();
        let server = tokio::spawn(async move {
            axum::serve(listener, router(state, Some(path)))
                .with_graceful_shutdown(stopped.cancelled_owned())
                .await
        });
        nodes.push((endpoint, app, protocol, port, server));
    }
    let result: Result<()> = async {
        let (_, sender, _, source_port, _) = &nodes[0];
        let (target_endpoint, receiver, _, target_port, _) = &nodes[1];
        sender.call("page".into(), Operation::Create { name: "发送者".into() }).await?;
        let room = sender.snapshots.borrow().join_code.clone().context("房间不存在")?;
        receiver.call("page".into(), Operation::Join { code: room.clone(), name: "接收者".into() }).await?;
        let source_headers = format!("Host: 127.0.0.1:{source_port}\r\nAuthorization: Bearer test-token\r\nX-Client-Id: page\r\nContent-Type: application/json\r\n");
        let target_headers = format!("Host: 127.0.0.1:{target_port}\r\nAuthorization: Bearer test-token\r\nX-Client-Id: page\r\nContent-Type: application/json\r\n");
        let (status, _, body) = http(*source_port, "GET", "/files", &format!("Host: 127.0.0.1:{source_port}\r\n"), &[]).await?;
        assert_eq!((status, body.as_slice()), (200, b"files-page".as_slice()));
        let payload: Vec<u8> = (0..128 * 1024).map(|index| (index % 251) as u8).collect();
        let metadata = json!({ "roomId": room, "recipientId": target_endpoint.id().to_string(), "name": "测试 data.bin", "size": payload.len() });
        let no_client = source_headers.replace("X-Client-Id: page\r\n", "");
        assert_eq!(http(*source_port, "POST", "/api/v1/files", &no_client, &serde_json::to_vec(&metadata)?).await?.0, 401);
        let mut invalid = metadata.clone(); invalid["name"] = json!("../data.bin");
        assert_eq!(http(*source_port, "POST", "/api/v1/files", &source_headers, &serde_json::to_vec(&invalid)?).await?.0, 400);
        let (status, _, body) = http(*source_port, "POST", "/api/v1/files", &source_headers, &serde_json::to_vec(&metadata)?).await?;
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
        let file: crate::protocol::SharedFile = serde_json::from_slice(&body)?;
        assert!(file.blob_ticket.is_none());
        let path = format!("/api/v1/files/{}?roomId={room}", file.id);
        let content = format!("/api/v1/files/{}/content?roomId={room}", file.id);
        let mut snapshots = receiver.snapshots.clone();
        tokio::time::timeout(Duration::from_secs(5), snapshots.wait_for(|state| state.files.contains_key(&file.id))).await??;
        let upload_headers = format!("{source_headers}Content-Length: {}\r\n", payload.len());
        assert_eq!(http(*source_port, "POST", &content, &upload_headers, &[]).await?.0, 400, "确认前应在读取文件内容前拒绝请求");
        assert_eq!(http(*target_port, "GET", &path, &target_headers, &[]).await?.0, 403);
        let (status, _, body) = http(*target_port, "POST", &format!("/api/v1/files/{}", file.id), &target_headers, &serde_json::to_vec(&json!({"roomId": room, "state": "accepted"}))?).await?;
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
        let accepted: crate::protocol::SharedFile = serde_json::from_slice(&body)?;
        sender.wait_file(&accepted, CancellationToken::new()).await?;
        assert_eq!(http(*source_port, "POST", &content, &source_headers, &payload).await?.0, 200);
        tokio::time::timeout(Duration::from_secs(5), snapshots.wait_for(|state| state.files.get(&file.id).is_some_and(|file| file.state == crate::protocol::FileState::Ready))).await??;
        let (status, head, body) = http(*target_port, "GET", &path, &target_headers, &[]).await?;
        assert_eq!(status, 200);
        assert!(head.contains("content-disposition: attachment; filename*=utf-8''%e6%b5%8b%e8%af%95%20data.bin"));
        assert!(head.contains("x-content-type-options: nosniff"));
        assert_eq!(body, payload);
        assert_eq!(http(*target_port, "POST", &format!("/api/v1/files/{}", file.id), &target_headers, &serde_json::to_vec(&json!({"roomId": room, "state": "completed"}))?).await?.0, 200);
        assert_eq!(http(*target_port, "GET", &path, &target_headers, &[]).await?.0, 403);
        Ok(())
    }.await;
    shutdown.cancel();
    tasks.close();
    tasks.wait().await;
    for (endpoint, _, protocol, _, server) in nodes {
        server.await??;
        protocol.shutdown().await?;
        endpoint.close().await;
    }
    result
}
