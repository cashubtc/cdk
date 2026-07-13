use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cdk_common::federation::{FederationConfig, MemberId};
use reqwest::Client;
use serde_json::Value;
use tokio::sync::mpsc;

use crate::app::AppEvent;

#[derive(Debug, Clone)]
pub struct MemberProbe {
    pub generation: u64,
    pub member_id: MemberId,
    pub success: bool,
    pub metadata_matches: bool,
    pub status: Option<u16>,
    pub latency_ms: u64,
    pub response_bytes: usize,
    pub mint_name: Option<String>,
    pub version: Option<String>,
    pub advertised_member_id: Option<u16>,
    pub advertised_federation_id: Option<String>,
    pub advertised_config_digest: Option<String>,
    pub active_keysets: usize,
    pub error: Option<String>,
    pub observed_at: u64,
}

#[derive(Debug)]
struct HttpObservation {
    status: Option<u16>,
    latency_ms: u64,
    bytes: Vec<u8>,
    error: Option<String>,
}

pub fn spawn_member_monitors(
    config: FederationConfig,
    tx: mpsc::UnboundedSender<AppEvent>,
    generation: u64,
) {
    let expected_federation_id = config.federation_id.to_string();
    let expected_digest = config
        .config_digest_for_validated_config()
        .map(|digest| digest.to_string())
        .unwrap_or_default();
    let client = match Client::builder()
        .connect_timeout(Duration::from_secs(4))
        .timeout(Duration::from_secs(8))
        .user_agent("fed-cdk-tui/0.1")
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            tx.send(AppEvent::BackgroundError(format!(
                "could not create telemetry client: {err}"
            )))
            .ok();
            return;
        }
    };

    for (offset, (member_id, member)) in config.members.into_iter().enumerate() {
        let client = client.clone();
        let tx = tx.clone();
        let federation_id = expected_federation_id.clone();
        let config_digest = expected_digest.clone();
        let url = member.public_mint_url.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis((offset as u64) * 120)).await;
            let mut interval = tokio::time::interval(Duration::from_secs(3));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                let probe = probe_member(
                    &client,
                    member_id,
                    &url,
                    &federation_id,
                    &config_digest,
                    generation,
                )
                .await;
                if tx.send(AppEvent::MemberProbe(probe)).is_err() {
                    return;
                }
            }
        });
    }
}

async fn probe_member(
    client: &Client,
    member_id: MemberId,
    base_url: &str,
    expected_federation_id: &str,
    expected_digest: &str,
    generation: u64,
) -> MemberProbe {
    let base_url = base_url.trim_end_matches('/');
    let info_url = format!("{base_url}/v1/info");
    let keysets_url = format!("{base_url}/v1/keysets");
    let (info, keysets) = tokio::join!(fetch(client, &info_url), fetch(client, &keysets_url));

    let info_json = serde_json::from_slice::<Value>(&info.bytes).ok();
    let keysets_json = serde_json::from_slice::<Value>(&keysets.bytes).ok();
    let federation = info_json.as_ref().and_then(|value| value.get("federation"));
    let advertised_member_id = federation
        .and_then(|value| value.get("member_id"))
        .and_then(Value::as_u64)
        .and_then(|value| u16::try_from(value).ok());
    let advertised_federation_id = federation
        .and_then(|value| value.get("federation_id"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let advertised_config_digest = federation
        .and_then(|value| value.get("config_digest"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let metadata_matches = advertised_member_id == Some(member_id.get())
        && advertised_federation_id.as_deref() == Some(expected_federation_id)
        && advertised_config_digest.as_deref() == Some(expected_digest);
    let active_keysets = keysets_json
        .as_ref()
        .and_then(|value| value.get("keysets"))
        .and_then(Value::as_array)
        .map(|keysets| {
            keysets
                .iter()
                .filter(|keyset| {
                    keyset
                        .get("active")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or_default();

    let status_success = info
        .status
        .is_some_and(|status| (200..300).contains(&status))
        && keysets
            .status
            .is_some_and(|status| (200..300).contains(&status));
    let error = info.error.clone().or(keysets.error.clone()).or_else(|| {
        (!metadata_matches && status_success)
            .then(|| "member federation metadata does not match imported roster".to_string())
    });

    MemberProbe {
        generation,
        member_id,
        success: status_success && metadata_matches,
        metadata_matches,
        status: info.status,
        latency_ms: info.latency_ms.max(keysets.latency_ms),
        response_bytes: info.bytes.len() + keysets.bytes.len(),
        mint_name: info_json
            .as_ref()
            .and_then(|value| value.get("name"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        version: info_json
            .as_ref()
            .and_then(|value| value.get("version"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        advertised_member_id,
        advertised_federation_id,
        advertised_config_digest,
        active_keysets,
        error,
        observed_at: unix_time(),
    }
}

async fn fetch(client: &Client, url: &str) -> HttpObservation {
    let started = Instant::now();
    match client.get(url).send().await {
        Ok(response) => {
            let status = response.status().as_u16();
            match response.bytes().await {
                Ok(bytes) => HttpObservation {
                    status: Some(status),
                    latency_ms: elapsed_ms(started),
                    bytes: bytes.to_vec(),
                    error: (!(200..300).contains(&status)).then(|| format!("HTTP {status}")),
                },
                Err(err) => HttpObservation {
                    status: Some(status),
                    latency_ms: elapsed_ms(started),
                    bytes: Vec::new(),
                    error: Some(err.to_string()),
                },
            }
        }
        Err(err) => HttpObservation {
            status: err.status().map(|status| status.as_u16()),
            latency_ms: elapsed_ms(started),
            bytes: Vec::new(),
            error: Some(err.to_string()),
        },
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}
