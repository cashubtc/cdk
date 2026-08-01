use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cdk::mint_url::MintUrl;
use cdk::nuts::MintInfo;
use cdk::wallet::HttpTransport;
use cdk_common::federation::{FederationConfig, MemberId};
use cdk_http_client::HttpError;
use serde_json::Value;
use tokio::sync::mpsc;

use crate::app::AppEvent;
use crate::wallet::WalletTransport;

#[derive(Debug, Clone)]
pub struct MemberProbe {
    pub generation: u64,
    pub member_id: MemberId,
    pub member_url: String,
    pub success: bool,
    pub metadata_matches: bool,
    pub status: Option<u16>,
    pub latency_ms: u64,
    pub response_bytes: usize,
    pub mint_name: Option<String>,
    pub version: Option<String>,
    pub mint_methods: Vec<String>,
    pub melt_methods: Vec<String>,
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
    transport: WalletTransport,
    tx: mpsc::UnboundedSender<AppEvent>,
    generation: u64,
    current_generation: Arc<AtomicU64>,
) {
    let expected_federation_id = config.federation_id.to_string();
    let expected_digest = config
        .config_digest_for_validated_config()
        .map(|digest| digest.to_string())
        .unwrap_or_default();
    for (offset, (member_id, member)) in config.members.into_iter().enumerate() {
        let transport = transport.clone();
        let tx = tx.clone();
        let federation_id = expected_federation_id.clone();
        let config_digest = expected_digest.clone();
        let current_generation = current_generation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis((offset as u64) * 120)).await;
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                if current_generation.load(Ordering::Relaxed) != generation {
                    return;
                }
                let probe = probe_member(
                    &transport,
                    member_id,
                    &member.public_mint_url,
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
    transport: &WalletTransport,
    member_id: MemberId,
    base_url: &MintUrl,
    expected_federation_id: &str,
    expected_digest: &str,
    generation: u64,
) -> MemberProbe {
    let base_url_string = base_url.to_string();
    let info_url = base_url.join_paths(&["v1", "info"]);
    let keysets_url = base_url.join_paths(&["v1", "keysets"]);
    let (info, keysets) = match (info_url, keysets_url) {
        (Ok(info_url), Ok(keysets_url)) => {
            tokio::join!(fetch(transport, info_url), fetch(transport, keysets_url))
        }
        (Err(err), _) | (_, Err(err)) => {
            let observation = HttpObservation {
                status: None,
                latency_ms: 0,
                bytes: Vec::new(),
                error: Some(err.to_string()),
            };
            (
                observation,
                HttpObservation {
                    status: None,
                    latency_ms: 0,
                    bytes: Vec::new(),
                    error: Some(err.to_string()),
                },
            )
        }
    };

    let info_json = serde_json::from_slice::<Value>(&info.bytes).ok();
    let mint_info = serde_json::from_slice::<MintInfo>(&info.bytes).ok();
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
        member_url: base_url_string,
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
        mint_methods: mint_info
            .as_ref()
            .map(advertised_mint_methods)
            .unwrap_or_default(),
        melt_methods: mint_info
            .as_ref()
            .map(advertised_melt_methods)
            .unwrap_or_default(),
        advertised_member_id,
        advertised_federation_id,
        advertised_config_digest,
        active_keysets,
        error,
        observed_at: unix_time(),
    }
}

fn advertised_mint_methods(info: &MintInfo) -> Vec<String> {
    if info.nuts.nut04.disabled {
        return Vec::new();
    }
    info.nuts
        .nut04
        .methods
        .iter()
        .map(|settings| format!("{}/{}", settings.method, settings.unit))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn advertised_melt_methods(info: &MintInfo) -> Vec<String> {
    if info.nuts.nut05.disabled {
        return Vec::new();
    }
    info.nuts
        .nut05
        .methods
        .iter()
        .map(|settings| format!("{}/{}", settings.method, settings.unit))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

async fn fetch(transport: &WalletTransport, url: url::Url) -> HttpObservation {
    let started = Instant::now();
    match tokio::time::timeout(Duration::from_secs(8), transport.http_get_raw(url, None)).await {
        Ok(Ok(response)) => {
            let status = response.status();
            match response.bytes().await {
                Ok(bytes) => HttpObservation {
                    status: Some(status),
                    latency_ms: elapsed_ms(started),
                    bytes,
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
        Ok(Err(err)) => HttpObservation {
            status: match err {
                HttpError::Status { status, .. } => Some(status),
                _ => None,
            },
            latency_ms: elapsed_ms(started),
            bytes: Vec::new(),
            error: Some(err.to_string()),
        },
        Err(_) => HttpObservation {
            status: None,
            latency_ms: elapsed_ms(started),
            bytes: Vec::new(),
            error: Some("request timed out".to_string()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_methods_follow_enabled_mint_info_advertisements() {
        let info: MintInfo = serde_json::from_str(
            r#"{
                "nuts": {
                    "4": {
                        "methods": [
                            {
                                "method": "onchain",
                                "unit": "sat",
                                "options": {"confirmations": 1}
                            },
                            {"method": "bolt11", "unit": "sat"}
                        ],
                        "disabled": false
                    },
                    "5": {
                        "methods": [{"method": "onchain", "unit": "sat"}],
                        "disabled": false
                    }
                }
            }"#,
        )
        .expect("valid mint info");

        assert_eq!(
            advertised_mint_methods(&info),
            vec!["bolt11/sat".to_string(), "onchain/sat".to_string()]
        );
        assert_eq!(
            advertised_melt_methods(&info),
            vec!["onchain/sat".to_string()]
        );
    }
}
