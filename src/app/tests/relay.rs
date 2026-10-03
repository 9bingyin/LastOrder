use super::*;

#[tokio::test]
#[ignore = "需要访问 Iroh 公共发现和 relay；普通测试不依赖公网"]
async fn public_relay_transports_media_without_direct_udp() -> Result<()> {
    let relay_mode = match std::env::var("LASTORDER_TEST_RELAY") {
        Ok(url) => iroh::RelayMode::Custom([url.parse::<iroh::RelayUrl>()?].into_iter().collect()),
        Err(_) => iroh::RelayMode::Default,
    };
    let coordinator_endpoint = Endpoint::builder(presets::N0)
        .relay_mode(relay_mode.clone())
        .clear_ip_transports()
        .bind()
        .await?;
    let publisher_endpoint = Endpoint::builder(presets::N0)
        .relay_mode(relay_mode.clone())
        .clear_ip_transports()
        .bind()
        .await?;
    let viewer_endpoint = Endpoint::builder(presets::N0)
        .relay_mode(relay_mode)
        .clear_ip_transports()
        .bind()
        .await?;
    tokio::time::timeout(Duration::from_secs(20), async {
        tokio::join!(
            coordinator_endpoint.online(),
            publisher_endpoint.online(),
            viewer_endpoint.online()
        );
    })
    .await
    .context("连接公共中继超时")?;
    let coordinator_discovery = Discovery::public(&coordinator_endpoint)?;
    let publisher_discovery = Discovery::public(&publisher_endpoint)?;
    let viewer_discovery = Discovery::public(&viewer_endpoint)?;
    let coordinator = TestApp::with_discovery(coordinator_endpoint, coordinator_discovery).await?;
    let publisher = TestApp::with_discovery(publisher_endpoint, publisher_discovery).await?;
    let viewer = TestApp::with_discovery(viewer_endpoint, viewer_discovery).await?;
    let code = coordinator.create().await?;
    publisher.join(code.clone()).await?;
    viewer.join(code).await?;
    let capture: LocalMedia =
        serde_json::from_value(publisher.app.call("page".into(), start_share()).await?)?;
    live(&publisher, &capture).await?;
    subscribe(&coordinator, &capture).await?;
    subscribe(&viewer, &capture).await?;
    for payload_type in [96, 111] {
        assert_eq!(
            deliver(&publisher, &coordinator, &capture, payload_type)
                .await?
                .packet
                .header
                .payload_type,
            payload_type
        );
        assert_eq!(
            deliver(&publisher, &viewer, &capture, payload_type)
                .await?
                .packet
                .payload,
            Bytes::from_static(b"media payload")
        );
    }
    wait_state(&viewer.app, |state| {
        state
            .connection
            .as_ref()
            .is_some_and(|connection| connection.path == "relay")
    })
    .await?;
    viewer.close().await?;
    publisher.close().await?;
    coordinator.close().await
}
