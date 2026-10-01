use std::{str::FromStr, time::Duration};

use anyhow::{Context, Result};
use iroh::{
    Endpoint, EndpointAddr,
    address_lookup::{EndpointInfo, N0_DNS_PKARR_RELAY_PROD, PkarrRelayClient, UserData},
};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::protocol::RoomCode;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone)]
pub enum Discovery {
    Public(PkarrRelayClient),
    #[cfg(test)]
    Memory(std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, EndpointAddr>>>),
}

impl Discovery {
    pub fn public(endpoint: &Endpoint) -> Result<Self> {
        Ok(Self::Public(PkarrRelayClient::new(
            Url::parse(N0_DNS_PKARR_RELAY_PROD)?,
            endpoint.tls_config().clone(),
            endpoint.dns_resolver()?.clone(),
        )))
    }

    pub async fn publish(&self, code: &RoomCode, endpoint: &Endpoint) -> Result<()> {
        match self {
            Self::Public(client) => {
                let key = code.discovery_key();
                let data =
                    UserData::from_str(&endpoint.id().to_z32()).context("编码房间入口失败")?;
                let info = EndpointInfo::from_parts(key.public(), endpoint.addr().into())
                    .with_user_data(Some(data));
                let packet = info
                    .to_pkarr_signed_packet(&key, 120)
                    .context("签名房间入口失败")?;
                tokio::time::timeout(REQUEST_TIMEOUT, client.publish(&packet))
                    .await
                    .context("发布房间入口超时")??;
            }
            #[cfg(test)]
            Self::Memory(records) => {
                records
                    .lock()
                    .map_err(|_| anyhow::anyhow!("测试发现状态不可用"))?
                    .insert(code.id(), endpoint.addr());
            }
        }
        Ok(())
    }

    pub async fn resolve(&self, code: &RoomCode) -> Result<EndpointAddr> {
        match self {
            Self::Public(client) => {
                let packet = tokio::time::timeout(
                    REQUEST_TIMEOUT,
                    client.resolve(code.discovery_key().public()),
                )
                .await
                .context("查找房间入口超时")??;
                let info =
                    EndpointInfo::from_pkarr_signed_packet(&packet).context("房间入口无效")?;
                let data = info.user_data().context("房间入口不存在")?;
                let id = iroh::EndpointId::from_z32(data.as_ref()).context("房主节点标识无效")?;
                Ok(EndpointAddr::from(id).with_addrs(info.addrs().cloned()))
            }
            #[cfg(test)]
            Self::Memory(records) => records
                .lock()
                .map_err(|_| anyhow::anyhow!("测试发现状态不可用"))?
                .get(&code.id())
                .cloned()
                .context("房间入口不存在"),
        }
    }

    pub async fn refresh(self, code: RoomCode, endpoint: Endpoint, cancel: CancellationToken) {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        interval.tick().await;
        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => break,
                _ = interval.tick() => {
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => break,
                        result = self.publish(&code, &endpoint) => {
                            if let Err(error) = result { tracing::warn!(%error, "刷新房间入口失败"); }
                        }
                    }
                }
            }
        }
    }

    #[cfg(test)]
    pub fn forget(&self, code: &RoomCode) {
        if let Self::Memory(records) = self
            && let Ok(mut records) = records.lock()
        {
            records.remove(&code.id());
        }
    }
}
