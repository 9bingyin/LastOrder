mod app;
mod media;
mod net;
mod protocol;
mod web;

use std::{
    net::{Ipv4Addr, SocketAddrV4},
    path::PathBuf,
    time::Duration,
};

use anyhow::{Context, Result};
use clap::Parser;
use iroh::{Endpoint, endpoint::presets};
use tokio::net::TcpListener;
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    version,
    about = "基于 Iroh 的屏幕共享；所有用户运行 CLI，在本机浏览器操作"
)]
struct Args {
    #[arg(
        long,
        default_value_t = 0,
        help = "localhost HTTP 端口，0 表示自动选择"
    )]
    port: u16,
    #[arg(long, default_value = "frontend/dist", help = "Bun 构建后的前端目录")]
    frontend_dir: PathBuf,
    #[arg(long, help = "不自动打开浏览器")]
    no_open: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("lastorder=info")),
        )
        .init();
    let args = Args::parse();
    if !args.frontend_dir.join("index.html").is_file() {
        anyhow::bail!(
            "前端尚未构建。请先执行：cd frontend && bun install && bun run build；或使用 --frontend-dir 指定构建目录"
        );
    }
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, args.port))
        .await
        .context("绑定 localhost 端口失败")?;
    let port = listener.local_addr()?.port();
    let endpoint = tokio::time::timeout(Duration::from_secs(15), Endpoint::bind(presets::N0))
        .await
        .context("初始化 Iroh 超时")??;
    let shutdown = CancellationToken::new();
    let tasks = TaskTracker::new();
    let discovery = net::discovery::Discovery::public(&endpoint)?;
    let app = app::Handle::spawn(endpoint.clone(), tasks.clone(), shutdown.clone(), discovery);
    let router = net::router(&endpoint, app.clone());
    let token = protocol::secret()?;
    let url = format!("http://127.0.0.1:{port}/#token={token}");
    println!("本地 WebUI：{url}");
    println!("访问凭证仅供本机使用。按 Ctrl-C 退出。");
    if !args.no_open
        && let Err(error) = webbrowser::open(&url)
    {
        tracing::warn!(%error, "无法自动打开浏览器，请使用上方本地地址");
    }
    let state = web::WebState::new(app, token, port);
    let result = axum::serve(listener, web::router(state, args.frontend_dir))
        .with_graceful_shutdown({
            let shutdown = shutdown.clone();
            async move {
                tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = shutdown.cancelled() => {} }
                shutdown.cancel();
            }
        })
        .await;
    shutdown.cancel();
    tasks.close();
    let drained = tokio::time::timeout(Duration::from_secs(15), tasks.wait()).await;
    if drained.is_err() {
        tracing::warn!("后台任务退出超时");
    }
    let router_result = router.shutdown().await;
    endpoint.close().await;
    result.context("本地 Web 服务失败")?;
    router_result.context("Iroh 协议关闭失败")?;
    Ok(())
}
