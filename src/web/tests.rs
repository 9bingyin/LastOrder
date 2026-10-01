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
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await?;
        socket
            .write_all(
                format!("GET {path} HTTP/1.1\r\nConnection: close\r\n{headers}\r\n").as_bytes(),
            )
            .await?;
        let mut bytes = Vec::new();
        socket.read_to_end(&mut bytes).await?;
        let response = String::from_utf8(bytes)?;
        let (head, body) = response.split_once("\r\n\r\n").context("HTTP 响应不完整")?;
        let status = head
            .split_whitespace()
            .nth(1)
            .context("缺少 HTTP 状态")?
            .parse()?;
        Ok((
            status,
            head.to_ascii_lowercase(),
            serde_json::from_str(body)?,
        ))
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
    );
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let port = listener.local_addr()?.port();
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
    result
}
