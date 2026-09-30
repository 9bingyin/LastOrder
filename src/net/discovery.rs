use std::str::FromStr;

use anyhow::{Context, Result, anyhow};
use iroh::{
    Endpoint, EndpointId, SecretKey,
    address_lookup::{EndpointInfo, N0_DNS_PKARR_RELAY_PROD, PkarrRelayClient, UserData},
};
use iroh_gossip::TopicId;
use url::Url;

pub(super) fn discovery_client(endpoint: &Endpoint) -> Result<PkarrRelayClient> {
    let relay_url = Url::parse(N0_DNS_PKARR_RELAY_PROD).context("invalid discovery relay url")?;
    Ok(PkarrRelayClient::new(
        relay_url,
        endpoint.tls_config().clone(),
        endpoint.dns_resolver()?.clone(),
    ))
}

pub(super) async fn publish_self(
    relay: &PkarrRelayClient,
    network_key: &SecretKey,
    endpoint_id: EndpointId,
) -> Result<()> {
    let user_data = UserData::from_str(&endpoint_id.to_z32())
        .map_err(|error| anyhow!("endpoint id does not fit discovery record: {error}"))?;
    let info = EndpointInfo::new(network_key.public()).with_user_data(Some(user_data));
    let packet = info
        .to_pkarr_signed_packet(network_key, 30)
        .map_err(|error| anyhow!("failed to encode discovery record: {error}"))?;
    relay.publish(&packet).await?;
    Ok(())
}

pub(super) async fn lookup_bootstrap(
    relay: &PkarrRelayClient,
    network_key: &SecretKey,
) -> Result<Option<EndpointId>> {
    let packet = match relay.resolve(network_key.public()).await {
        Ok(packet) => packet,
        Err(error) => {
            if error.to_string().contains("404") {
                return Ok(None);
            }
            return Err(error.into());
        }
    };
    let info = EndpointInfo::from_pkarr_signed_packet(&packet)
        .map_err(|error| anyhow!("failed to decode discovery record: {error}"))?;
    let Some(user_data) = info.user_data() else {
        return Ok(None);
    };
    let endpoint_id = EndpointId::from_z32(user_data.as_ref())
        .map_err(|error| anyhow!("invalid endpoint id in discovery record: {error}"))?;
    Ok(Some(endpoint_id))
}

pub(super) fn topic_id(network_id: &str) -> TopicId {
    TopicId::from_bytes(blake3::derive_key(
        "lastorder.topic.v1",
        network_id.as_bytes(),
    ))
}

pub(super) fn network_secret(network_id: &str) -> SecretKey {
    SecretKey::from_bytes(&blake3::derive_key(
        "lastorder.network.v1",
        network_id.as_bytes(),
    ))
}

pub fn generate_network_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).context("failed to generate network id")?;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let mut network_id = String::with_capacity(hex.len() + hex.len() / 4);
    for (index, character) in hex.chars().enumerate() {
        if index > 0 && index.is_multiple_of(4) {
            network_id.push('-');
        }
        network_id.push(character);
    }
    Ok(network_id)
}

#[cfg(test)]
mod tests {
    use anyhow::Result;

    use super::{generate_network_id, network_secret, topic_id};

    #[test]
    fn generated_network_id_is_grouped_hex() -> Result<()> {
        let network_id = generate_network_id()?;
        let parts: Vec<_> = network_id.split('-').collect();
        assert_eq!(parts.len(), 8);
        assert!(parts.iter().all(|part| part.len() == 4));
        Ok(())
    }

    #[test]
    fn network_id_derivation_is_stable() {
        assert_eq!(topic_id("alpha"), topic_id("alpha"));
        assert_ne!(topic_id("alpha"), topic_id("beta"));
        assert_eq!(
            network_secret("alpha").public(),
            network_secret("alpha").public()
        );
        assert_ne!(
            network_secret("alpha").public(),
            network_secret("beta").public()
        );
    }
}
