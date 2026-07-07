use std::future::Future;
use std::hash::{Hash, Hasher};
use std::pin::Pin;
use std::sync::Arc;

use aleph_bft::{DataProvider, FinalizationHandler, SpawnHandle, TaskHandle};
use cdk::federation::{FederationAlephBftUnitPayload, FederationConsensusRuntime, FederationError};
use parity_scale_codec::{Decode, Encode, Error as CodecError, Input, Output};
use tokio::sync::mpsc;

/// AlephBFT data wrapper that preserves CDK's canonical consensus encoding.
#[derive(Debug, Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CdkAlephBftData {
    canonical: Vec<u8>,
    payload: FederationAlephBftUnitPayload,
}

#[cfg_attr(not(test), allow(dead_code))]
impl CdkAlephBftData {
    /// Wrap a validated CDK unit payload for AlephBFT ordering.
    pub(crate) fn try_from_payload(
        payload: FederationAlephBftUnitPayload,
    ) -> Result<Self, FederationError> {
        let canonical = payload.canonical_bytes()?;
        Ok(Self { canonical, payload })
    }

    /// Return the CDK unit payload ordered by AlephBFT.
    pub(crate) fn payload(&self) -> &FederationAlephBftUnitPayload {
        &self.payload
    }

    /// Return the ordered CDK unit payload.
    pub(crate) fn into_payload(self) -> FederationAlephBftUnitPayload {
        self.payload
    }

    /// Return the exact canonical bytes encoded into AlephBFT messages.
    pub(crate) fn canonical_bytes(&self) -> &[u8] {
        &self.canonical
    }
}

impl PartialEq for CdkAlephBftData {
    fn eq(&self, other: &Self) -> bool {
        self.canonical == other.canonical
    }
}

impl Eq for CdkAlephBftData {}

impl Hash for CdkAlephBftData {
    fn hash<H>(&self, state: &mut H)
    where
        H: Hasher,
    {
        self.canonical.hash(state);
    }
}

impl Encode for CdkAlephBftData {
    fn size_hint(&self) -> usize {
        self.canonical.size_hint()
    }

    fn encode_to<T>(&self, dest: &mut T)
    where
        T: Output + ?Sized,
    {
        self.canonical.encode_to(dest);
    }
}

impl Decode for CdkAlephBftData {
    fn decode<I>(input: &mut I) -> Result<Self, CodecError>
    where
        I: Input,
    {
        let canonical = Vec::<u8>::decode(input)?;
        let payload = serde_json::from_slice::<FederationAlephBftUnitPayload>(&canonical)
            .map_err(|_| CodecError::from("invalid CDK AlephBFT data JSON"))?;
        let data = Self::try_from_payload(payload)
            .map_err(|_| CodecError::from("invalid CDK AlephBFT unit payload"))?;
        if data.canonical != canonical {
            return Err(CodecError::from("non-canonical CDK AlephBFT data encoding"));
        }

        Ok(data)
    }
}

/// Runtime-backed data provider for upstream AlephBFT sessions.
#[derive(Debug, Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CdkAlephBftDataProvider {
    consensus: Arc<tokio::sync::Mutex<FederationConsensusRuntime>>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl CdkAlephBftDataProvider {
    /// Create a data provider over the same consensus runtime used by public admission.
    pub(crate) fn new(consensus: Arc<tokio::sync::Mutex<FederationConsensusRuntime>>) -> Self {
        Self { consensus }
    }
}

#[async_trait::async_trait]
impl DataProvider for CdkAlephBftDataProvider {
    type Output = CdkAlephBftData;

    async fn get_data(&mut self) -> Option<Self::Output> {
        let payload = {
            let mut consensus = self.consensus.lock().await;
            match consensus.next_aleph_bft_unit_payload() {
                Ok(payload) => payload,
                Err(err) => {
                    tracing::warn!("AlephBFT data provider could not build CDK unit: {}", err);
                    return None;
                }
            }
        };

        match payload {
            Some(payload) => match CdkAlephBftData::try_from_payload(payload) {
                Ok(data) => Some(data),
                Err(err) => {
                    tracing::warn!("AlephBFT data provider rejected CDK unit encoding: {}", err);
                    None
                }
            },
            None => None,
        }
    }
}

/// Finalization handler that hands ordered CDK units back to the mint runtime.
#[derive(Debug)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CdkAlephBftFinalizationHandler {
    finalized_units: mpsc::UnboundedSender<FederationAlephBftUnitPayload>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl CdkAlephBftFinalizationHandler {
    /// Create a handler that forwards finalized unit payloads to an async apply loop.
    pub(crate) fn new(
        finalized_units: mpsc::UnboundedSender<FederationAlephBftUnitPayload>,
    ) -> Self {
        Self { finalized_units }
    }
}

impl FinalizationHandler<CdkAlephBftData> for CdkAlephBftFinalizationHandler {
    fn data_finalized(&mut self, data: CdkAlephBftData) {
        if self.finalized_units.send(data.into_payload()).is_err() {
            tracing::warn!("AlephBFT finalized CDK unit receiver is closed");
        }
    }
}

/// Tokio-backed task spawner for upstream AlephBFT.
#[derive(Debug, Clone, Copy, Default)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CdkAlephBftTokioSpawnHandle;

impl SpawnHandle for CdkAlephBftTokioSpawnHandle {
    fn spawn(&self, name: &'static str, task: impl Future<Output = ()> + Send + 'static) {
        tokio::spawn(async move {
            tracing::trace!(task = name, "AlephBFT task started");
            task.await;
        });
    }

    fn spawn_essential(
        &self,
        name: &'static str,
        task: impl Future<Output = ()> + Send + 'static,
    ) -> TaskHandle {
        let handle = tokio::spawn(async move {
            tracing::trace!(task = name, "AlephBFT essential task started");
            task.await;
        });
        Box::pin(async move {
            match handle.await {
                Ok(()) => Ok(()),
                Err(err) => {
                    tracing::error!("AlephBFT essential task failed: {}", err);
                    Err(())
                }
            }
        }) as Pin<Box<dyn Future<Output = Result<(), ()>> + Send>>
    }
}

#[cfg(test)]
mod tests {
    use aleph_bft::{DataProvider, FinalizationHandler, SpawnHandle};
    use cdk::federation::{
        FederationAlephBftUnitPayload, FederationConsensusBackendKind,
        FederationConsensusDefaultItem, FederationConsensusItem, FederationConsensusRuntime,
        FederationId, FederationMintQuoteOperation, FederationOperation,
        FederationOperationEnvelope, MemberId,
    };
    use cdk::nuts::{CurrencyUnit, MintQuoteBolt11Request, MintQuoteBolt11Response};
    use cdk::{Amount, MintQuoteRequest, MintQuoteResponse};
    use parity_scale_codec::{Decode, Encode};
    use serde_json::json;

    use super::*;

    fn federation_id(byte: u8) -> FederationId {
        FederationId::from_bytes([byte; 32])
    }

    fn member_id(id: u16) -> MemberId {
        MemberId::new(id).expect("member id")
    }

    fn module_fact_payload(index: u64) -> FederationAlephBftUnitPayload {
        FederationAlephBftUnitPayload::new(
            federation_id(7),
            member_id(1),
            0,
            index,
            vec![FederationConsensusItem::Default(
                FederationConsensusDefaultItem::new("adapter-test", json!({ "index": index })),
            )],
        )
        .expect("unit payload")
    }

    fn mint_quote_envelope(quote: &str) -> FederationOperationEnvelope {
        FederationOperationEnvelope::new(
            federation_id(8),
            FederationOperation::MintQuote(FederationMintQuoteOperation {
                quote: quote.to_string(),
                request: MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
                    amount: Amount::from(1),
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                }),
                response: MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
                    quote: quote.to_string(),
                    request: format!("invoice-{quote}"),
                    amount: Some(Amount::from(1)),
                    unit: Some(CurrencyUnit::Sat),
                    state: cdk::nuts::MintQuoteState::Unpaid,
                    expiry: None,
                    pubkey: None,
                }),
            }),
        )
    }

    #[test]
    fn cdk_aleph_bft_data_scale_round_trips_canonical_payload() {
        let payload = module_fact_payload(0);
        let data = CdkAlephBftData::try_from_payload(payload.clone()).expect("data");

        let encoded = data.encode();
        let decoded = CdkAlephBftData::decode(&mut encoded.as_slice()).expect("decode");

        assert_eq!(decoded, data);
        assert_eq!(
            decoded.canonical_bytes(),
            payload.canonical_bytes().expect("canonical")
        );
        assert_eq!(decoded.payload().body_hash, payload.body_hash);
    }

    #[test]
    fn cdk_aleph_bft_data_rejects_non_canonical_scale_bytes() {
        let payload = module_fact_payload(1);
        let mut value = serde_json::to_value(&payload).expect("json value");
        value["items"][0]["data"]["payload"] = json!({ "index": 1 });
        let non_canonical = serde_json::to_vec_pretty(&value).expect("pretty json");
        payload
            .ensure_supported_version()
            .expect("payload still valid");

        let encoded = non_canonical.encode();
        let result = CdkAlephBftData::decode(&mut encoded.as_slice());

        assert!(result.is_err(), "non-canonical bytes must fail");
    }

    #[tokio::test]
    async fn cdk_aleph_bft_data_provider_pulls_from_consensus_runtime_queue() {
        let runtime = FederationConsensusRuntime::with_backend(
            FederationConsensusBackendKind::AlephBft,
            federation_id(8),
            member_id(1),
        )
        .expect("runtime");
        let consensus = Arc::new(tokio::sync::Mutex::new(runtime));
        {
            let mut runtime = consensus.lock().await;
            let admission = runtime
                .submit_aleph_bft_envelope(mint_quote_envelope("quote-1"))
                .expect("submit");
            assert_eq!(
                admission,
                cdk::federation::FederationConsensusItemAdmission::Accepted
            );
        }

        let mut provider = CdkAlephBftDataProvider::new(consensus.clone());
        let data = provider.get_data().await.expect("queued data");

        assert_eq!(data.payload().federation_id, federation_id(8));
        assert_eq!(data.payload().submitting_member, member_id(1));
        assert_eq!(data.payload().unit_index, 0);
        assert!(provider.get_data().await.is_none());
        assert_eq!(consensus.lock().await.queued_len(), 0);
    }

    #[tokio::test]
    async fn cdk_aleph_bft_finalization_handler_forwards_payloads() {
        let payload = module_fact_payload(2);
        let data = CdkAlephBftData::try_from_payload(payload.clone()).expect("data");
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let mut handler = CdkAlephBftFinalizationHandler::new(sender);

        handler.data_finalized(data);
        let finalized = receiver.recv().await.expect("finalized payload");

        assert_eq!(finalized.body_hash, payload.body_hash);
        assert_eq!(finalized.unit_index, payload.unit_index);
    }

    #[tokio::test]
    async fn cdk_aleph_bft_tokio_spawn_handle_reports_essential_completion() {
        let spawn = CdkAlephBftTokioSpawnHandle;
        let handle = spawn.spawn_essential("adapter-test", async {});

        assert!(handle.await.is_ok());
    }

    #[test]
    fn mint_quote_fixture_uses_expected_keysetless_quote_shape() {
        let envelope = mint_quote_envelope("quote-fixture");
        let canonical = envelope.canonical_bytes().expect("canonical envelope");
        let decoded = FederationOperationEnvelope::from_json_slice(&canonical).expect("decode");

        assert_eq!(
            decoded.operation.kind(),
            cdk::federation::FederationOperationKind::MintQuote
        );
        assert_eq!(decoded.federation_id, federation_id(8));
    }
}
