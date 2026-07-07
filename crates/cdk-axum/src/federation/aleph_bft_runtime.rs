use core::fmt;

use std::future::Future;
use std::hash::{Hash, Hasher as StdHasher};
use std::pin::Pin;
use std::sync::Arc;

use aleph_bft::{
    DataProvider, FinalizationHandler, Hasher as AlephHasher, Index, Keychain, MultiKeychain,
    Network, NetworkData, NodeCount, NodeIndex, Recipient, SignatureSet, SpawnHandle, TaskHandle,
};
use cdk::federation::{
    FederationAlephBftUnitPayload, FederationConfig, FederationConsensusRuntime, FederationError,
    MemberConfigPrivate, MemberConfigPublic, MemberId,
};
use cdk::nuts::SecretKey;
use parity_scale_codec::{Decode, Encode, Error as CodecError, Input, Output};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

/// SHA-256 hasher used by CDK's AlephBFT session units.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CdkAlephBftHasher;

impl AlephHasher for CdkAlephBftHasher {
    type Hash = [u8; 32];

    fn hash(s: &[u8]) -> Self::Hash {
        Sha256::digest(s).into()
    }
}

/// Concrete upstream AlephBFT network payload used by CDK federation sessions.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) type CdkAlephBftNetworkData = NetworkData<
    CdkAlephBftHasher,
    CdkAlephBftData,
    CdkAlephBftSignature,
    SignatureSet<CdkAlephBftSignature>,
>;

/// Outbound message emitted by the AlephBFT network adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CdkAlephBftOutboundMessage<D> {
    /// Opaque AlephBFT payload to deliver.
    pub data: D,
    /// AlephBFT recipient chosen by the protocol.
    pub recipient: Recipient,
}

/// Channel-backed AlephBFT network implementation.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CdkAlephBftNetwork<D> {
    incoming: mpsc::UnboundedReceiver<D>,
    outgoing: mpsc::UnboundedSender<CdkAlephBftOutboundMessage<D>>,
}

impl<D> fmt::Debug for CdkAlephBftNetwork<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CdkAlephBftNetwork").finish_non_exhaustive()
    }
}

/// Test and transport bridge handle for a [`CdkAlephBftNetwork`].
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CdkAlephBftNetworkHandle<D> {
    incoming: mpsc::UnboundedSender<D>,
    outgoing: mpsc::UnboundedReceiver<CdkAlephBftOutboundMessage<D>>,
}

impl<D> fmt::Debug for CdkAlephBftNetworkHandle<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CdkAlephBftNetworkHandle")
            .finish_non_exhaustive()
    }
}

#[cfg_attr(not(test), allow(dead_code))]
impl<D> CdkAlephBftNetwork<D> {
    /// Create a network plus bridge handle for injecting inbound and draining outbound messages.
    pub(crate) fn new_pair() -> (Self, CdkAlephBftNetworkHandle<D>) {
        let (incoming_sender, incoming_receiver) = mpsc::unbounded_channel();
        let (outgoing_sender, outgoing_receiver) = mpsc::unbounded_channel();
        (
            Self {
                incoming: incoming_receiver,
                outgoing: outgoing_sender,
            },
            CdkAlephBftNetworkHandle {
                incoming: incoming_sender,
                outgoing: outgoing_receiver,
            },
        )
    }
}

#[cfg_attr(not(test), allow(dead_code))]
impl<D> CdkAlephBftNetworkHandle<D> {
    /// Queue an inbound message as if it arrived from the federation transport.
    pub(crate) fn queue_incoming(&self, data: D) -> Result<(), mpsc::error::SendError<D>> {
        self.incoming.send(data)
    }

    /// Receive the next outbound message produced by AlephBFT.
    pub(crate) async fn next_outgoing(&mut self) -> Option<CdkAlephBftOutboundMessage<D>> {
        self.outgoing.recv().await
    }
}

#[async_trait::async_trait]
impl<D> Network<D> for CdkAlephBftNetwork<D>
where
    D: Send + 'static,
{
    fn send(&self, data: D, recipient: Recipient) {
        if self
            .outgoing
            .send(CdkAlephBftOutboundMessage { data, recipient })
            .is_err()
        {
            tracing::warn!("AlephBFT network outbound bridge is closed");
        }
    }

    async fn next_event(&mut self) -> Option<D> {
        self.incoming.recv().await
    }
}

/// Schnorr signature bytes used by CDK's AlephBFT keychain adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Encode, Decode)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CdkAlephBftSignature([u8; 64]);

impl CdkAlephBftSignature {
    fn from_schnorr(signature: cdk::secp256k1::schnorr::Signature) -> Self {
        Self(signature.serialize())
    }

    fn as_schnorr(self) -> Option<cdk::secp256k1::schnorr::Signature> {
        cdk::secp256k1::schnorr::Signature::from_slice(&self.0).ok()
    }
}

/// AlephBFT keychain backed by the federation secp256k1 identity roster.
#[derive(Debug, Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CdkAlephBftKeychain {
    private_config: MemberConfigPrivate,
    members_by_node: Vec<(MemberId, MemberConfigPublic)>,
    local_index: NodeIndex,
    node_count: NodeCount,
}

#[cfg_attr(not(test), allow(dead_code))]
impl CdkAlephBftKeychain {
    /// Build an AlephBFT keychain from the validated federation config and local identity key.
    pub(crate) fn try_new(
        public_config: &FederationConfig,
        private_config: MemberConfigPrivate,
    ) -> Result<Self, cdk::Error> {
        public_config.validate()?;
        if !matches!(&private_config.identity_secret_key, SecretKey::Secp256k1(_)) {
            return Err(cdk::Error::Custom(format!(
                "AlephBFT keychain requires secp256k1 identity key for member {}",
                private_config.member_id
            )));
        }
        if let Some(federation_id) = private_config.federation_id {
            if federation_id != public_config.federation_id {
                return Err(cdk::Error::from(FederationError::FederationIdMismatch {
                    expected: public_config.federation_id,
                    found: federation_id,
                }));
            }
        }

        let node_count = NodeCount(public_config.members.len());
        let expected_consensus_threshold = u16::try_from(node_count.consensus_threshold().0)
            .map_err(|_| cdk::Error::Custom("AlephBFT node count exceeds u16".to_string()))?;
        if public_config.threshold_params.consensus_threshold != expected_consensus_threshold {
            return Err(cdk::Error::Custom(format!(
                "AlephBFT keychain requires BFT consensus threshold {}, found {}",
                expected_consensus_threshold, public_config.threshold_params.consensus_threshold
            )));
        }

        let mut members_by_node = Vec::with_capacity(public_config.members.len());
        let mut local_index = None;
        for (index, (member_id, member)) in public_config.members.iter().enumerate() {
            if *member_id == private_config.member_id {
                local_index = Some(NodeIndex(index));
                if private_config.identity_secret_key.public_key() != member.identity_public_key {
                    return Err(cdk::Error::from(FederationError::PrivateIdentityMismatch {
                        member_id: *member_id,
                    }));
                }
            }
            members_by_node.push((*member_id, member.clone()));
        }
        let local_index = local_index.ok_or(cdk::Error::from(FederationError::UnknownMember {
            member_id: private_config.member_id,
        }))?;

        Ok(Self {
            private_config,
            members_by_node,
            local_index,
            node_count,
        })
    }

    /// Return the federation member represented by a given AlephBFT node index.
    pub(crate) fn member_for_node(&self, node: NodeIndex) -> Option<MemberId> {
        self.members_by_node
            .get(node.0)
            .map(|(member_id, _)| *member_id)
    }
}

impl Index for CdkAlephBftKeychain {
    fn index(&self) -> NodeIndex {
        self.local_index
    }
}

impl Keychain for CdkAlephBftKeychain {
    type Signature = CdkAlephBftSignature;

    fn node_count(&self) -> NodeCount {
        self.node_count
    }

    fn sign(&self, msg: &[u8]) -> Self::Signature {
        CdkAlephBftSignature::from_schnorr(
            self.private_config
                .identity_secret_key
                .sign(msg)
                .expect("validated secp256k1 federation identity key signs AlephBFT messages"),
        )
    }

    fn verify(&self, msg: &[u8], signature: &Self::Signature, index: NodeIndex) -> bool {
        let Some((_, member)) = self.members_by_node.get(index.0) else {
            return false;
        };
        let Some(signature) = signature.as_schnorr() else {
            return false;
        };

        member.identity_public_key.verify(msg, &signature).is_ok()
    }
}

impl MultiKeychain for CdkAlephBftKeychain {
    type PartialMultisignature = SignatureSet<CdkAlephBftSignature>;

    fn bootstrap_multi(
        &self,
        signature: &Self::Signature,
        index: NodeIndex,
    ) -> Self::PartialMultisignature {
        let mut partial = SignatureSet::with_size(self.node_count());
        partial.insert(index, *signature);
        partial
    }

    fn is_complete(&self, msg: &[u8], partial: &Self::PartialMultisignature) -> bool {
        if partial.iter().count() < self.node_count().consensus_threshold().0 {
            return false;
        }

        partial
            .iter()
            .all(|(index, signature)| self.verify(msg, signature, index))
    }
}

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
        H: StdHasher,
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
    use std::collections::BTreeMap;
    use std::str::FromStr;

    use aleph_bft::{
        DataProvider, FinalizationHandler, Index, Keychain, MultiKeychain, Network,
        PartialMultisignature, Recipient, SpawnHandle,
    };
    use cdk::federation::{
        FederationAlephBftUnitPayload, FederationConfig, FederationConsensusBackendKind,
        FederationConsensusDefaultItem, FederationConsensusItem, FederationConsensusRuntime,
        FederationId, FederationMintQuoteOperation, FederationOperation,
        FederationOperationEnvelope, MemberConfigPrivate, MemberConfigPublic, MemberId,
        ThresholdParams,
    };
    use cdk::mint_url::MintUrl;
    use cdk::nuts::{CurrencyUnit, MintQuoteBolt11Request, MintQuoteBolt11Response, SecretKey};
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

    fn identity_secret(id: u16) -> SecretKey {
        let mut bytes = [0u8; 32];
        bytes[31] = u8::try_from(id).expect("test member id fits in u8");
        SecretKey::from_slice(&bytes).expect("identity secret")
    }

    fn public_member(id: u16, identity_secret: &SecretKey) -> MemberConfigPublic {
        MemberConfigPublic {
            member_id: member_id(id),
            public_mint_url: MintUrl::from_str(&format!("https://mint{id}.example.test"))
                .expect("mint url"),
            federation_api_url: MintUrl::from_str(&format!("https://fed{id}.example.test"))
                .expect("federation url"),
            identity_public_key: identity_secret.public_key(),
        }
    }

    fn private_member(id: u16, identity_secret: SecretKey) -> MemberConfigPrivate {
        MemberConfigPrivate {
            federation_id: None,
            member_id: member_id(id),
            identity_secret_key: identity_secret,
            keyset_secret_shares: BTreeMap::new(),
            setup_transcript_hash: None,
        }
    }

    fn keychain_fixture() -> (FederationConfig, Vec<MemberConfigPrivate>) {
        let mut members = BTreeMap::new();
        let mut private_members = Vec::new();
        for id in 1..=4 {
            let secret = identity_secret(id);
            members.insert(member_id(id), public_member(id, &secret));
            private_members.push(private_member(id, secret));
        }
        let threshold_params = ThresholdParams::with_bft_consensus(4, 3).expect("threshold params");
        let public_config =
            FederationConfig::new(threshold_params, members, Vec::new()).expect("public config");

        (public_config, private_members)
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
    fn cdk_aleph_bft_keychain_signs_and_verifies_roster_members() {
        let (public_config, private_members) = keychain_fixture();
        let first = CdkAlephBftKeychain::try_new(&public_config, private_members[0].clone())
            .expect("keychain");
        let second = CdkAlephBftKeychain::try_new(&public_config, private_members[1].clone())
            .expect("keychain");
        let message = b"cdk aleph bft keychain message";

        let signature = first.sign(message);

        assert_eq!(first.index(), NodeIndex(0));
        assert_eq!(first.member_for_node(NodeIndex(0)), Some(member_id(1)));
        assert_eq!(first.member_for_node(NodeIndex(3)), Some(member_id(4)));
        assert_eq!(first.member_for_node(NodeIndex(4)), None);
        assert!(first.verify(message, &signature, NodeIndex(0)));
        assert!(!first.verify(b"tampered", &signature, NodeIndex(0)));
        assert!(!first.verify(message, &signature, NodeIndex(99)));
        assert!(!second.verify(message, &signature, second.index()));
    }

    #[test]
    fn cdk_aleph_bft_keychain_requires_bft_multisignature_quorum() {
        let (public_config, private_members) = keychain_fixture();
        let keychains = private_members
            .into_iter()
            .map(|private_config| {
                CdkAlephBftKeychain::try_new(&public_config, private_config).expect("keychain")
            })
            .collect::<Vec<_>>();
        let message = b"cdk aleph bft multisignature message";
        let first = keychains[0].sign(message);
        let second = keychains[1].sign(message);
        let third = keychains[2].sign(message);

        let partial = keychains[0].bootstrap_multi(&first, keychains[0].index());
        assert!(!keychains[0].is_complete(message, &partial));
        let partial = partial.add_signature(&second, keychains[1].index());
        assert!(!keychains[0].is_complete(message, &partial));
        let partial = partial.add_signature(&third, keychains[2].index());
        assert!(keychains[0].is_complete(message, &partial));

        let bad_third = keychains[2].sign(b"different message");
        let bad_partial = keychains[0]
            .bootstrap_multi(&first, keychains[0].index())
            .add_signature(&second, keychains[1].index())
            .add_signature(&bad_third, keychains[2].index());
        assert!(!keychains[0].is_complete(message, &bad_partial));
    }

    #[test]
    fn cdk_aleph_bft_keychain_rejects_non_bft_consensus_threshold() {
        let mut members = BTreeMap::new();
        let mut private_members = Vec::new();
        for id in 1..=5 {
            let secret = identity_secret(id);
            members.insert(member_id(id), public_member(id, &secret));
            private_members.push(private_member(id, secret));
        }
        let public_config = FederationConfig::new(
            ThresholdParams::new(5, 3, 3).expect("threshold params"),
            members,
            Vec::new(),
        )
        .expect("public config");

        let err = CdkAlephBftKeychain::try_new(&public_config, private_members[0].clone())
            .expect_err("non-BFT threshold must fail");

        assert!(
            err.to_string().contains("BFT consensus threshold 4"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn cdk_aleph_bft_hasher_uses_stable_sha256() {
        assert_eq!(
            CdkAlephBftHasher::hash(b"cdk aleph bft"),
            [
                0x11, 0xc9, 0xad, 0xb6, 0xa5, 0x45, 0x23, 0x9e, 0x59, 0x15, 0xc1, 0x6a, 0xdd, 0x83,
                0x74, 0x8a, 0x09, 0x51, 0x86, 0x82, 0x5c, 0xc5, 0x35, 0xc2, 0xdb, 0x50, 0xa6, 0xfe,
                0x5f, 0xe6, 0x53, 0xcf,
            ]
        );
    }

    #[test]
    fn cdk_aleph_bft_network_data_alias_is_network_compatible() {
        fn assert_network<N>(_: &N)
        where
            N: Network<CdkAlephBftNetworkData>,
        {
        }

        let (network, _handle) = CdkAlephBftNetwork::<CdkAlephBftNetworkData>::new_pair();

        assert_network(&network);
    }

    #[tokio::test]
    async fn cdk_aleph_bft_network_preserves_outbound_recipient() {
        let (network, mut handle) = CdkAlephBftNetwork::<String>::new_pair();

        network.send("network-message".to_string(), Recipient::Node(NodeIndex(2)));
        let outbound = handle.next_outgoing().await.expect("outbound message");

        assert_eq!(outbound.data, "network-message");
        assert_eq!(outbound.recipient, Recipient::Node(NodeIndex(2)));
    }

    #[tokio::test]
    async fn cdk_aleph_bft_network_delivers_inbound_messages() {
        let (mut network, handle) = CdkAlephBftNetwork::<String>::new_pair();

        handle
            .queue_incoming("inbound-message".to_string())
            .expect("queue inbound message");

        assert_eq!(
            network.next_event().await.as_deref(),
            Some("inbound-message")
        );
    }

    #[tokio::test]
    async fn cdk_aleph_bft_network_returns_none_when_inbound_bridge_closes() {
        let (mut network, handle) = CdkAlephBftNetwork::<String>::new_pair();
        drop(handle);

        assert!(network.next_event().await.is_none());
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
