use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use bip39::Mnemonic;
use cdk_common::federation::{
    generate_trusted_dealer_configs, FederatedKeyset, FederationConfig, FederationError,
    FederationId, FederationInvite, FederationJournalEntry, FederationKeysetRotationOperation,
    FederationMeltQuoteMetadata, FederationOperation, FederationOperationEnvelope,
    FederationOperationId, InMemoryFederationJournal, MemberId, ThresholdParams,
    TrustedDealerConfigBundle, TrustedDealerKeysetSpec, TrustedDealerMember,
};
use cdk_common::mint_url::MintUrl;
use cdk_common::nuts::nut01::{trusted_dealer_keygen, BlsG1PublicKey, BlsSecretKey};
use cdk_common::nuts::nut02::KeySet;
use cdk_common::payment::PaymentIdentifier;
use cdk_common::util::unix_time;
use cdk_common::{
    Amount, BlindSignature, BlindedMessage, Bolt11Invoice, CurrencyUnit, Id,
    MeltQuoteBolt11Request, MeltQuoteBolt11Response, MeltQuoteCreateResponse, MeltQuoteRequest,
    MeltQuoteResponse, MintQuoteBolt11Response, PublicKey, QuoteId,
};
use cdk_common::{MintQuoteRequest, MintQuoteResponse, MintResponse, SecretKey};
use cdk_fake_wallet::{create_fake_invoice, FakeWallet};

use super::{
    FederatedMintConnector, FederatedMintConnectorConfig, FederationWalletConnector,
    FederationWalletPerformanceStage, FEDERATION_CATCH_UP_MAX_RETRIES,
};
use crate::dhke::verify_bls_blind_signature;
use crate::mint::{FederationRuntimeConfig, Mint, MintBuilder, MintInput, MintMeltLimits};
use crate::nuts::nut00::{KnownMethod, PaymentMethod, ProofsMethods};
use crate::nuts::{
    BatchCheckMintQuoteRequest, BatchMintRequest, CheckStateRequest, CheckStateResponse,
    KeysetResponse, MeltQuoteState, MeltRequest, MintInfo, MintQuoteBolt11Request, MintQuoteState,
    MintRequest, PreMintSecrets, ProofState, RestoreRequest, RestoreResponse, State, SwapRequest,
    SwapResponse,
};
use crate::types::{FeeReserve, QuoteTTL};
use crate::wallet::mint_metadata_cache::MintMetadataCache;
use crate::wallet::test_utils::MockMintConnector;
use crate::wallet::{AuthWallet, MintConnector, WalletBuilder};

fn member_id(id: u16) -> MemberId {
    MemberId::new(id).expect("member id")
}

fn identity_secret(member_id: MemberId) -> SecretKey {
    let mut bytes = [0u8; 32];
    bytes[31] = member_id.get() as u8;
    SecretKey::from_slice(&bytes).expect("secret key")
}

#[test]
fn wallet_performance_stages_have_unique_fixed_labels() {
    let labels = FederationWalletPerformanceStage::ALL
        .into_iter()
        .map(FederationWalletPerformanceStage::as_str)
        .collect::<HashSet<_>>();

    assert_eq!(labels.len(), FederationWalletPerformanceStage::ALL.len());
}

fn member(member_id: MemberId) -> TrustedDealerMember {
    TrustedDealerMember::new(
        member_id,
        MintUrl::from_str(&format!("https://mint{}.example.com", member_id.get()))
            .expect("mint url"),
        MintUrl::from_str(&format!("https://fed{}.example.com", member_id.get())).expect("fed url"),
        identity_secret(member_id),
    )
}

async fn test_auth_wallet() -> AuthWallet {
    let mint_url = MintUrl::from_str("https://federation.example.com").expect("mint url");
    let localstore = Arc::new(
        cdk_sqlite::wallet::memory::empty()
            .await
            .expect("wallet db"),
    );
    let metadata_cache = Arc::new(MintMetadataCache::new(mint_url.clone()));

    AuthWallet::new(
        mint_url,
        None,
        localstore,
        metadata_cache,
        HashMap::new(),
        None,
    )
}

fn bundle() -> cdk_common::federation::TrustedDealerConfigBundle {
    static BUNDLE: OnceLock<TrustedDealerConfigBundle> = OnceLock::new();
    BUNDLE
        .get_or_init(|| {
            generate_trusted_dealer_configs(
                ThresholdParams::with_bft_consensus(2, 1).expect("params"),
                vec![member(member_id(1)), member(member_id(2))],
                vec![TrustedDealerKeysetSpec::new(
                    CurrencyUnit::Sat,
                    true,
                    0,
                    None,
                    BTreeMap::from([(
                        Amount::from(1),
                        vec![BlsSecretKey::from_reduced_bytes(&[2; 32])],
                    )]),
                )],
            )
            .expect("trusted dealer bundle")
        })
        .clone()
}

fn threshold_bundle() -> TrustedDealerConfigBundle {
    static THRESHOLD_BUNDLE: OnceLock<TrustedDealerConfigBundle> = OnceLock::new();
    THRESHOLD_BUNDLE
        .get_or_init(|| {
            generate_trusted_dealer_configs(
                ThresholdParams::with_bft_consensus(3, 2).expect("params"),
                vec![
                    member(member_id(1)),
                    member(member_id(2)),
                    member(member_id(3)),
                ],
                vec![TrustedDealerKeysetSpec::new(
                    CurrencyUnit::Sat,
                    true,
                    0,
                    None,
                    BTreeMap::from([
                        (
                            Amount::from(1),
                            vec![
                                BlsSecretKey::from_reduced_bytes(&[2; 32]),
                                BlsSecretKey::from_reduced_bytes(&[3; 32]),
                            ],
                        ),
                        (
                            Amount::from(2),
                            vec![
                                BlsSecretKey::from_reduced_bytes(&[4; 32]),
                                BlsSecretKey::from_reduced_bytes(&[5; 32]),
                            ],
                        ),
                        (
                            Amount::from(4),
                            vec![
                                BlsSecretKey::from_reduced_bytes(&[6; 32]),
                                BlsSecretKey::from_reduced_bytes(&[7; 32]),
                            ],
                        ),
                    ]),
                )],
            )
            .expect("trusted dealer bundle")
        })
        .clone()
}

fn threshold_five_bundle() -> TrustedDealerConfigBundle {
    static THRESHOLD_FIVE_BUNDLE: OnceLock<TrustedDealerConfigBundle> = OnceLock::new();
    THRESHOLD_FIVE_BUNDLE
        .get_or_init(|| {
            generate_trusted_dealer_configs(
                ThresholdParams::with_bft_consensus(5, 3).expect("params"),
                vec![
                    member(member_id(1)),
                    member(member_id(2)),
                    member(member_id(3)),
                    member(member_id(4)),
                    member(member_id(5)),
                ],
                vec![TrustedDealerKeysetSpec::new(
                    CurrencyUnit::Sat,
                    true,
                    0,
                    None,
                    BTreeMap::from([
                        (
                            Amount::from(1),
                            vec![
                                BlsSecretKey::from_reduced_bytes(&[2; 32]),
                                BlsSecretKey::from_reduced_bytes(&[3; 32]),
                                BlsSecretKey::from_reduced_bytes(&[4; 32]),
                            ],
                        ),
                        (
                            Amount::from(2),
                            vec![
                                BlsSecretKey::from_reduced_bytes(&[4; 32]),
                                BlsSecretKey::from_reduced_bytes(&[5; 32]),
                                BlsSecretKey::from_reduced_bytes(&[6; 32]),
                            ],
                        ),
                        (
                            Amount::from(4),
                            vec![
                                BlsSecretKey::from_reduced_bytes(&[6; 32]),
                                BlsSecretKey::from_reduced_bytes(&[7; 32]),
                                BlsSecretKey::from_reduced_bytes(&[8; 32]),
                            ],
                        ),
                    ]),
                )],
            )
            .expect("trusted dealer bundle")
        })
        .clone()
}

fn threshold_four_bundle() -> TrustedDealerConfigBundle {
    static THRESHOLD_FOUR_BUNDLE: OnceLock<TrustedDealerConfigBundle> = OnceLock::new();
    THRESHOLD_FOUR_BUNDLE
        .get_or_init(|| {
            generate_trusted_dealer_configs(
                ThresholdParams::with_bft_consensus(4, 3).expect("params"),
                vec![
                    member(member_id(1)),
                    member(member_id(2)),
                    member(member_id(3)),
                    member(member_id(4)),
                ],
                vec![TrustedDealerKeysetSpec::new(
                    CurrencyUnit::Sat,
                    true,
                    0,
                    None,
                    BTreeMap::from([(
                        Amount::from(1),
                        vec![
                            BlsSecretKey::from_reduced_bytes(&[2; 32]),
                            BlsSecretKey::from_reduced_bytes(&[3; 32]),
                            BlsSecretKey::from_reduced_bytes(&[4; 32]),
                        ],
                    )]),
                )],
            )
            .expect("trusted dealer 3-of-4 bundle")
        })
        .clone()
}

fn threshold_rotation_bundle() -> TrustedDealerConfigBundle {
    static THRESHOLD_ROTATION_BUNDLE: OnceLock<TrustedDealerConfigBundle> = OnceLock::new();
    THRESHOLD_ROTATION_BUNDLE
        .get_or_init(|| {
            generate_trusted_dealer_configs(
                ThresholdParams::with_bft_consensus(3, 2).expect("params"),
                vec![
                    member(member_id(1)),
                    member(member_id(2)),
                    member(member_id(3)),
                ],
                vec![
                    TrustedDealerKeysetSpec::new(
                        CurrencyUnit::Sat,
                        true,
                        0,
                        None,
                        BTreeMap::from([(
                            Amount::from(4),
                            vec![
                                BlsSecretKey::from_reduced_bytes(&[6; 32]),
                                BlsSecretKey::from_reduced_bytes(&[7; 32]),
                            ],
                        )]),
                    ),
                    TrustedDealerKeysetSpec::new(
                        CurrencyUnit::Sat,
                        false,
                        0,
                        None,
                        BTreeMap::from([
                            (
                                Amount::from(1),
                                vec![
                                    BlsSecretKey::from_reduced_bytes(&[8; 32]),
                                    BlsSecretKey::from_reduced_bytes(&[9; 32]),
                                ],
                            ),
                            (
                                Amount::from(2),
                                vec![
                                    BlsSecretKey::from_reduced_bytes(&[10; 32]),
                                    BlsSecretKey::from_reduced_bytes(&[11; 32]),
                                ],
                            ),
                        ]),
                    ),
                ],
            )
            .expect("trusted dealer rotation bundle")
        })
        .clone()
}

fn active_keyset(bundle: &TrustedDealerConfigBundle) -> FederatedKeyset {
    bundle
        .public_config
        .keysets
        .iter()
        .find(|keyset| keyset.active)
        .expect("active keyset")
        .clone()
}

fn inactive_rotation_keyset(bundle: &TrustedDealerConfigBundle) -> FederatedKeyset {
    bundle
        .public_config
        .keysets
        .iter()
        .find(|keyset| !keyset.active)
        .expect("inactive rotation keyset")
        .clone()
}

fn activated_rotation_keyset(bundle: &TrustedDealerConfigBundle) -> FederatedKeyset {
    let mut keyset = inactive_rotation_keyset(bundle);
    keyset.active = true;
    keyset
}

fn rotated_public_config(bundle: &TrustedDealerConfigBundle) -> FederationConfig {
    bundle
        .public_config
        .apply_keyset_rotation(&FederationKeysetRotationOperation {
            keyset: activated_rotation_keyset(bundle),
        })
        .expect("rotated public config")
}

fn advertised_keyset(bundle: &cdk_common::federation::TrustedDealerConfigBundle) -> KeySet {
    let keyset = &bundle.public_config.keysets[0];
    KeySet {
        id: keyset.id,
        unit: keyset.unit.clone(),
        active: Some(keyset.active),
        keys: keyset.keys(),
        input_fee_ppk: keyset.input_fee_ppk,
        final_expiry: keyset.final_expiry,
    }
}

fn federation_mint_info(bundle: &TrustedDealerConfigBundle, member_id: MemberId) -> MintInfo {
    MintInfo::new().federation(
        bundle
            .public_config
            .mint_info_for_member(member_id)
            .expect("federation mint-info advertisement"),
    )
}

fn mismatched_federation_mint_info(
    bundle: &TrustedDealerConfigBundle,
    member_id: MemberId,
) -> MintInfo {
    let mut info = federation_mint_info(bundle, member_id);
    info.federation
        .as_mut()
        .expect("federation mint-info")
        .federation_id = FederationId::from_bytes([42u8; 32]).to_string();
    info
}

fn set_member_advertisement_with_keyset(
    connector: &MockMintConnector,
    bundle: &TrustedDealerConfigBundle,
    member_id: MemberId,
    advertised: KeySet,
) {
    connector.set_active_keyset(advertised);
    connector.set_mint_info_response(Ok(federation_mint_info(bundle, member_id)));
}

fn blinded_outputs(bundle: &TrustedDealerConfigBundle) -> Vec<BlindedMessage> {
    let keyset = &bundle.public_config.keysets[0];
    blinded_outputs_for_keyset(keyset, b"federated wallet connector output")
}

fn non_v3_blinded_outputs(bundle: &TrustedDealerConfigBundle, label: &[u8]) -> Vec<BlindedMessage> {
    let keyset = &bundle.public_config.keysets[0];
    let non_v3_keyset_id = Id::v2_from_data(
        &keyset.keys(),
        &keyset.unit,
        keyset.input_fee_ppk,
        keyset.final_expiry,
    );
    blinded_outputs_for_keyset(keyset, label)
        .into_iter()
        .map(|mut output| {
            output.keyset_id = non_v3_keyset_id;
            output
        })
        .collect()
}

fn blinded_outputs_for_keyset(keyset: &FederatedKeyset, label: &[u8]) -> Vec<BlindedMessage> {
    vec![
        blinded_output_for_keyset_amount(keyset, 1, label),
        blinded_output_for_keyset_amount(keyset, 2, label),
    ]
}

fn blinded_output_for_keyset_amount(
    keyset: &FederatedKeyset,
    amount: u64,
    label: &[u8],
) -> BlindedMessage {
    let amount_label = [label, b" ".as_slice(), amount.to_string().as_bytes()].concat();
    BlindedMessage::new(
        Amount::from(amount),
        keyset.id,
        PublicKey::from(BlsG1PublicKey::hash_to_curve(&amount_label)),
    )
}

fn blinded_change_outputs_for_keyset(
    keyset: &FederatedKeyset,
    label: &[u8],
) -> Vec<BlindedMessage> {
    blinded_outputs_for_keyset(keyset, label)
        .into_iter()
        .map(|mut output| {
            output.amount = Amount::ZERO;
            output
        })
        .collect()
}

fn blinded_change_outputs(bundle: &TrustedDealerConfigBundle) -> Vec<BlindedMessage> {
    let keyset = &bundle.public_config.keysets[0];
    blinded_change_outputs_for_keyset(keyset, b"federated wallet connector change")
}

fn signed_change_outputs(change_outputs: &[BlindedMessage]) -> Vec<BlindedMessage> {
    [Amount::from(1), Amount::from(2)]
        .into_iter()
        .zip(change_outputs.iter().cloned())
        .map(|(amount, mut output)| {
            output.amount = amount;
            output
        })
        .collect()
}

fn aggregate_secret_for_amount(amount: u64) -> BlsSecretKey {
    let byte = match amount {
        1 => 2,
        2 => 4,
        4 => 6,
        _ => panic!("missing aggregate secret fixture for amount {amount}"),
    };

    BlsSecretKey::from_reduced_bytes(&[byte; 32])
}

fn proof_for_amount(
    bundle: &TrustedDealerConfigBundle,
    amount: u64,
    label: &str,
) -> crate::nuts::Proof {
    let keyset = &bundle.public_config.keysets[0];
    let secret = crate::secret::Secret::new(format!("federated-wallet-proof-{label}"));
    let signature =
        BlsG1PublicKey::hash_to_curve(secret.as_bytes()).mul(&aggregate_secret_for_amount(amount));

    crate::nuts::Proof::new(
        Amount::from(amount),
        keyset.id,
        secret,
        PublicKey::from(signature),
    )
}

fn share_response(
    bundle: &TrustedDealerConfigBundle,
    member_id: MemberId,
    outputs: &[BlindedMessage],
) -> MintResponse {
    let private = bundle
        .private_configs
        .get(&member_id)
        .expect("member private config");
    let share_set = private
        .sign_blinded_messages(
            &bundle.public_config,
            FederationOperationId::from_bytes([9; 32]),
            outputs,
        )
        .expect("signature shares");

    MintResponse {
        signatures: share_set
            .shares
            .into_iter()
            .map(|share| BlindSignature {
                amount: share.amount,
                keyset_id: share.keyset_id,
                c: share.share.signature().into(),
                dleq: None,
            })
            .collect(),
    }
}

fn pending_finalization_error(byte: u8) -> crate::Error {
    crate::Error::Federation(FederationError::OperationPendingFinalization {
        operation_id: FederationOperationId::from_bytes([byte; 32]),
        timeout_ms: 25,
    })
}

fn federation_catch_up_error(member: MemberId) -> crate::Error {
    crate::Error::MintErrorResponse(cdk_common::error::ErrorResponse::new(
        cdk_common::error::ErrorCode::FederationCatchUp,
        format!("federation member {member} is catching up"),
    ))
}

fn member_connectors(
    bundle: &TrustedDealerConfigBundle,
    outputs: &[BlindedMessage],
) -> BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>> {
    let advertised = advertised_keyset(bundle);
    [member_id(1), member_id(2), member_id(3)]
        .into_iter()
        .map(|member_id| {
            let connector = MockMintConnector::new();
            set_member_advertisement_with_keyset(&connector, bundle, member_id, advertised.clone());
            connector.set_post_mint_response(Ok(share_response(bundle, member_id, outputs)));
            connector.set_post_batch_mint_response(Ok(share_response(bundle, member_id, outputs)));
            connector.set_post_swap_response(Ok(SwapResponse {
                signatures: share_response(bundle, member_id, outputs).signatures,
            }));
            (
                member_id,
                Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
            )
        })
        .collect()
}

fn public_config_with_mismatched_public_share(
    bundle: &TrustedDealerConfigBundle,
) -> FederationConfig {
    let mut public_config = bundle.public_config.clone();
    let amount = Amount::from(1);
    let wrong_keyset = trusted_dealer_keygen(
        &[
            BlsSecretKey::from_reduced_bytes(&[9; 32]),
            BlsSecretKey::from_reduced_bytes(&[10; 32]),
        ],
        &[
            member_id(1).to_bls_signer_id(),
            member_id(2).to_bls_signer_id(),
        ],
    )
    .expect("wrong keyset");
    public_config.keysets[0]
        .public_shares
        .get_mut(&member_id(1))
        .expect("member shares")
        .insert(amount, wrong_keyset.public_shares()[0]);
    public_config
}

fn member_connectors_with_only_keysets(
    bundle: &TrustedDealerConfigBundle,
) -> BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>> {
    let advertised = advertised_keyset(bundle);
    [member_id(1), member_id(2), member_id(3)]
        .into_iter()
        .map(|member_id| {
            let connector = MockMintConnector::new();
            set_member_advertisement_with_keyset(&connector, bundle, member_id, advertised.clone());
            (
                member_id,
                Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
            )
        })
        .collect()
}

#[derive(Clone)]
struct PostMintConcurrencyProbeConnector {
    advertised: KeySet,
    mint_info: MintInfo,
    response: MintResponse,
    delay: Duration,
    active_calls: Arc<AtomicUsize>,
    max_active_calls: Arc<AtomicUsize>,
    total_calls: Arc<AtomicUsize>,
}

struct ActiveCallGuard(Arc<AtomicUsize>);

impl Drop for ActiveCallGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl fmt::Debug for PostMintConcurrencyProbeConnector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PostMintConcurrencyProbeConnector")
            .finish_non_exhaustive()
    }
}

impl PostMintConcurrencyProbeConnector {
    fn new(
        advertised: KeySet,
        mint_info: MintInfo,
        response: MintResponse,
        delay: Duration,
        active_calls: Arc<AtomicUsize>,
        max_active_calls: Arc<AtomicUsize>,
        total_calls: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            advertised,
            mint_info,
            response,
            delay,
            active_calls,
            max_active_calls,
            total_calls,
        }
    }

    fn unsupported(operation: &str) -> crate::Error {
        crate::Error::Custom(format!(
            "post-mint concurrency probe does not implement {operation}"
        ))
    }

    fn record_active_call(&self) -> ActiveCallGuard {
        let active = self.active_calls.fetch_add(1, Ordering::SeqCst) + 1;
        self.total_calls.fetch_add(1, Ordering::SeqCst);
        let mut observed = self.max_active_calls.load(Ordering::SeqCst);
        while active > observed {
            match self.max_active_calls.compare_exchange(
                observed,
                active,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(current) => observed = current,
            }
        }
        ActiveCallGuard(self.active_calls.clone())
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl MintConnector for PostMintConcurrencyProbeConnector {
    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    async fn resolve_dns_txt(&self, _domain: &str) -> Result<Vec<String>, crate::Error> {
        Err(Self::unsupported("resolve_dns_txt"))
    }

    async fn fetch_lnurl_pay_request(
        &self,
        _url: &str,
    ) -> Result<crate::lightning_address::LnurlPayResponse, crate::Error> {
        Err(Self::unsupported("fetch_lnurl_pay_request"))
    }

    async fn fetch_lnurl_invoice(
        &self,
        _url: &str,
    ) -> Result<crate::lightning_address::LnurlPayInvoiceResponse, crate::Error> {
        Err(Self::unsupported("fetch_lnurl_invoice"))
    }

    async fn get_mint_keys(&self) -> Result<Vec<KeySet>, crate::Error> {
        Ok(vec![self.advertised.clone()])
    }

    async fn get_mint_keyset(&self, _keyset_id: Id) -> Result<KeySet, crate::Error> {
        Err(Self::unsupported("get_mint_keyset"))
    }

    async fn get_mint_keysets(&self) -> Result<KeysetResponse, crate::Error> {
        Err(Self::unsupported("get_mint_keysets"))
    }

    async fn post_mint_quote(
        &self,
        _request: MintQuoteRequest,
    ) -> Result<MintQuoteResponse<String>, crate::Error> {
        Err(Self::unsupported("post_mint_quote"))
    }

    async fn post_mint(
        &self,
        _method: &PaymentMethod,
        _request: MintRequest<String>,
    ) -> Result<MintResponse, crate::Error> {
        let _active_call = self.record_active_call();
        tokio::time::sleep(self.delay).await;
        Ok(self.response.clone())
    }

    async fn post_batch_check_mint_quote_status(
        &self,
        _method: &PaymentMethod,
        _request: BatchCheckMintQuoteRequest<String>,
    ) -> Result<Vec<MintQuoteResponse<String>>, crate::Error> {
        Err(Self::unsupported("post_batch_check_mint_quote_status"))
    }

    async fn post_batch_mint(
        &self,
        _method: &PaymentMethod,
        _request: BatchMintRequest<String>,
    ) -> Result<MintResponse, crate::Error> {
        Err(Self::unsupported("post_batch_mint"))
    }

    async fn post_melt_quote(
        &self,
        _request: MeltQuoteRequest,
    ) -> Result<MeltQuoteCreateResponse<String>, crate::Error> {
        Err(Self::unsupported("post_melt_quote"))
    }

    async fn get_mint_quote_status(
        &self,
        _method: PaymentMethod,
        _quote_id: &str,
    ) -> Result<MintQuoteResponse<String>, crate::Error> {
        Err(Self::unsupported("get_mint_quote_status"))
    }

    async fn get_melt_quote_status(
        &self,
        _method: PaymentMethod,
        _quote_id: &str,
    ) -> Result<MeltQuoteResponse<String>, crate::Error> {
        Err(Self::unsupported("get_melt_quote_status"))
    }

    async fn post_melt(
        &self,
        _method: &PaymentMethod,
        _request: MeltRequest<String>,
    ) -> Result<MeltQuoteResponse<String>, crate::Error> {
        Err(Self::unsupported("post_melt"))
    }

    async fn post_swap(&self, _request: SwapRequest) -> Result<SwapResponse, crate::Error> {
        Err(Self::unsupported("post_swap"))
    }

    async fn get_mint_info(&self) -> Result<MintInfo, crate::Error> {
        Ok(self.mint_info.clone())
    }

    async fn post_check_state(
        &self,
        _request: CheckStateRequest,
    ) -> Result<CheckStateResponse, crate::Error> {
        Err(Self::unsupported("post_check_state"))
    }

    async fn post_restore(
        &self,
        _request: RestoreRequest,
    ) -> Result<RestoreResponse, crate::Error> {
        Err(Self::unsupported("post_restore"))
    }

    async fn get_auth_wallet(&self) -> Option<crate::wallet::AuthWallet> {
        None
    }

    async fn set_auth_wallet(&self, _wallet: Option<crate::wallet::AuthWallet>) {}
}

fn melt_response(
    bundle: &TrustedDealerConfigBundle,
    member_id: MemberId,
    change_outputs: &[BlindedMessage],
) -> MeltQuoteResponse<String> {
    let signed_outputs = signed_change_outputs(change_outputs);
    MeltQuoteResponse::Bolt11(MeltQuoteBolt11Response {
        quote: "melt-quote".to_string(),
        amount: Amount::from(3),
        fee_reserve: Amount::ZERO,
        state: MeltQuoteState::Paid,
        expiry: 42,
        payment_preimage: Some("payment-preimage".to_string()),
        change: Some(share_response(bundle, member_id, &signed_outputs).signatures),
        request: Some("bolt11-invoice".to_string()),
        unit: Some(CurrencyUnit::Sat),
        method: PaymentMethod::Known(KnownMethod::Bolt11),
    })
}

fn melt_response_without_change() -> MeltQuoteResponse<String> {
    MeltQuoteResponse::Bolt11(MeltQuoteBolt11Response {
        quote: "melt-quote".to_string(),
        amount: Amount::from(1),
        fee_reserve: Amount::ZERO,
        state: MeltQuoteState::Paid,
        expiry: 42,
        payment_preimage: Some("payment-preimage".to_string()),
        change: None,
        request: Some("bolt11-invoice".to_string()),
        unit: Some(CurrencyUnit::Sat),
        method: PaymentMethod::Known(KnownMethod::Bolt11),
    })
}

fn melt_response_without_change_with_state(
    state: MeltQuoteState,
) -> MeltQuoteBolt11Response<String> {
    let mut response = match melt_response_without_change() {
        MeltQuoteResponse::Bolt11(response) => response,
        _ => panic!("expected bolt11 melt response"),
    };
    response.state = state;
    if state != MeltQuoteState::Paid {
        response.payment_preimage = None;
    }
    response
}

fn melt_quote_create_response() -> MeltQuoteCreateResponse<String> {
    MeltQuoteCreateResponse::Bolt11(MeltQuoteBolt11Response {
        quote: QuoteId::new().to_string(),
        amount: Amount::from(1),
        fee_reserve: Amount::ZERO,
        state: MeltQuoteState::Unpaid,
        expiry: 42,
        payment_preimage: None,
        change: None,
        request: Some("bolt11-invoice".to_string()),
        unit: Some(CurrencyUnit::Sat),
        method: PaymentMethod::Known(KnownMethod::Bolt11),
    })
}

fn melt_quote_status_from_create_response(
    response: &MeltQuoteCreateResponse<String>,
) -> MeltQuoteBolt11Response<String> {
    match response {
        MeltQuoteCreateResponse::Bolt11(response) => response.clone(),
        _ => panic!("expected bolt11 melt quote response"),
    }
}

fn assert_melt_quote_matches_expected(
    response: &MeltQuoteCreateResponse<String>,
    expected: &MeltQuoteCreateResponse<String>,
) {
    let quote_id = response.quote().expect("melt quote id");
    QuoteId::from_str(quote_id).expect("valid melt quote id");
    assert_eq!(response, expected);
}

fn mint_quote_status_response(state: MintQuoteState) -> MintQuoteBolt11Response<String> {
    mint_quote_status_response_for("mint-quote", state)
}

fn mint_quote_status_response_for(
    quote: &str,
    state: MintQuoteState,
) -> MintQuoteBolt11Response<String> {
    MintQuoteBolt11Response {
        quote: quote.to_string(),
        request: "bolt11-invoice".to_string(),
        amount: Some(Amount::from(3)),
        unit: Some(CurrencyUnit::Sat),
        method: PaymentMethod::Known(KnownMethod::Bolt11),
        amount_paid: match state {
            MintQuoteState::Unpaid => Amount::ZERO,
            MintQuoteState::Paid | MintQuoteState::Issued => Amount::from(3),
        },
        amount_issued: match state {
            MintQuoteState::Issued => Amount::from(3),
            MintQuoteState::Unpaid | MintQuoteState::Paid => Amount::ZERO,
        },
        updated_at: 0,
        state,
        expiry: Some(42),
        pubkey: None,
    }
}

fn restore_response(
    bundle: &TrustedDealerConfigBundle,
    member_id: MemberId,
    restore_outputs: &[BlindedMessage],
) -> RestoreResponse {
    RestoreResponse {
        outputs: restore_outputs.to_vec(),
        signatures: share_response(bundle, member_id, &signed_change_outputs(restore_outputs))
            .signatures,
    }
}

fn proof_state_response(ys: &[PublicKey], states: &[State]) -> CheckStateResponse {
    CheckStateResponse {
        states: ys
            .iter()
            .copied()
            .zip(states.iter().copied())
            .map(|(y, state)| ProofState {
                y,
                state,
                witness: None,
            })
            .collect(),
    }
}

fn member_connectors_with_melt_change(
    bundle: &TrustedDealerConfigBundle,
    change_outputs: &[BlindedMessage],
) -> BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>> {
    let advertised = advertised_keyset(bundle);
    [member_id(1), member_id(2), member_id(3)]
        .into_iter()
        .map(|member_id| {
            let connector = MockMintConnector::new();
            set_member_advertisement_with_keyset(&connector, bundle, member_id, advertised.clone());
            connector.set_post_melt_response(Ok(melt_response(bundle, member_id, change_outputs)));
            (
                member_id,
                Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
            )
        })
        .collect()
}

mod local_connector;

use self::local_connector::{http_member_connectors, LocalFederatedMemberConnector};

async fn create_fake_wallet_member_mint(
    bundle: &TrustedDealerConfigBundle,
    member_id: MemberId,
) -> Mint {
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let runtime_config = FederationRuntimeConfig::new(
        bundle.public_config.clone(),
        bundle
            .private_configs
            .get(&member_id)
            .expect("private member config")
            .clone(),
    )
    .expect("runtime config");
    let mut builder = MintBuilder::new(db.clone())
        .with_batch_minting(Some(10), Some(vec![KnownMethod::Bolt11.to_string()]))
        .with_federation_config(runtime_config);
    let fake = FakeWallet::new(
        FeeReserve {
            min_fee_reserve: Amount::from(1),
            percent_fee_reserve: 0.0,
        },
        HashMap::default(),
        HashSet::default(),
        0,
        CurrencyUnit::Sat,
    );
    builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintMeltLimits::new(1, 10_000),
            Arc::new(fake),
        )
        .await
        .expect("payment processor");

    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mint = builder
        .build_with_seed(db, &mnemonic.to_seed_normalized(""))
        .await
        .expect("mint");
    mint.set_quote_ttl(QuoteTTL::new(10_000, 10_000))
        .await
        .expect("quote ttl");
    mint.start().await.expect("mint start");
    mint
}

fn appended_entry(
    journal: &mut InMemoryFederationJournal,
    envelope: FederationOperationEnvelope,
) -> FederationJournalEntry {
    journal.append(envelope).expect("append").entry().clone()
}

async fn persist_and_apply_entry(mint: &Mint, entry: &FederationJournalEntry) {
    let localstore = mint.localstore();
    let mut tx = localstore.begin_transaction().await.expect("begin");
    tx.add_federation_journal_entry(entry)
        .await
        .expect("persist journal entry");
    tx.commit().await.expect("commit journal entry");

    mint.apply_federation_journal_entries(std::slice::from_ref(entry))
        .await
        .expect("apply journal entry");
}

fn bolt11_quote_request(amount: u64) -> MintQuoteRequest {
    MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
        amount: Amount::from(amount),
        unit: CurrencyUnit::Sat,
        description: None,
        pubkey: None,
    })
}

fn bolt11_melt_quote_request() -> MeltQuoteRequest {
    MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
        request: create_fake_invoice(1000, "".to_string()),
        unit: CurrencyUnit::Sat,
        options: None,
    })
}

fn bolt11_quote_response(quote: &QuoteId, amount: u64) -> MintQuoteResponse<String> {
    MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
        quote: quote.to_string(),
        request: format!("invoice-{quote}"),
        amount: Some(Amount::from(amount)),
        unit: Some(CurrencyUnit::Sat),
        method: PaymentMethod::Known(KnownMethod::Bolt11),
        amount_paid: Amount::ZERO,
        amount_issued: Amount::ZERO,
        updated_at: 0,
        state: MintQuoteState::Unpaid,
        expiry: None,
        pubkey: None,
    })
}

fn set_mint_quote_status_from_response(
    connector: &MockMintConnector,
    response: &MintQuoteResponse<String>,
) {
    match response {
        MintQuoteResponse::Bolt11(response) => {
            connector.set_mint_quote_status_response(Ok(response.clone()));
        }
        _ => panic!("expected bolt11 mint quote response"),
    }
}

fn push_mint_quote_status_from_response(
    connector: &MockMintConnector,
    response: &MintQuoteResponse<String>,
) {
    match response {
        MintQuoteResponse::Bolt11(response) => {
            connector.push_mint_quote_status_response(Ok(response.clone()));
        }
        _ => panic!("expected bolt11 mint quote response"),
    }
}

async fn persist_paid_mint_quote_to_federation(
    mints: &[Arc<Mint>],
    config: &FederationRuntimeConfig,
    journal: &Arc<tokio::sync::Mutex<InMemoryFederationJournal>>,
    quote_id: &QuoteId,
    amount: u64,
) {
    let entries = {
        let mut journal = journal.lock().await;
        let quote_entry = appended_entry(
            &mut journal,
            config.mint_quote_operation(
                quote_id.clone(),
                bolt11_quote_request(amount),
                bolt11_quote_response(quote_id, amount),
            ),
        );
        let paid_entry = appended_entry(
            &mut journal,
            config.mint_quote_payment_observation(
                quote_id.clone(),
                PaymentMethod::Known(KnownMethod::Bolt11),
                MintQuoteState::Paid,
                Amount::from(amount),
                Amount::ZERO,
                Some(format!("payment-{quote_id}")),
            ),
        );
        vec![quote_entry, paid_entry]
    };

    for mint in mints {
        for entry in &entries {
            persist_and_apply_entry(mint, entry).await;
        }
    }
}

async fn persist_unpaid_bolt11_melt_quote_to_federation(
    mints: &[Arc<Mint>],
    config: &FederationRuntimeConfig,
    journal: &Arc<tokio::sync::Mutex<InMemoryFederationJournal>>,
    quote_id: &QuoteId,
    invoice: &Bolt11Invoice,
    amount: u64,
    expiry: u64,
) {
    let request = MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
        request: invoice.clone(),
        unit: CurrencyUnit::Sat,
        options: None,
    });
    let response = MeltQuoteCreateResponse::Bolt11(MeltQuoteBolt11Response {
        quote: quote_id.clone(),
        amount: Amount::from(amount),
        fee_reserve: Amount::from(1),
        state: MeltQuoteState::Unpaid,
        expiry,
        payment_preimage: None,
        change: None,
        request: Some(invoice.to_string()),
        unit: Some(CurrencyUnit::Sat),
        method: PaymentMethod::Known(KnownMethod::Bolt11),
    });
    let metadata = FederationMeltQuoteMetadata {
        created_time: 1_700_000_000,
        request_lookup_id: Some(PaymentIdentifier::PaymentHash(
            *invoice.payment_hash().as_ref(),
        )),
        extra_json: None,
        estimated_blocks: None,
    };
    let entry = {
        let mut journal = journal.lock().await;
        appended_entry(
            &mut journal,
            config.melt_quote_operation(quote_id, request, response, metadata),
        )
    };

    for mint in mints {
        persist_and_apply_entry(mint, &entry).await;
    }
}

async fn fake_wallet_member_connectors_with_melt_quote(
    bundle: &TrustedDealerConfigBundle,
    mint_quote_id: &QuoteId,
    mint_quote_amount: u64,
    melt_quote_id: &QuoteId,
    melt_invoice: &Bolt11Invoice,
    melt_amount: u64,
) -> BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>> {
    let mut connectors = BTreeMap::new();
    let mut member_mints = BTreeMap::new();
    let melt_expiry = unix_time() + 10_000;
    for member_id in bundle.public_config.members.keys().copied() {
        let mint = Arc::new(create_fake_wallet_member_mint(bundle, member_id).await);
        member_mints.insert(member_id, mint);
    }

    let federation_mints = Arc::new(member_mints.values().cloned().collect::<Vec<_>>());
    let federation_journal = Arc::new(tokio::sync::Mutex::new(InMemoryFederationJournal::new()));
    let federation_config = federation_mints
        .first()
        .expect("federation mint")
        .federation_config()
        .expect("federation config")
        .clone();
    persist_unpaid_bolt11_melt_quote_to_federation(
        &federation_mints,
        &federation_config,
        &federation_journal,
        melt_quote_id,
        melt_invoice,
        melt_amount,
        melt_expiry,
    )
    .await;
    persist_paid_mint_quote_to_federation(
        &federation_mints,
        &federation_config,
        &federation_journal,
        mint_quote_id,
        mint_quote_amount,
    )
    .await;
    for (member_id, mint) in member_mints {
        connectors.insert(
            member_id,
            Arc::new(LocalFederatedMemberConnector::new_with_federation_mints(
                mint,
                federation_mints.clone(),
                federation_journal.clone(),
            )) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    connectors
}

async fn rotated_fake_wallet_member_connectors_with_state(
    bundle: &TrustedDealerConfigBundle,
    old_mint_quote_id: &QuoteId,
    new_mint_quote_id: &QuoteId,
    old_restore_output: &BlindedMessage,
    melt_quote_id: &QuoteId,
    melt_invoice: &Bolt11Invoice,
    melt_amount: u64,
) -> BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>> {
    let mut connectors = BTreeMap::new();
    let mut member_mints = BTreeMap::new();
    let melt_expiry = unix_time() + 10_000;
    let rotation_keyset = activated_rotation_keyset(bundle);

    for member_id in [member_id(1), member_id(2), member_id(3)] {
        let mint = Arc::new(create_fake_wallet_member_mint(bundle, member_id).await);
        member_mints.insert(member_id, mint);
    }

    let federation_mints = Arc::new(member_mints.values().cloned().collect::<Vec<_>>());
    let federation_journal = Arc::new(tokio::sync::Mutex::new(InMemoryFederationJournal::new()));
    let config = federation_mints
        .first()
        .expect("federation mint")
        .federation_config()
        .expect("federation config")
        .clone();
    persist_unpaid_bolt11_melt_quote_to_federation(
        &federation_mints,
        &config,
        &federation_journal,
        melt_quote_id,
        melt_invoice,
        melt_amount,
        melt_expiry,
    )
    .await;
    persist_paid_mint_quote_to_federation(
        &federation_mints,
        &config,
        &federation_journal,
        old_mint_quote_id,
        old_restore_output.amount.into(),
    )
    .await;
    persist_paid_mint_quote_to_federation(
        &federation_mints,
        &config,
        &federation_journal,
        new_mint_quote_id,
        3,
    )
    .await;

    let old_mint_input = MintInput::Single(MintRequest {
        quote: old_mint_quote_id.clone(),
        outputs: vec![old_restore_output.clone()],
        signature: None,
    });
    let setup_entries = {
        let mut journal = federation_journal.lock().await;
        let old_mint_entry = appended_entry(
            &mut journal,
            config
                .mint_operation(old_mint_input)
                .expect("old mint operation"),
        );

        let rotation_entry = appended_entry(
            &mut journal,
            config.operation_envelope(FederationOperation::KeysetRotation(
                FederationKeysetRotationOperation {
                    keyset: rotation_keyset.clone(),
                },
            )),
        );
        vec![old_mint_entry, rotation_entry]
    };
    for mint in federation_mints.iter() {
        for entry in &setup_entries {
            persist_and_apply_entry(mint, entry).await;
        }
    }

    for (member_id, mint) in member_mints {
        connectors.insert(
            member_id,
            Arc::new(LocalFederatedMemberConnector::new_with_federation_mints(
                mint,
                federation_mints.clone(),
                federation_journal.clone(),
            )) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    connectors
}

async fn settled_federation_melt_response(
    connector: &FederatedMintConnector,
    quote_id: &QuoteId,
    initial: MeltQuoteResponse<String>,
) -> MeltQuoteResponse<String> {
    if initial.state() == MeltQuoteState::Paid {
        return initial;
    }
    assert_eq!(initial.state(), MeltQuoteState::Pending);
    for _ in 0..100 {
        if let Ok(response) = connector
            .get_melt_quote_status(
                PaymentMethod::Known(KnownMethod::Bolt11),
                &quote_id.to_string(),
            )
            .await
        {
            if !matches!(
                response.state(),
                MeltQuoteState::Pending | MeltQuoteState::Unknown
            ) {
                return response;
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    panic!("federated melt did not reach a threshold terminal state")
}

fn assert_aggregate_response_valid(
    bundle: &TrustedDealerConfigBundle,
    outputs: &[BlindedMessage],
    response: &MintResponse,
) {
    assert_eq!(response.signatures.len(), outputs.len());
    for (signature, output) in response.signatures.iter().zip(outputs) {
        let keyset = bundle
            .public_config
            .keysets
            .iter()
            .find(|keyset| keyset.id == signature.keyset_id)
            .expect("signature keyset");
        let aggregate_key = keyset
            .keys()
            .amount_key(signature.amount)
            .expect("amount key");
        verify_bls_blind_signature(aggregate_key, signature.c, output.blinded_secret)
            .expect("aggregate signature verifies");
    }
}

#[test]
fn wallet_connector_accepts_matching_v3_aggregate_keysets() {
    let bundle = bundle();
    let advertised = advertised_keyset(&bundle);

    FederationWalletConnector::validate_member_keysets(
        &bundle.public_config,
        member_id(1),
        &[advertised],
    )
    .expect("matching member keysets");
}

#[test]
fn wallet_connector_rejects_non_v3_aggregate_keysets() {
    let bundle = bundle();
    let mut advertised = advertised_keyset(&bundle);
    let expected = &bundle.public_config.keysets[0];
    advertised.id = Id::v2_from_data(
        &advertised.keys,
        &expected.unit,
        expected.input_fee_ppk,
        expected.final_expiry,
    );

    assert!(matches!(
        FederationWalletConnector::validate_member_keysets(
            &bundle.public_config,
            member_id(1),
            &[advertised],
        ),
        Err(crate::Error::Federation(
            FederationError::UnknownFederatedKeyset { .. }
                | FederationError::InvalidFederatedKeysetVersion { .. }
        ))
    ));
}

#[test]
fn wallet_connector_rejects_mismatched_aggregate_keysets() {
    let bundle = bundle();
    let mut advertised = advertised_keyset(&bundle);
    advertised.keys = cdk_common::Keys::new(BTreeMap::new());

    assert!(matches!(
        FederationWalletConnector::validate_member_keysets(
            &bundle.public_config,
            member_id(1),
            &[advertised],
        ),
        Err(crate::Error::Federation(
            FederationError::Nut02(_) | FederationError::AdvertisedFederatedKeysetMismatch { .. }
        ))
    ));
}

#[test]
fn federated_mint_connector_rejects_invalid_member_call_policy() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let connectors = member_connectors(&bundle, &outputs);

    for config in [
        FederatedMintConnectorConfig {
            member_fallback_timeout: Duration::ZERO,
            ..FederatedMintConnectorConfig::default()
        },
        FederatedMintConnectorConfig {
            member_request_timeout: Duration::ZERO,
            ..FederatedMintConnectorConfig::default()
        },
        FederatedMintConnectorConfig {
            quote_status_member_timeout: Duration::ZERO,
            ..FederatedMintConnectorConfig::default()
        },
        FederatedMintConnectorConfig {
            fanout_concurrency: 0,
            ..FederatedMintConnectorConfig::default()
        },
        FederatedMintConnectorConfig {
            mint_quote_visibility_attempts: 0,
            ..FederatedMintConnectorConfig::default()
        },
        FederatedMintConnectorConfig {
            mint_quote_visibility_retry_delay: Duration::ZERO,
            ..FederatedMintConnectorConfig::default()
        },
        FederatedMintConnectorConfig {
            quote_coordinator_member_id: Some(member_id(99)),
            ..FederatedMintConnectorConfig::default()
        },
    ] {
        assert!(FederatedMintConnector::new_with_config(
            bundle.public_config.clone(),
            connectors.clone(),
            config,
        )
        .is_err());
    }
}

#[test]
fn federated_mint_connector_rejects_tampered_aggregate_key() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let connectors = member_connectors(&bundle, &outputs);
    let mut public_config = bundle.public_config.clone();
    let amount = Amount::from(1);
    let wrong_aggregate_key = *public_config.keysets[0]
        .aggregate_keys
        .get(&Amount::from(2))
        .expect("alternate aggregate key");
    public_config.keysets[0]
        .aggregate_keys
        .insert(amount, wrong_aggregate_key);

    assert!(FederatedMintConnector::new(public_config, connectors).is_err());
}

#[test]
fn federated_mint_connector_rejects_tampered_public_config_urls() {
    let bundle = threshold_bundle();
    let public_config = public_config_with_mismatched_public_share(&bundle);

    assert!(FederatedMintConnector::from_public_config_urls(public_config).is_err());
}

#[test]
fn federated_mint_connector_trusted_public_config_urls_skip_full_aggregate_validation() {
    let bundle = threshold_bundle();
    let public_config = public_config_with_mismatched_public_share(&bundle);

    let connector = FederatedMintConnector::from_validated_public_config_urls_with_config(
        public_config.clone(),
        FederatedMintConnectorConfig::default(),
    )
    .expect("trusted local-registry config should skip repeated aggregate validation");

    assert_eq!(connector.public_config(), &public_config);
}

#[test]
fn federated_mint_connector_builds_http_members_from_invite_code() {
    let bundle = threshold_bundle();
    let invite = FederationInvite::new(bundle.public_config.clone()).expect("invite");
    let invite_code = invite.to_invite_code().expect("invite code");
    let config = FederatedMintConnectorConfig {
        quote_coordinator_member_id: Some(member_id(2)),
        fanout_concurrency: 2,
        ..FederatedMintConnectorConfig::default()
    };

    let connector = FederatedMintConnector::from_invite_code_with_config(&invite_code, config)
        .expect("connector from invite");

    assert_eq!(connector.public_config(), &bundle.public_config);
    assert_eq!(connector.connector_config(), config);
    assert_eq!(connector.quote_coordinator_member_id, member_id(2));
    assert_eq!(
        connector
            .member_connectors
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        vec![member_id(1), member_id(2), member_id(3)]
    );
}

#[test]
fn federated_mint_connector_rejects_invalid_invite_code() {
    assert!(FederatedMintConnector::from_invite_code("cashuAunsupported").is_err());
}

#[test]
fn federated_mint_connector_rejects_unsupported_wallet_protocol_version() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let connectors = member_connectors(&bundle, &outputs);
    let mut public_config = bundle.public_config.clone();
    let expected = public_config.wallet_protocol_version;
    let found = expected.saturating_add(1);
    public_config.wallet_protocol_version = found;

    assert!(matches!(
        FederatedMintConnector::new(public_config, connectors),
        Err(crate::Error::Federation(
            FederationError::ProtocolVersionMismatch {
                component: "wallet",
                expected: actual_expected,
                found: actual_found,
            }
        )) if actual_expected == expected && actual_found == found
    ));
}

#[tokio::test]
async fn federated_mint_connector_aggregates_mint_signature_shares() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let connector = FederatedMintConnector::new(
        bundle.public_config.clone(),
        member_connectors(&bundle, &outputs),
    )
    .expect("federated mint connector");
    let response = connector
        .post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: "paid-quote".to_string(),
                outputs: outputs.clone(),
                signature: None,
            },
        )
        .await
        .expect("mint response");

    assert_aggregate_response_valid(&bundle, &outputs, &response);
}

#[tokio::test]
async fn federated_mint_connector_tolerates_offline_member_key_discovery() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    let offline_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(
        &offline_member,
        &bundle,
        member_id(1),
        advertised.clone(),
    );
    offline_member.set_mint_keys_error("member unavailable");
    offline_member
        .set_post_mint_response(Err(crate::Error::Custom("member unavailable".to_string())));
    connectors.insert(
        member_id(1),
        Arc::new(offline_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for member_id in [member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.set_post_mint_response(Ok(share_response(&bundle, member_id, &outputs)));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = connector
        .post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: "paid-quote".to_string(),
                outputs: outputs.clone(),
                signature: None,
            },
        )
        .await
        .expect("mint response");

    assert_aggregate_response_valid(&bundle, &outputs, &response);
}

#[tokio::test]
async fn federated_mint_connector_gets_mint_info_from_first_healthy_member() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let expected = federation_mint_info(&bundle, member_id(2)).name("healthy federation member");
    let mut connectors = BTreeMap::new();

    let unavailable_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(
        &unavailable_member,
        &bundle,
        member_id(1),
        advertised.clone(),
    );
    unavailable_member.set_mint_info_error("first member unavailable");
    connectors.insert(
        member_id(1),
        Arc::new(unavailable_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let healthy_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(
        &healthy_member,
        &bundle,
        member_id(2),
        advertised.clone(),
    );
    healthy_member.set_mint_info_response(Ok(expected.clone()));
    connectors.insert(
        member_id(2),
        Arc::new(healthy_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let unused_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&unused_member, &bundle, member_id(3), advertised);
    connectors.insert(
        member_id(3),
        Arc::new(unused_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = connector.get_mint_info().await.expect("mint info");

    assert_eq!(response, expected);
}

#[tokio::test]
async fn federated_mint_connector_fetches_lnurl_pay_request_from_first_healthy_member() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    let unavailable_member = MockMintConnector::new();
    unavailable_member.set_active_keyset(advertised.clone());
    unavailable_member.set_lnurl_pay_request_response(Err(crate::Error::Custom(
        "first member unavailable".to_string(),
    )));
    connectors.insert(
        member_id(1),
        Arc::new(unavailable_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let healthy_member = MockMintConnector::new();
    healthy_member.set_active_keyset(advertised.clone());
    healthy_member.set_lnurl_pay_request_response(Ok(crate::lightning_address::LnurlPayResponse {
        callback: "https://example.com/callback".to_string(),
        min_sendable: 1,
        max_sendable: 1_000,
        metadata: "[]".to_string(),
        tag: Some("payRequest".to_string()),
        reason: None,
    }));
    connectors.insert(
        member_id(2),
        Arc::new(healthy_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let unused_member = MockMintConnector::new();
    unused_member.set_active_keyset(advertised);
    connectors.insert(
        member_id(3),
        Arc::new(unused_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = connector
        .fetch_lnurl_pay_request("https://example.com/.well-known/lnurlp/alice")
        .await
        .expect("lnurl pay request");

    assert_eq!(response.callback, "https://example.com/callback");
    assert_eq!(response.min_sendable, 1);
    assert_eq!(response.max_sendable, 1_000);
    assert_eq!(response.metadata, "[]");
}

#[tokio::test]
async fn federated_mint_connector_fetches_lnurl_invoice_from_first_healthy_member() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    let unavailable_member = MockMintConnector::new();
    unavailable_member.set_active_keyset(advertised.clone());
    unavailable_member.set_lnurl_invoice_response(Err(crate::Error::Custom(
        "first member unavailable".to_string(),
    )));
    connectors.insert(
        member_id(1),
        Arc::new(unavailable_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let healthy_member = MockMintConnector::new();
    healthy_member.set_active_keyset(advertised.clone());
    healthy_member.set_lnurl_invoice_response(Ok(
        crate::lightning_address::LnurlPayInvoiceResponse {
            pr: Some("bolt11-invoice".to_string()),
            success_action: None,
            routes: None,
            reason: None,
        },
    ));
    connectors.insert(
        member_id(2),
        Arc::new(healthy_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let unused_member = MockMintConnector::new();
    unused_member.set_active_keyset(advertised);
    connectors.insert(
        member_id(3),
        Arc::new(unused_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = connector
        .fetch_lnurl_invoice("https://example.com/callback?amount=1000")
        .await
        .expect("lnurl invoice");

    assert_eq!(response.pr.as_deref(), Some("bolt11-invoice"));
    assert!(response.success_action.is_none());
    assert!(response.routes.is_none());
    assert!(response.reason.is_none());
}

#[tokio::test]
async fn federated_mint_connector_polls_member_keysets_concurrently() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let mut connectors = BTreeMap::new();

    for member_id in [member_id(1), member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.set_mint_keys_barrier(barrier.clone());
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response =
        tokio::time::timeout(std::time::Duration::from_secs(1), connector.get_mint_keys())
            .await
            .expect("member keyset probes should run concurrently")
            .expect("aggregate keysets");

    assert_eq!(response.len(), 1);
    assert_eq!(response[0].id, advertised.id);
}

#[tokio::test(start_paused = true)]
async fn federated_mint_connector_times_out_stalled_mint_info_validation() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let request_timeout = Duration::from_secs(2);
    let mut connectors = BTreeMap::new();

    let stalled_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(
        &stalled_member,
        &bundle,
        member_id(1),
        advertised.clone(),
    );
    stalled_member.set_mint_info_barrier(Arc::new(tokio::sync::Barrier::new(2)));
    connectors.insert(
        member_id(1),
        Arc::new(stalled_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for member_id in [member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            member_request_timeout: request_timeout,
            fanout_concurrency: 1,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let validation = tokio::spawn(async move { connector.get_mint_keys().await });

    tokio::task::yield_now().await;
    tokio::time::advance(request_timeout).await;

    let response = validation
        .await
        .expect("keyset validation task")
        .expect("healthy threshold after mint info timeout");
    assert_eq!(response.len(), 1);
    assert_eq!(response[0].id, advertised.id);
}

#[tokio::test(start_paused = true)]
async fn federated_mint_connector_times_out_stalled_mint_keys_validation() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let request_timeout = Duration::from_secs(2);
    let mut connectors = BTreeMap::new();

    let stalled_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(
        &stalled_member,
        &bundle,
        member_id(1),
        advertised.clone(),
    );
    stalled_member.set_mint_keys_barrier(Arc::new(tokio::sync::Barrier::new(2)));
    connectors.insert(
        member_id(1),
        Arc::new(stalled_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for member_id in [member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            member_request_timeout: request_timeout,
            fanout_concurrency: 1,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let validation = tokio::spawn(async move { connector.get_mint_keys().await });

    tokio::task::yield_now().await;
    tokio::time::advance(request_timeout).await;

    let response = validation
        .await
        .expect("keyset validation task")
        .expect("healthy threshold after mint key timeout");
    assert_eq!(response.len(), 1);
    assert_eq!(response[0].id, advertised.id);
}

#[tokio::test]
async fn federated_mint_connector_mints_after_threshold_keyset_advertisements() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    for member_id in [member_id(1), member_id(2)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.set_post_mint_response(Ok(share_response(&bundle, member_id, &outputs)));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let slow_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&slow_member, &bundle, member_id(3), advertised.clone());
    slow_member.set_mint_keys_barrier(Arc::new(tokio::sync::Barrier::new(2)));
    slow_member.set_post_mint_response(Ok(share_response(&bundle, member_id(3), &outputs)));
    connectors.insert(
        member_id(3),
        Arc::new(slow_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        connector.post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: "paid-quote".to_string(),
                outputs: outputs.clone(),
                signature: None,
            },
        ),
    )
    .await
    .expect("mint should not wait for every keyset advertisement")
    .expect("mint response");

    assert_eq!(response.signatures.len(), outputs.len());
}

#[tokio::test]
async fn federated_mint_connector_reuses_validated_member_keysets() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();
    let mut raw_connectors = Vec::new();

    for member_id in [member_id(1), member_id(2), member_id(3)] {
        let connector = Arc::new(MockMintConnector::new());
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.push_post_mint_response(Ok(share_response(&bundle, member_id, &outputs)));
        connector.push_post_mint_response(Ok(share_response(&bundle, member_id, &outputs)));
        raw_connectors.push(connector.clone());
        connectors.insert(member_id, connector as Arc<dyn MintConnector + Send + Sync>);
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    connector
        .post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: "paid-quote".to_string(),
                outputs: outputs.clone(),
                signature: None,
            },
        )
        .await
        .expect("first mint response");

    for connector in raw_connectors {
        connector.set_mint_keys_error("keyset probe should use cache");
    }

    let response = connector
        .post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: "paid-quote-2".to_string(),
                outputs: outputs.clone(),
                signature: None,
            },
        )
        .await
        .expect("second mint response should reuse validated keysets");

    assert_aggregate_response_valid(&bundle, &outputs, &response);
}

#[tokio::test]
async fn federated_mint_connector_excludes_bad_keyset_member_from_mint_fanout() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let mut bad_advertised = advertised.clone();
    bad_advertised.input_fee_ppk = bad_advertised.input_fee_ppk.saturating_add(1);
    let mut connectors = BTreeMap::new();

    for member_id in [member_id(1), member_id(2)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.set_post_mint_response(Ok(share_response(&bundle, member_id, &outputs)));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let bad_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&bad_member, &bundle, member_id(3), bad_advertised);
    bad_member.set_post_mint_barrier(Arc::new(tokio::sync::Barrier::new(2)));
    bad_member.set_post_mint_response(Ok(share_response(&bundle, member_id(3), &outputs)));
    connectors.insert(
        member_id(3),
        Arc::new(bad_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        connector.post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: "paid-quote".to_string(),
                outputs: outputs.clone(),
                signature: None,
            },
        ),
    )
    .await
    .expect("bad keyset member should not receive mint fanout")
    .expect("mint response");

    assert_aggregate_response_valid(&bundle, &outputs, &response);
}

#[tokio::test]
async fn federated_mint_connector_gets_keyset_after_threshold_member_advertisements() {
    let bundle = threshold_bundle();
    let keyset_id = bundle.public_config.keysets[0].id;
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    for member_id in [member_id(1), member_id(2)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let slow_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&slow_member, &bundle, member_id(3), advertised.clone());
    slow_member.set_mint_keys_barrier(Arc::new(tokio::sync::Barrier::new(2)));
    connectors.insert(
        member_id(3),
        Arc::new(slow_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        connector.get_mint_keyset(keyset_id),
    )
    .await
    .expect("keyset fetch should not wait for every member")
    .expect("keyset");

    assert_eq!(response.id, advertised.id);
}

#[tokio::test]
async fn federated_mint_connector_fans_out_mint_members_concurrently() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let mut connectors = BTreeMap::new();

    for member_id in [member_id(1), member_id(2)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.set_post_mint_barrier(barrier.clone());
        connector.set_post_mint_response(Ok(share_response(&bundle, member_id, &outputs)));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let failing_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&failing_member, &bundle, member_id(3), advertised);
    failing_member
        .set_post_mint_response(Err(crate::Error::Custom("member unavailable".to_string())));
    connectors.insert(
        member_id(3),
        Arc::new(failing_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        connector.post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: "paid-quote".to_string(),
                outputs: outputs.clone(),
                signature: None,
            },
        ),
    )
    .await
    .expect("mint member requests should run concurrently")
    .expect("mint response");

    assert_aggregate_response_valid(&bundle, &outputs, &response);
}

#[tokio::test]
async fn federated_mint_connector_aggregates_batch_mint_signature_shares() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let connector = FederatedMintConnector::new(
        bundle.public_config.clone(),
        member_connectors(&bundle, &outputs),
    )
    .expect("federated mint connector");
    let response = connector
        .post_batch_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            BatchMintRequest {
                quotes: vec!["paid-quote-1".to_string(), "paid-quote-2".to_string()],
                quote_amounts: None,
                outputs: outputs.clone(),
                signatures: None,
            },
        )
        .await
        .expect("batch mint response");

    assert_aggregate_response_valid(&bundle, &outputs, &response);
}

#[tokio::test]
async fn federated_mint_connector_aggregates_fake_wallet_member_shares() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let quote_id = QuoteId::new();
    let melt_quote_id = QuoteId::new();
    let melt_invoice = create_fake_invoice(1000, "".to_string());
    let connector = FederatedMintConnector::new(
        bundle.public_config.clone(),
        fake_wallet_member_connectors_with_melt_quote(
            &bundle,
            &quote_id,
            3,
            &melt_quote_id,
            &melt_invoice,
            1,
        )
        .await,
    )
    .expect("federated mint connector");

    let fake_quote = connector
        .post_mint_quote(MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
            amount: Amount::from(3),
            unit: CurrencyUnit::Sat,
            description: None,
            pubkey: None,
        }))
        .await
        .expect("fake wallet mint quote");
    let fake_quote_id = fake_quote.quote().to_string();
    let mut settled_state = fake_quote.state();
    for _ in 0..100 {
        let status = connector
            .get_mint_quote_status(PaymentMethod::Known(KnownMethod::Bolt11), &fake_quote_id)
            .await
            .expect("fake wallet quote status");
        settled_state = status.state();
        if settled_state == Some(MintQuoteState::Paid) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(settled_state, Some(MintQuoteState::Paid));

    let mint_response = connector
        .post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: quote_id.to_string(),
                outputs: outputs.clone(),
                signature: None,
            },
        )
        .await
        .expect("mint response");
    assert_aggregate_response_valid(&bundle, &outputs, &mint_response);

    let batch_response = connector
        .post_batch_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            BatchMintRequest {
                quotes: vec![fake_quote_id],
                quote_amounts: None,
                outputs: outputs.clone(),
                signatures: None,
            },
        )
        .await
        .expect("batch mint response");
    assert_aggregate_response_valid(&bundle, &outputs, &batch_response);

    let swap_outputs = blinded_outputs_for_keyset(&active_keyset(&bundle), b"fake-wallet-swap");
    let swap_response = connector
        .post_swap(SwapRequest::new(
            vec![
                proof_for_amount(&bundle, 1, "fake-wallet-swap-one"),
                proof_for_amount(&bundle, 2, "fake-wallet-swap-two"),
            ],
            swap_outputs.clone(),
        ))
        .await
        .expect("swap response");
    assert_aggregate_response_valid(
        &bundle,
        &swap_outputs,
        &MintResponse {
            signatures: swap_response.signatures,
        },
    );

    let change_outputs = vec![blinded_change_outputs(&bundle)
        .into_iter()
        .next()
        .expect("change output")];
    let initial_melt_response = connector
        .post_melt(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MeltRequest::new(
                melt_quote_id.to_string(),
                vec![proof_for_amount(&bundle, 4, "fake-wallet-melt")],
                Some(change_outputs.clone()),
            ),
        )
        .await
        .expect("melt response");
    let melt_response =
        settled_federation_melt_response(&connector, &melt_quote_id, initial_melt_response).await;
    let change = melt_response.change().cloned().expect("change signatures");
    let mut signed_change_output = change_outputs[0].clone();
    signed_change_output.amount = Amount::from(2);
    assert_eq!(melt_response.state(), MeltQuoteState::Paid);
    assert_aggregate_response_valid(
        &bundle,
        &[signed_change_output],
        &MintResponse { signatures: change },
    );
}

#[tokio::test]
async fn federated_mint_connector_aggregates_rotated_fake_wallet_member_shares() {
    let bundle = threshold_rotation_bundle();
    let old_keyset = active_keyset(&bundle);
    let next_keyset = activated_rotation_keyset(&bundle);
    let old_mint_quote_id = QuoteId::new();
    let new_mint_quote_id = QuoteId::new();
    let melt_quote_id = QuoteId::new();
    let melt_invoice = create_fake_invoice(1000, "".to_string());
    let old_restore_output =
        blinded_output_for_keyset_amount(&old_keyset, 4, b"rotated wallet old restore");
    let next_outputs = blinded_outputs_for_keyset(&next_keyset, b"rotated wallet output");
    let next_swap_outputs = vec![
        blinded_output_for_keyset_amount(&next_keyset, 2, b"rotated wallet swap output a"),
        blinded_output_for_keyset_amount(&next_keyset, 2, b"rotated wallet swap output b"),
    ];
    let next_change_outputs =
        vec![
            blinded_change_outputs_for_keyset(&next_keyset, b"rotated wallet melt change")
                .into_iter()
                .next()
                .expect("next change output"),
        ];
    let connector = FederatedMintConnector::new(
        rotated_public_config(&bundle),
        rotated_fake_wallet_member_connectors_with_state(
            &bundle,
            &old_mint_quote_id,
            &new_mint_quote_id,
            &old_restore_output,
            &melt_quote_id,
            &melt_invoice,
            1,
        )
        .await,
    )
    .expect("rotated federated mint connector");

    let advertised = connector.get_mint_keys().await.expect("active keysets");
    assert_eq!(advertised.len(), 1);
    assert_eq!(advertised[0].id, next_keyset.id);

    let mint_response = connector
        .post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: new_mint_quote_id.to_string(),
                outputs: next_outputs.clone(),
                signature: None,
            },
        )
        .await
        .expect("rotated mint response");
    assert_aggregate_response_valid(&bundle, &next_outputs, &mint_response);

    let swap_response = connector
        .post_swap(SwapRequest::new(
            vec![proof_for_amount(&bundle, 4, "rotated-wallet-swap")],
            next_swap_outputs.clone(),
        ))
        .await
        .expect("rotated swap response");
    assert_aggregate_response_valid(
        &bundle,
        &next_swap_outputs,
        &MintResponse {
            signatures: swap_response.signatures,
        },
    );

    let initial_melt_response = connector
        .post_melt(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MeltRequest::new(
                melt_quote_id.to_string(),
                vec![proof_for_amount(&bundle, 4, "rotated-wallet-melt")],
                Some(next_change_outputs.clone()),
            ),
        )
        .await
        .expect("rotated melt response");
    let melt_response =
        settled_federation_melt_response(&connector, &melt_quote_id, initial_melt_response).await;
    let change = melt_response.change().cloned().expect("change signatures");
    let mut signed_change_output = next_change_outputs[0].clone();
    signed_change_output.amount = Amount::from(2);
    assert_eq!(melt_response.state(), MeltQuoteState::Paid);
    assert_aggregate_response_valid(
        &bundle,
        &[signed_change_output],
        &MintResponse { signatures: change },
    );

    let restore_response = connector
        .post_restore(RestoreRequest {
            outputs: vec![old_restore_output.clone()],
        })
        .await
        .expect("rotated restore response");
    assert_eq!(restore_response.outputs, vec![old_restore_output.clone()]);
    assert_aggregate_response_valid(
        &bundle,
        &[old_restore_output],
        &MintResponse {
            signatures: restore_response.signatures,
        },
    );
}

#[tokio::test]
async fn federated_mint_connector_maps_rotated_stale_outputs_to_inactive_keyset() {
    let bundle = threshold_rotation_bundle();
    let old_keyset = active_keyset(&bundle);
    let stale_outputs = vec![blinded_output_for_keyset_amount(
        &old_keyset,
        4,
        b"rotated stale wallet output",
    )];
    let connector = FederatedMintConnector::new(
        rotated_public_config(&bundle),
        member_connectors_with_only_keysets(&bundle),
    )
    .expect("rotated federated mint connector");

    let result = connector
        .post_swap(SwapRequest::new(
            vec![proof_for_amount(&bundle, 4, "rotated-stale-swap")],
            stale_outputs,
        ))
        .await;

    assert!(
        matches!(result, Err(crate::Error::InactiveKeyset)),
        "stale rotated swap outputs should retry through InactiveKeyset, got {result:?}"
    );
}

#[tokio::test]
async fn federated_wallet_swaps_restored_old_keyset_proofs_after_rotation() {
    let bundle = threshold_rotation_bundle();
    let old_keyset = active_keyset(&bundle);
    let next_keyset = activated_rotation_keyset(&bundle);
    let seed = [42u8; 64];
    let restore_secrets =
        PreMintSecrets::restore_batch(old_keyset.id, &seed, 0, 1).expect("restore secrets");
    let restore_output = restore_secrets
        .blinded_messages()
        .into_iter()
        .next()
        .expect("restore output");
    let mut old_mint_output = restore_output.clone();
    old_mint_output.amount = Amount::from(4);
    let old_mint_quote_id = QuoteId::new();
    let new_mint_quote_id = QuoteId::new();
    let melt_quote_id = QuoteId::new();
    let melt_invoice = create_fake_invoice(1000, "".to_string());
    let connector = Arc::new(
        FederatedMintConnector::new(
            rotated_public_config(&bundle),
            rotated_fake_wallet_member_connectors_with_state(
                &bundle,
                &old_mint_quote_id,
                &new_mint_quote_id,
                &old_mint_output,
                &melt_quote_id,
                &melt_invoice,
                1,
            )
            .await,
        )
        .expect("rotated federated mint connector"),
    );
    let first_member = bundle
        .public_config
        .members
        .values()
        .next()
        .expect("first member");
    let localstore = Arc::new(
        cdk_sqlite::wallet::memory::empty()
            .await
            .expect("wallet db"),
    );
    let wallet = WalletBuilder::new()
        .mint_url(first_member.public_mint_url.clone())
        .unit(CurrencyUnit::Sat)
        .localstore(localstore)
        .seed(seed)
        .shared_client(connector)
        .build()
        .expect("wallet");

    let restored = wallet
        .restore_with_opts(crate::wallet::NUT13Options::new(1, 1).expect("restore opts"))
        .await
        .expect("restore old-keyset proof");
    assert_eq!(restored.unspent, Amount::from(4));
    assert_eq!(
        wallet
            .active_keyset()
            .await
            .expect("active keyset after rotation")
            .id,
        next_keyset.id
    );
    let old_proofs = wallet.get_unspent_proofs().await.expect("old proofs");
    assert_eq!(
        old_proofs.total_amount().expect("old proof amount"),
        Amount::from(4)
    );

    wallet
        .swap(
            None,
            crate::amount::SplitTarget::default(),
            old_proofs.clone(),
            None,
            false,
            false,
        )
        .await
        .expect("swap restored old-keyset proofs");
    let spent_states = wallet
        .check_proofs_spent(old_proofs)
        .await
        .expect("spent states");
    assert!(spent_states
        .iter()
        .all(|proof_state| proof_state.state == State::Spent));
}

#[tokio::test]
async fn federated_mint_connector_aggregates_rotated_http_member_shares() {
    let bundle = threshold_rotation_bundle();
    let old_keyset = active_keyset(&bundle);
    let next_keyset = activated_rotation_keyset(&bundle);
    let old_mint_quote_id = QuoteId::new();
    let new_mint_quote_id = QuoteId::new();
    let melt_quote_id = QuoteId::new();
    let melt_invoice = create_fake_invoice(1000, "".to_string());
    let old_restore_output =
        blinded_output_for_keyset_amount(&old_keyset, 4, b"rotated http old restore");
    let next_outputs = blinded_outputs_for_keyset(&next_keyset, b"rotated http output");
    let next_swap_outputs = vec![
        blinded_output_for_keyset_amount(&next_keyset, 2, b"rotated http swap output a"),
        blinded_output_for_keyset_amount(&next_keyset, 2, b"rotated http swap output b"),
    ];
    let next_change_outputs =
        vec![
            blinded_change_outputs_for_keyset(&next_keyset, b"rotated http melt change")
                .into_iter()
                .next()
                .expect("next change output"),
        ];
    let local_connectors = rotated_fake_wallet_member_connectors_with_state(
        &bundle,
        &old_mint_quote_id,
        &new_mint_quote_id,
        &old_restore_output,
        &melt_quote_id,
        &melt_invoice,
        1,
    )
    .await;
    let connector = FederatedMintConnector::new(
        rotated_public_config(&bundle),
        http_member_connectors(local_connectors),
    )
    .expect("rotated federated HTTP mint connector");

    let advertised = connector
        .get_mint_keyset(next_keyset.id)
        .await
        .expect("next keyset over HTTP");
    assert_eq!(advertised.id, next_keyset.id);

    let mint_response = connector
        .post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: new_mint_quote_id.to_string(),
                outputs: next_outputs.clone(),
                signature: None,
            },
        )
        .await
        .expect("rotated HTTP mint response");
    assert_aggregate_response_valid(&bundle, &next_outputs, &mint_response);

    let swap_response = connector
        .post_swap(SwapRequest::new(
            vec![proof_for_amount(&bundle, 4, "rotated-http-swap")],
            next_swap_outputs.clone(),
        ))
        .await
        .expect("rotated HTTP swap response");
    assert_aggregate_response_valid(
        &bundle,
        &next_swap_outputs,
        &MintResponse {
            signatures: swap_response.signatures,
        },
    );

    let melt_proof = proof_for_amount(&bundle, 4, "rotated-http-melt");
    let melt_y = melt_proof.y().expect("melt proof y");
    let initial_melt_response = connector
        .post_melt(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MeltRequest::new(
                melt_quote_id.to_string(),
                vec![melt_proof],
                Some(next_change_outputs.clone()),
            ),
        )
        .await
        .expect("rotated HTTP melt response");
    let melt_response =
        settled_federation_melt_response(&connector, &melt_quote_id, initial_melt_response).await;
    let change = melt_response.change().cloned().expect("change signatures");
    let mut signed_change_output = next_change_outputs[0].clone();
    signed_change_output.amount = Amount::from(2);
    assert_eq!(melt_response.state(), MeltQuoteState::Paid);
    assert_aggregate_response_valid(
        &bundle,
        &[signed_change_output],
        &MintResponse { signatures: change },
    );
    let melt_status = connector
        .get_melt_quote_status(
            PaymentMethod::Known(KnownMethod::Bolt11),
            &melt_quote_id.to_string(),
        )
        .await
        .expect("rotated HTTP melt status");
    assert_eq!(melt_status.state(), MeltQuoteState::Paid);
    let spent_state = connector
        .post_check_state(CheckStateRequest { ys: vec![melt_y] })
        .await
        .expect("rotated HTTP check-state");
    assert_eq!(spent_state.states.len(), 1);
    assert_eq!(spent_state.states[0].state, State::Spent);

    let restore_response = connector
        .post_restore(RestoreRequest {
            outputs: vec![old_restore_output.clone()],
        })
        .await
        .expect("rotated HTTP restore response");
    assert_eq!(restore_response.outputs, vec![old_restore_output.clone()]);
    assert_aggregate_response_valid(
        &bundle,
        &[old_restore_output],
        &MintResponse {
            signatures: restore_response.signatures,
        },
    );
}

#[tokio::test]
async fn federated_mint_connector_preserves_threshold_inactive_keyset_rejections() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let outputs = blinded_outputs(&bundle);
    let mut connectors = BTreeMap::new();

    for member_id in [member_id(1), member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.set_post_swap_response(Err(crate::Error::InactiveKeyset));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let result = connector
        .post_swap(SwapRequest::new(Vec::new(), outputs))
        .await;

    assert!(
        matches!(result, Err(crate::Error::InactiveKeyset)),
        "threshold inactive-keyset member rejections should remain retryable, got {result:?}"
    );
}

#[tokio::test]
async fn federated_mint_connector_aggregates_swap_signature_shares() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let connector = FederatedMintConnector::new(
        bundle.public_config.clone(),
        member_connectors(&bundle, &outputs),
    )
    .expect("federated mint connector");
    let response = connector
        .post_swap(SwapRequest::new(Vec::new(), outputs.clone()))
        .await
        .expect("swap response");

    assert_aggregate_response_valid(
        &bundle,
        &outputs,
        &MintResponse {
            signatures: response.signatures,
        },
    );
}

#[tokio::test]
async fn local_federated_members_reject_sliding_window_swap_output_attack() {
    let bundle = threshold_four_bundle();
    let keyset = active_keyset(&bundle);
    let outputs = [
        blinded_output_for_keyset_amount(&keyset, 1, b"sliding-window-swap-output-a"),
        blinded_output_for_keyset_amount(&keyset, 1, b"sliding-window-swap-output-b"),
        blinded_output_for_keyset_amount(&keyset, 1, b"sliding-window-swap-output-c"),
        blinded_output_for_keyset_amount(&keyset, 1, b"sliding-window-swap-output-d"),
    ];
    let inputs = vec![
        proof_for_amount(&bundle, 1, "sliding-window-swap-input-a"),
        proof_for_amount(&bundle, 1, "sliding-window-swap-input-b"),
        proof_for_amount(&bundle, 1, "sliding-window-swap-input-c"),
    ];
    let mint_quote_id = QuoteId::new();
    let melt_quote_id = QuoteId::new();
    let melt_invoice = create_fake_invoice(1000, "".to_string());
    let connectors = fake_wallet_member_connectors_with_melt_quote(
        &bundle,
        &mint_quote_id,
        3,
        &melt_quote_id,
        &melt_invoice,
        1000,
    )
    .await;
    let views = [
        (
            member_id(1),
            vec![outputs[0].clone(), outputs[1].clone(), outputs[2].clone()],
        ),
        (
            member_id(2),
            vec![outputs[1].clone(), outputs[2].clone(), outputs[3].clone()],
        ),
        (
            member_id(3),
            vec![outputs[2].clone(), outputs[3].clone(), outputs[0].clone()],
        ),
        (
            member_id(4),
            vec![outputs[3].clone(), outputs[0].clone(), outputs[1].clone()],
        ),
    ];
    let mut successes = Vec::new();
    let mut failures = Vec::new();

    for (member_id, member_outputs) in views {
        let connector = connectors
            .get(&member_id)
            .expect("member connector")
            .clone();
        match connector
            .post_swap(SwapRequest::new(inputs.clone(), member_outputs.clone()))
            .await
        {
            Ok(response) => successes.push((member_id, member_outputs, response)),
            Err(err) => failures.push((member_id, err.to_string())),
        }
    }

    assert_eq!(
        successes.len(),
        1,
        "only the first consensus-accepted swap view may receive shares"
    );
    assert_eq!(
        failures.len(),
        3,
        "overlapping swap views must not receive enough shares to aggregate"
    );
    for (_, error) in &failures {
        assert!(
            error.contains("Token Already Spent")
                || error.contains("missing persisted federation signature share"),
            "losing sliding-window swap view should fail before returning shares: {error}"
        );
    }

    let (_, winning_outputs, _) = successes.first().expect("winning view");
    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let aggregate = connector
        .post_swap(SwapRequest::new(inputs, winning_outputs.clone()))
        .await
        .expect("accepted swap view should aggregate across members");

    assert_aggregate_response_valid(
        &bundle,
        winning_outputs,
        &MintResponse {
            signatures: aggregate.signatures,
        },
    );
}

#[tokio::test]
async fn federated_mint_connector_aggregates_melt_change_signature_shares() {
    let bundle = threshold_bundle();
    let change_outputs = blinded_change_outputs(&bundle);
    let connector = FederatedMintConnector::new(
        bundle.public_config.clone(),
        member_connectors_with_melt_change(&bundle, &change_outputs),
    )
    .expect("federated mint connector");
    let response = connector
        .post_melt(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MeltRequest::new(
                "melt-quote".to_string(),
                Vec::new(),
                Some(change_outputs.clone()),
            ),
        )
        .await
        .expect("melt response");
    let change = response.change().cloned().expect("change signatures");

    assert_aggregate_response_valid(
        &bundle,
        &signed_change_outputs(&change_outputs),
        &MintResponse { signatures: change },
    );
}

#[tokio::test]
async fn federated_mint_connector_fans_out_melt_without_change() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    let failing_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(
        &failing_member,
        &bundle,
        member_id(1),
        advertised.clone(),
    );
    failing_member
        .set_post_melt_response(Err(crate::Error::Custom("member unavailable".to_string())));
    connectors.insert(
        member_id(1),
        Arc::new(failing_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for member_id in [member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.set_post_melt_response(Ok(melt_response_without_change()));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = connector
        .post_melt(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MeltRequest::new("melt-quote".to_string(), Vec::new(), None),
        )
        .await
        .expect("melt response");

    assert_eq!(response.state(), MeltQuoteState::Paid);
    assert!(response.change().is_none());
}

#[tokio::test]
async fn federated_mint_connector_returns_threshold_pending_without_slow_surplus_member() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    let slow_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&slow_member, &bundle, member_id(1), advertised.clone());
    slow_member.set_post_melt_delay(Duration::from_secs(30));
    slow_member.set_post_melt_response(Ok(MeltQuoteResponse::Bolt11(
        melt_response_without_change_with_state(MeltQuoteState::Pending),
    )));
    connectors.insert(
        member_id(1),
        Arc::new(slow_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for member_id in [member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.set_post_melt_response(Ok(MeltQuoteResponse::Bolt11(
            melt_response_without_change_with_state(MeltQuoteState::Pending),
        )));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let request = (0..128)
        .find_map(|attempt| {
            let request = MeltRequest::new(format!("melt-quote-{attempt}"), Vec::new(), None);
            let envelope = cdk_common::federation::FederationOperationEnvelope::new(
                bundle.public_config.federation_id,
                cdk_common::federation::FederationOperation::Melt(
                    cdk_common::federation::FederationMeltOperation {
                        request: request.clone(),
                    },
                ),
            );
            let operation_id = envelope.operation_id().expect("melt operation id");
            let selector = u64::from_be_bytes(
                operation_id.to_bytes()[..8]
                    .try_into()
                    .expect("operation selector bytes"),
            );
            (selector % 3 == 0).then_some(request)
        })
        .expect("request whose old roster selector points at the slow member");
    let connector = FederatedMintConnector::new(bundle.public_config, connectors)
        .expect("federated mint connector");
    let response = tokio::time::timeout(
        Duration::from_secs(1),
        connector.post_melt(&PaymentMethod::Known(KnownMethod::Bolt11), request),
    )
    .await
    .expect("threshold pending response must not wait for the slow surplus member")
    .expect("melt response");

    assert_eq!(response.state(), MeltQuoteState::Pending);
}

#[tokio::test]
async fn federated_mint_connector_uses_threshold_mint_quote_status_ignoring_updated_at() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let status = mint_quote_status_response(MintQuoteState::Paid);
    let mut connectors = BTreeMap::new();

    let failing_member = MockMintConnector::new();
    failing_member.set_active_keyset(advertised.clone());
    failing_member.set_mint_quote_status_response(Err(crate::Error::Custom(
        "first member unavailable".to_string(),
    )));
    connectors.insert(
        member_id(1),
        Arc::new(failing_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for (member_id, updated_at) in [(member_id(2), 10), (member_id(3), 20)] {
        let connector = MockMintConnector::new();
        connector.set_active_keyset(advertised.clone());
        let mut member_status = status.clone();
        member_status.updated_at = updated_at;
        connector.set_mint_quote_status_response(Ok(member_status));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = connector
        .get_mint_quote_status(PaymentMethod::Known(KnownMethod::Bolt11), "mint-quote")
        .await
        .expect("mint quote status");

    assert_eq!(response.state(), Some(MintQuoteState::Paid));
}

#[tokio::test]
async fn federated_mint_connector_waits_for_matching_mint_quote_status_quorum() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    let stale_member = MockMintConnector::new();
    stale_member.set_active_keyset(advertised.clone());
    stale_member
        .set_mint_quote_status_response(Ok(mint_quote_status_response(MintQuoteState::Unpaid)));
    connectors.insert(
        member_id(1),
        Arc::new(stale_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for member_id in [member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        connector.set_active_keyset(advertised.clone());
        connector
            .set_mint_quote_status_response(Ok(mint_quote_status_response(MintQuoteState::Paid)));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = connector
        .get_mint_quote_status(PaymentMethod::Known(KnownMethod::Bolt11), "mint-quote")
        .await
        .expect("mint quote status");

    assert_eq!(response.state(), Some(MintQuoteState::Paid));
}

#[tokio::test]
async fn federated_mint_connector_rejects_single_member_mint_quote_status() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    let lone_member = MockMintConnector::new();
    lone_member.set_active_keyset(advertised.clone());
    lone_member
        .set_mint_quote_status_response(Ok(mint_quote_status_response(MintQuoteState::Paid)));
    connectors.insert(
        member_id(1),
        Arc::new(lone_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for member_id in [member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        connector.set_active_keyset(advertised.clone());
        connector.set_mint_quote_status_response(Err(crate::Error::UnknownQuote));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let error = connector
        .get_mint_quote_status(PaymentMethod::Known(KnownMethod::Bolt11), "mint-quote")
        .await
        .expect_err("one member must not decide mint quote status");

    assert!(matches!(error, crate::Error::InvalidMintResponse(_)));
}

#[tokio::test(start_paused = true)]
async fn federated_mint_connector_creates_mint_quote_with_first_healthy_member() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let expected = bolt11_quote_response(&QuoteId::new(), 3);
    let fallback_timeout = Duration::from_secs(30);

    let unavailable_member = Arc::new(MockMintConnector::new());
    unavailable_member.set_active_keyset(advertised.clone());
    unavailable_member.set_post_mint_quote_response(Err(crate::Error::Custom(
        "quote coordinator unavailable".to_string(),
    )));
    set_mint_quote_status_from_response(&unavailable_member, &expected);
    unavailable_member.push_mint_quote_status_response(Err(crate::Error::UnknownQuote));

    let healthy_member = Arc::new(MockMintConnector::new());
    healthy_member.set_active_keyset(advertised.clone());
    healthy_member.set_post_mint_quote_response(Ok(expected.clone()));
    set_mint_quote_status_from_response(&healthy_member, &expected);
    healthy_member.push_mint_quote_status_response(Err(crate::Error::UnknownQuote));

    let unused_member = Arc::new(MockMintConnector::new());
    unused_member.set_active_keyset(advertised);
    set_mint_quote_status_from_response(&unused_member, &expected);
    unused_member.push_mint_quote_status_response(Err(crate::Error::UnknownQuote));

    let connectors = BTreeMap::from([
        (
            member_id(1),
            unavailable_member.clone() as Arc<dyn MintConnector + Send + Sync>,
        ),
        (
            member_id(2),
            healthy_member.clone() as Arc<dyn MintConnector + Send + Sync>,
        ),
        (
            member_id(3),
            unused_member as Arc<dyn MintConnector + Send + Sync>,
        ),
    ]);
    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            member_fallback_timeout: fallback_timeout,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let started = tokio::time::Instant::now();
    let response = connector
        .post_mint_quote(bolt11_quote_request(3))
        .await
        .expect("mint quote");

    assert!(started.elapsed() < fallback_timeout);
    QuoteId::from_str(response.quote()).expect("valid mint quote id");
    assert_eq!(response.request(), expected.request());
    assert_eq!(response.method(), expected.method());
    assert_eq!(unavailable_member.post_mint_quote_call_count(), 1);
    assert_eq!(healthy_member.post_mint_quote_call_count(), 1);
}

#[tokio::test]
async fn federated_mint_connector_uses_configured_quote_coordinator_for_quote_creation() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let quote_id = QuoteId::new();
    let expected = bolt11_quote_response(&quote_id, 3);
    let mut connectors = BTreeMap::new();

    let default_coordinator = MockMintConnector::new();
    default_coordinator.set_active_keyset(advertised.clone());
    default_coordinator.set_post_mint_quote_response(Err(crate::Error::InvalidPaymentMethod));
    set_mint_quote_status_from_response(&default_coordinator, &expected);
    connectors.insert(
        member_id(1),
        Arc::new(default_coordinator) as Arc<dyn MintConnector + Send + Sync>,
    );

    let preferred_member = MockMintConnector::new();
    preferred_member.set_active_keyset(advertised.clone());
    preferred_member.set_post_mint_quote_response(Ok(expected.clone()));
    set_mint_quote_status_from_response(&preferred_member, &expected);
    connectors.insert(
        member_id(2),
        Arc::new(preferred_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let third_member = MockMintConnector::new();
    third_member.set_active_keyset(advertised);
    set_mint_quote_status_from_response(&third_member, &expected);
    connectors.insert(
        member_id(3),
        Arc::new(third_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            quote_coordinator_member_id: Some(member_id(2)),
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let response = connector
        .post_mint_quote(bolt11_quote_request(3))
        .await
        .expect("mint quote");

    QuoteId::from_str(response.quote()).expect("valid assigned mint quote id");
    assert_eq!(connector.quote_coordinator_member_id, member_id(2));
}

#[tokio::test]
async fn federated_mint_connector_requires_threshold_quote_visibility_after_creation() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let quote_id = QuoteId::new();
    let expected = bolt11_quote_response(&quote_id, 3);
    let mut connectors = BTreeMap::new();

    let quote_coordinator = MockMintConnector::new();
    quote_coordinator.set_active_keyset(advertised.clone());
    quote_coordinator.set_post_mint_quote_response(Ok(expected.clone()));
    set_mint_quote_status_from_response(&quote_coordinator, &expected);
    connectors.insert(
        member_id(1),
        Arc::new(quote_coordinator) as Arc<dyn MintConnector + Send + Sync>,
    );

    for member_id in [member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        connector.set_active_keyset(advertised.clone());
        connector.set_mint_quote_status_response(Err(crate::Error::UnknownQuote));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            mint_quote_visibility_attempts: 1,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let err = connector
        .post_mint_quote(bolt11_quote_request(3))
        .await
        .expect_err("quote should not be threshold visible");

    assert!(
        err.to_string()
            .contains("insufficient federation mint quote status responses"),
        "{err}"
    );
}

#[tokio::test]
async fn federated_mint_connector_retries_mint_quote_visibility_after_creation() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let quote_id = QuoteId::new();
    let expected = bolt11_quote_response(&quote_id, 3);
    let mut connectors = BTreeMap::new();

    let quote_coordinator = MockMintConnector::new();
    quote_coordinator.set_active_keyset(advertised.clone());
    quote_coordinator.set_post_mint_quote_response(Ok(expected.clone()));
    push_mint_quote_status_from_response(&quote_coordinator, &expected);
    push_mint_quote_status_from_response(&quote_coordinator, &expected);
    connectors.insert(
        member_id(1),
        Arc::new(quote_coordinator) as Arc<dyn MintConnector + Send + Sync>,
    );

    let catching_up_member = MockMintConnector::new();
    catching_up_member.set_active_keyset(advertised.clone());
    catching_up_member.push_mint_quote_status_response(Err(crate::Error::UnknownQuote));
    push_mint_quote_status_from_response(&catching_up_member, &expected);
    connectors.insert(
        member_id(2),
        Arc::new(catching_up_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let unavailable_member = MockMintConnector::new();
    unavailable_member.set_active_keyset(advertised);
    unavailable_member.push_mint_quote_status_response(Err(crate::Error::UnknownQuote));
    unavailable_member.push_mint_quote_status_response(Err(crate::Error::UnknownQuote));
    connectors.insert(
        member_id(3),
        Arc::new(unavailable_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            mint_quote_visibility_attempts: 2,
            mint_quote_visibility_retry_delay: Duration::from_millis(1),
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let response = connector
        .post_mint_quote(bolt11_quote_request(3))
        .await
        .expect("mint quote should become threshold visible");

    QuoteId::from_str(response.quote()).expect("valid assigned mint quote id");
    assert_eq!(response.request(), expected.request());
}

#[tokio::test]
async fn federated_mint_connector_returns_after_threshold_mint_responses() {
    let bundle = threshold_four_bundle();
    let keyset = active_keyset(&bundle);
    let outputs = vec![blinded_output_for_keyset_amount(
        &keyset,
        1,
        b"threshold mint liveness",
    )];
    let advertised = advertised_keyset(&bundle);
    let request_timeout = Duration::from_secs(30);
    let mut connectors = BTreeMap::new();

    let slow_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&slow_member, &bundle, member_id(1), advertised.clone());
    slow_member.set_post_mint_delay(Duration::from_secs(30));
    slow_member.set_post_mint_response(Ok(share_response(&bundle, member_id(1), &outputs)));
    connectors.insert(
        member_id(1),
        Arc::new(slow_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for member_id in [member_id(2), member_id(3), member_id(4)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.set_post_mint_response(Ok(share_response(&bundle, member_id, &outputs)));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            member_request_timeout: request_timeout,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let request = MintRequest {
        quote: "paid-quote".to_string(),
        outputs: outputs.clone(),
        signature: None,
    };
    let mint = tokio::spawn(async move {
        connector
            .post_mint(&PaymentMethod::Known(KnownMethod::Bolt11), request)
            .await
    });

    let response = tokio::time::timeout(Duration::from_millis(250), mint)
        .await
        .expect("mint should not wait for slow non-threshold member")
        .expect("mint task")
        .expect("mint response");
    assert_aggregate_response_valid(&bundle, &outputs, &response);
}

#[tokio::test]
async fn federated_mint_connector_retries_retryable_member_request_failure() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    let flaky_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&flaky_member, &bundle, member_id(1), advertised.clone());
    flaky_member.push_post_mint_response(Err(crate::Error::Custom(
        "temporary member transport failure".to_string(),
    )));
    flaky_member.push_post_mint_response(Ok(share_response(&bundle, member_id(1), &outputs)));
    connectors.insert(
        member_id(1),
        Arc::new(flaky_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let healthy_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(
        &healthy_member,
        &bundle,
        member_id(2),
        advertised.clone(),
    );
    healthy_member.set_post_mint_response(Ok(share_response(&bundle, member_id(2), &outputs)));
    connectors.insert(
        member_id(2),
        Arc::new(healthy_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let rejecting_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&rejecting_member, &bundle, member_id(3), advertised);
    rejecting_member.set_post_mint_response(Err(crate::Error::InvalidPaymentMethod));
    connectors.insert(
        member_id(3),
        Arc::new(rejecting_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            member_request_max_retries: 1,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let response = connector
        .post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: "paid-quote".to_string(),
                outputs: outputs.clone(),
                signature: None,
            },
        )
        .await
        .expect("mint response");

    assert_aggregate_response_valid(&bundle, &outputs, &response);
}

#[tokio::test]
async fn federated_mint_connector_fans_signing_request_out_to_every_member() {
    let bundle = threshold_five_bundle();
    let outputs = blinded_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();
    let mut observed_connectors = Vec::new();

    for id in [
        member_id(1),
        member_id(2),
        member_id(3),
        member_id(4),
        member_id(5),
    ] {
        let connector = Arc::new(MockMintConnector::new());
        set_member_advertisement_with_keyset(&connector, &bundle, id, advertised.clone());
        if id == member_id(1) {
            connector.set_post_swap_response(Err(crate::Error::Timeout));
        } else {
            connector.set_post_swap_response(Ok(SwapResponse {
                signatures: share_response(&bundle, id, &outputs).signatures,
            }));
        }
        observed_connectors.push(connector.clone());
        connectors.insert(id, connector as Arc<dyn MintConnector + Send + Sync>);
    }

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            member_request_max_retries: 0,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let request = SwapRequest::new(Vec::new(), outputs.clone());
    let response = connector
        .post_swap(request.clone())
        .await
        .expect("three compatible shares from the full roster");

    assert_aggregate_response_valid(
        &bundle,
        &outputs,
        &MintResponse {
            signatures: response.signatures,
        },
    );
    for member in observed_connectors {
        assert_eq!(
            member.captured_swap_requests(),
            vec![request.clone()],
            "every configured federation member must receive the signing request"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn federated_mint_connector_waits_for_catching_up_member_with_identical_request() {
    let bundle = threshold_four_bundle();
    let outputs = vec![blinded_output_for_keyset_amount(
        &active_keyset(&bundle),
        1,
        b"catch-up retry output",
    )];
    let advertised = advertised_keyset(&bundle);
    let request = SwapRequest::new(Vec::new(), outputs.clone());
    let mut connectors = BTreeMap::new();

    for id in [member_id(1), member_id(2)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, id, advertised.clone());
        connector.set_post_swap_response(Ok(SwapResponse {
            signatures: share_response(&bundle, id, &outputs).signatures,
        }));
        connectors.insert(
            id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let catching_up_member = Arc::new(MockMintConnector::new());
    set_member_advertisement_with_keyset(
        &catching_up_member,
        &bundle,
        member_id(3),
        advertised.clone(),
    );
    catching_up_member.push_post_swap_response(Err(federation_catch_up_error(member_id(3))));
    catching_up_member.push_post_swap_response(Err(federation_catch_up_error(member_id(3))));
    catching_up_member.push_post_swap_response(Ok(SwapResponse {
        signatures: share_response(&bundle, member_id(3), &outputs).signatures,
    }));
    connectors.insert(
        member_id(3),
        catching_up_member.clone() as Arc<dyn MintConnector + Send + Sync>,
    );

    let rejecting_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&rejecting_member, &bundle, member_id(4), advertised);
    rejecting_member.set_post_swap_response(Err(crate::Error::InvalidPaymentMethod));
    connectors.insert(
        member_id(4),
        Arc::new(rejecting_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = connector
        .post_swap(request.clone())
        .await
        .expect("catching-up member should become available before the retry deadline");

    assert_aggregate_response_valid(
        &bundle,
        &outputs,
        &MintResponse {
            signatures: response.signatures,
        },
    );
    assert_eq!(
        catching_up_member.captured_swap_requests(),
        vec![request.clone(), request.clone(), request],
        "catch-up retries must reuse the exact deterministic request"
    );
}

#[tokio::test(start_paused = true)]
async fn federated_mint_connector_returns_typed_ambiguous_error_after_catch_up_deadline() {
    let bundle = threshold_five_bundle();
    let outputs = blinded_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let request = SwapRequest::new(Vec::new(), outputs.clone());
    let mut connectors = BTreeMap::new();

    for id in [member_id(1), member_id(2)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, id, advertised.clone());
        connector.set_post_swap_response(Ok(SwapResponse {
            signatures: share_response(&bundle, id, &outputs).signatures,
        }));
        connectors.insert(
            id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let mut catching_up_members = Vec::new();
    for id in [member_id(3), member_id(4), member_id(5)] {
        let connector = Arc::new(MockMintConnector::new());
        set_member_advertisement_with_keyset(&connector, &bundle, id, advertised.clone());
        for _ in 0..=FEDERATION_CATCH_UP_MAX_RETRIES {
            connector.push_post_swap_response(Err(federation_catch_up_error(id)));
        }
        catching_up_members.push(connector.clone());
        connectors.insert(id, connector as Arc<dyn MintConnector + Send + Sync>);
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let error = connector
        .post_swap(request.clone())
        .await
        .expect_err("two shares cannot satisfy a three-share threshold");

    assert!(matches!(error, crate::Error::FederationCatchUp { .. }));
    assert!(!error.is_definitive_failure());
    for member in catching_up_members {
        let requests = member.captured_swap_requests();
        assert_eq!(
            requests.len(),
            usize::from(FEDERATION_CATCH_UP_MAX_RETRIES) + 1
        );
        assert!(requests.iter().all(|candidate| candidate == &request));
    }
}

#[tokio::test]
async fn federated_mint_connector_retries_rate_limited_member_request() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    let rate_limited_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(
        &rate_limited_member,
        &bundle,
        member_id(1),
        advertised.clone(),
    );
    rate_limited_member.push_post_mint_response(Err(crate::Error::HttpError(
        Some(429),
        "too many requests".to_string(),
    )));
    rate_limited_member.push_post_mint_response(Ok(share_response(
        &bundle,
        member_id(1),
        &outputs,
    )));
    connectors.insert(
        member_id(1),
        Arc::new(rate_limited_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let healthy_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(
        &healthy_member,
        &bundle,
        member_id(2),
        advertised.clone(),
    );
    healthy_member.set_post_mint_response(Ok(share_response(&bundle, member_id(2), &outputs)));
    connectors.insert(
        member_id(2),
        Arc::new(healthy_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let rejecting_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&rejecting_member, &bundle, member_id(3), advertised);
    rejecting_member.set_post_mint_response(Err(crate::Error::InvalidPaymentMethod));
    connectors.insert(
        member_id(3),
        Arc::new(rejecting_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            member_request_max_retries: 1,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let response = connector
        .post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: "paid-quote".to_string(),
                outputs: outputs.clone(),
                signature: None,
            },
        )
        .await
        .expect("mint response");

    assert_aggregate_response_valid(&bundle, &outputs, &response);
}

#[tokio::test]
async fn federated_mint_connector_tolerates_mixed_final_pending_and_rejected_value_responses() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let change_outputs = blinded_change_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    let final_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&final_member, &bundle, member_id(1), advertised.clone());
    final_member.set_post_mint_response(Ok(share_response(&bundle, member_id(1), &outputs)));
    final_member.set_post_swap_response(Ok(SwapResponse {
        signatures: share_response(&bundle, member_id(1), &outputs).signatures,
    }));
    final_member.set_post_melt_response(Ok(melt_response(&bundle, member_id(1), &change_outputs)));
    connectors.insert(
        member_id(1),
        Arc::new(final_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let pending_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(
        &pending_member,
        &bundle,
        member_id(2),
        advertised.clone(),
    );
    pending_member.push_post_mint_response(Err(pending_finalization_error(1)));
    pending_member.push_post_mint_response(Ok(share_response(&bundle, member_id(2), &outputs)));
    pending_member.push_post_swap_response(Err(pending_finalization_error(2)));
    pending_member.push_post_swap_response(Ok(SwapResponse {
        signatures: share_response(&bundle, member_id(2), &outputs).signatures,
    }));
    pending_member.push_post_melt_response(Err(pending_finalization_error(3)));
    pending_member.push_post_melt_response(Ok(melt_response(
        &bundle,
        member_id(2),
        &change_outputs,
    )));
    connectors.insert(
        member_id(2),
        Arc::new(pending_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let rejecting_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&rejecting_member, &bundle, member_id(3), advertised);
    rejecting_member.set_post_mint_response(Err(crate::Error::InvalidPaymentMethod));
    rejecting_member.set_post_swap_response(Err(crate::Error::InvalidPaymentMethod));
    rejecting_member.set_post_melt_response(Err(crate::Error::InvalidPaymentMethod));
    connectors.insert(
        member_id(3),
        Arc::new(rejecting_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            member_request_max_retries: 1,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");

    let mint_response = connector
        .post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: "paid-quote".to_string(),
                outputs: outputs.clone(),
                signature: None,
            },
        )
        .await
        .expect("mint response");
    assert_aggregate_response_valid(&bundle, &outputs, &mint_response);

    let swap_response = connector
        .post_swap(SwapRequest::new(Vec::new(), outputs.clone()))
        .await
        .expect("swap response");
    assert_aggregate_response_valid(
        &bundle,
        &outputs,
        &MintResponse {
            signatures: swap_response.signatures,
        },
    );

    let melt_response = connector
        .post_melt(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MeltRequest::new(
                "melt-quote".to_string(),
                Vec::new(),
                Some(change_outputs.clone()),
            ),
        )
        .await
        .expect("melt response");
    let change = melt_response.change().cloned().expect("change signatures");
    assert_eq!(melt_response.state(), MeltQuoteState::Paid);
    assert_aggregate_response_valid(
        &bundle,
        &signed_change_outputs(&change_outputs),
        &MintResponse { signatures: change },
    );
}

#[tokio::test]
async fn federated_mint_connector_returns_threshold_rejection_without_waiting_for_slow_members() {
    let bundle = threshold_five_bundle();
    let outputs = blinded_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();
    let rejection = cdk_common::error::ErrorResponse::from(crate::Error::TokenAlreadySpent);

    for member_id in [member_id(1), member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.set_post_swap_response(Err(crate::Error::MintErrorResponse(rejection.clone())));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let duplicate_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(
        &duplicate_member,
        &bundle,
        member_id(4),
        advertised.clone(),
    );
    duplicate_member.set_post_swap_delay(Duration::from_secs(30));
    duplicate_member.set_post_swap_response(Err(crate::Error::DuplicateInputs));
    connectors.insert(
        member_id(4),
        Arc::new(duplicate_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let unavailable_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&unavailable_member, &bundle, member_id(5), advertised);
    unavailable_member.set_post_swap_delay(Duration::from_secs(30));
    unavailable_member.set_post_swap_response(Err(crate::Error::Timeout));
    connectors.insert(
        member_id(5),
        Arc::new(unavailable_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            member_request_max_retries: 0,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let result = tokio::time::timeout(
        Duration::from_millis(250),
        connector.post_swap(SwapRequest::new(Vec::new(), outputs)),
    )
    .await
    .expect("matching threshold rejection should cancel slow member requests");

    assert!(matches!(result, Err(crate::Error::TokenAlreadySpent)));
}

#[tokio::test]
async fn federated_mint_connector_keeps_disagreeing_rejections_ambiguous() {
    let bundle = threshold_five_bundle();
    let outputs = blinded_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    for (member_id, error) in [
        (member_id(1), crate::Error::TokenAlreadySpent),
        (member_id(2), crate::Error::TokenAlreadySpent),
        (member_id(3), crate::Error::DuplicateInputs),
        (member_id(4), crate::Error::DuplicateInputs),
        (member_id(5), crate::Error::Timeout),
    ] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.set_post_swap_response(Err(error));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            member_request_max_retries: 0,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let result = connector
        .post_swap(SwapRequest::new(Vec::new(), outputs))
        .await;

    assert!(matches!(result, Err(crate::Error::InvalidMintResponse(_))));
}

#[tokio::test]
async fn federated_mint_connector_requires_exact_error_response_agreement() {
    let bundle = threshold_five_bundle();
    let outputs = blinded_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let first_rejection = cdk_common::error::ErrorResponse::new(
        cdk_common::error::ErrorCode::TokenAlreadySpent,
        "spent in operation one".to_string(),
    );
    let second_rejection = cdk_common::error::ErrorResponse::new(
        cdk_common::error::ErrorCode::TokenAlreadySpent,
        "spent in operation two".to_string(),
    );
    let mut connectors = BTreeMap::new();

    for (id, rejection) in [
        (member_id(1), first_rejection.clone()),
        (member_id(2), first_rejection),
        (member_id(3), second_rejection.clone()),
        (member_id(4), second_rejection),
    ] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, id, advertised.clone());
        connector.set_post_swap_response(Err(crate::Error::MintErrorResponse(rejection)));
        connectors.insert(
            id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let unavailable_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&unavailable_member, &bundle, member_id(5), advertised);
    unavailable_member.set_post_swap_response(Err(crate::Error::Timeout));
    connectors.insert(
        member_id(5),
        Arc::new(unavailable_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            member_request_max_retries: 0,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let result = connector
        .post_swap(SwapRequest::new(Vec::new(), outputs))
        .await;

    assert!(matches!(result, Err(crate::Error::InvalidMintResponse(_))));
}

#[tokio::test]
async fn federated_mint_connector_round_trips_mint_and_melt_cashu_rejections() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();
    let slow_member_id = member_id(3);

    for member_id in [member_id(1), member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        if member_id == slow_member_id {
            connector.set_post_mint_delay(Duration::from_secs(30));
            connector.set_post_melt_delay(Duration::from_secs(30));
        }
        connector.set_post_mint_response(Err(crate::Error::IssuedQuote));
        connector.set_post_melt_response(Err(crate::Error::TokenAlreadySpent));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let mint_result = tokio::time::timeout(
        Duration::from_millis(250),
        connector.post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: "issued-quote".to_string(),
                outputs,
                signature: None,
            },
        ),
    )
    .await
    .expect("matching mint rejection should cancel slow member requests");
    assert!(matches!(mint_result, Err(crate::Error::IssuedQuote)));

    let melt_result = tokio::time::timeout(
        Duration::from_millis(250),
        connector.post_melt(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MeltRequest::new("melt-quote".to_string(), Vec::new(), None),
        ),
    )
    .await
    .expect("matching melt rejection should cancel slow member requests");
    assert!(matches!(melt_result, Err(crate::Error::TokenAlreadySpent)));
}

#[tokio::test]
async fn federated_mint_connector_keeps_timeouts_and_invalid_shares_ambiguous() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let mut timeout_connectors = BTreeMap::new();
    let mut invalid_share_connectors = BTreeMap::new();

    for id in [member_id(1), member_id(2), member_id(3)] {
        let timeout_connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&timeout_connector, &bundle, id, advertised.clone());
        timeout_connector.set_post_swap_delay(Duration::from_millis(50));
        timeout_connector.set_post_swap_response(Ok(SwapResponse {
            signatures: share_response(&bundle, id, &outputs).signatures,
        }));
        timeout_connectors.insert(
            id,
            Arc::new(timeout_connector) as Arc<dyn MintConnector + Send + Sync>,
        );

        let invalid_connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&invalid_connector, &bundle, id, advertised.clone());
        let wrong_signer = if id == member_id(3) {
            member_id(1)
        } else {
            member_id(3)
        };
        invalid_connector.set_post_swap_response(Ok(SwapResponse {
            signatures: share_response(&bundle, wrong_signer, &outputs).signatures,
        }));
        invalid_share_connectors.insert(
            id,
            Arc::new(invalid_connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let timeout_connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        timeout_connectors,
        FederatedMintConnectorConfig {
            member_request_timeout: Duration::from_millis(1),
            member_request_max_retries: 0,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("timeout connector");
    let timeout_result = timeout_connector
        .post_swap(SwapRequest::new(Vec::new(), outputs.clone()))
        .await;
    assert!(matches!(
        timeout_result,
        Err(crate::Error::InvalidMintResponse(_))
    ));

    let invalid_share_connector =
        FederatedMintConnector::new(bundle.public_config.clone(), invalid_share_connectors)
            .expect("invalid-share connector");
    let invalid_share_result = invalid_share_connector
        .post_swap(SwapRequest::new(Vec::new(), outputs))
        .await;
    assert!(matches!(
        invalid_share_result,
        Err(crate::Error::InvalidMintResponse(_))
    ));
}

#[tokio::test]
async fn federated_mint_connector_tolerates_slow_offline_and_bad_share_members() {
    let bundle = threshold_five_bundle();
    let outputs = blinded_outputs(&bundle);
    let change_outputs = blinded_change_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let slow_delay = Duration::from_millis(10);
    let mut connectors = BTreeMap::new();

    for member_id in [member_id(1), member_id(2)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.set_post_mint_response(Ok(share_response(&bundle, member_id, &outputs)));
        connector.set_post_swap_response(Ok(SwapResponse {
            signatures: share_response(&bundle, member_id, &outputs).signatures,
        }));
        connector.set_post_melt_response(Ok(melt_response(&bundle, member_id, &change_outputs)));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let offline_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(
        &offline_member,
        &bundle,
        member_id(3),
        advertised.clone(),
    );
    offline_member.set_post_mint_response(Err(crate::Error::Custom("member offline".to_string())));
    offline_member.set_post_swap_response(Err(crate::Error::Custom("member offline".to_string())));
    offline_member.set_post_melt_response(Err(crate::Error::Custom("member offline".to_string())));
    connectors.insert(
        member_id(3),
        Arc::new(offline_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let bad_share_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(
        &bad_share_member,
        &bundle,
        member_id(4),
        advertised.clone(),
    );
    bad_share_member.set_post_mint_response(Ok(share_response(&bundle, member_id(5), &outputs)));
    bad_share_member.set_post_swap_response(Ok(SwapResponse {
        signatures: share_response(&bundle, member_id(5), &outputs).signatures,
    }));
    bad_share_member.set_post_melt_response(Ok(melt_response(
        &bundle,
        member_id(5),
        &change_outputs,
    )));
    connectors.insert(
        member_id(4),
        Arc::new(bad_share_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let slow_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&slow_member, &bundle, member_id(5), advertised);
    slow_member.set_post_mint_delay(slow_delay);
    slow_member.set_post_swap_delay(slow_delay);
    slow_member.set_post_melt_delay(slow_delay);
    slow_member.set_post_mint_response(Ok(share_response(&bundle, member_id(5), &outputs)));
    slow_member.set_post_swap_response(Ok(SwapResponse {
        signatures: share_response(&bundle, member_id(5), &outputs).signatures,
    }));
    slow_member.set_post_melt_response(Ok(melt_response(&bundle, member_id(5), &change_outputs)));
    connectors.insert(
        member_id(5),
        Arc::new(slow_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            member_request_timeout: Duration::from_secs(1),
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");

    let mint_response = connector
        .post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: "paid-quote".to_string(),
                outputs: outputs.clone(),
                signature: None,
            },
        )
        .await
        .expect("mint response");
    assert_aggregate_response_valid(&bundle, &outputs, &mint_response);

    let swap_response = connector
        .post_swap(SwapRequest::new(Vec::new(), outputs.clone()))
        .await
        .expect("swap response");
    assert_aggregate_response_valid(
        &bundle,
        &outputs,
        &MintResponse {
            signatures: swap_response.signatures,
        },
    );

    let melt_response = connector
        .post_melt(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MeltRequest::new(
                "melt-quote".to_string(),
                Vec::new(),
                Some(change_outputs.clone()),
            ),
        )
        .await
        .expect("melt response");
    let change = melt_response.change().cloned().expect("change signatures");
    assert_eq!(melt_response.state(), MeltQuoteState::Paid);
    assert_aggregate_response_valid(
        &bundle,
        &signed_change_outputs(&change_outputs),
        &MintResponse { signatures: change },
    );
}

#[test]
fn federated_share_batch_rejects_metadata_before_parsing_signature_points() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        member_connectors(&bundle, &outputs),
        FederatedMintConnectorConfig::default(),
    )
    .expect("federated mint connector");
    let mut response = share_response(&bundle, member_id(1), &outputs);
    response.signatures[0].amount = Amount::from(2);
    response.signatures[0].c = identity_secret(member_id(1)).public_key();

    let error = connector
        .valid_member_share_set(
            &outputs,
            member_id(1),
            &response,
            super::ShareAmountValidation::Exact,
        )
        .expect_err("mismatched signature metadata");

    assert!(error.to_string().contains("signature amount"));
}

#[tokio::test]
async fn federated_mint_connector_uses_threshold_live_members_for_wallet_operations() {
    let bundle = threshold_five_bundle();
    let outputs = blinded_outputs(&bundle);
    let change_outputs = blinded_change_outputs(&bundle);
    let restore_outputs = blinded_change_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let check_ys = vec![
        PublicKey::from(BlsG1PublicKey::hash_to_curve(b"threshold-live-check-y-1")),
        PublicKey::from(BlsG1PublicKey::hash_to_curve(b"threshold-live-check-y-2")),
    ];
    let check_response = proof_state_response(&check_ys, &[State::Spent, State::Pending]);
    let mut connectors = BTreeMap::new();

    for member_id in [member_id(1), member_id(2), member_id(3), member_id(5)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.set_post_mint_response(Ok(share_response(&bundle, member_id, &outputs)));
        connector.set_post_swap_response(Ok(SwapResponse {
            signatures: share_response(&bundle, member_id, &outputs).signatures,
        }));
        connector.set_post_melt_response(Ok(melt_response(&bundle, member_id, &change_outputs)));
        connector._set_restore_response(Ok(restore_response(&bundle, member_id, &restore_outputs)));
        connector.set_check_state_response(Ok(check_response.clone()));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let offline_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&offline_member, &bundle, member_id(4), advertised);
    offline_member.set_mint_keys_error("member offline");
    offline_member.set_post_mint_response(Err(crate::Error::Custom("member offline".to_string())));
    offline_member.set_post_swap_response(Err(crate::Error::Custom("member offline".to_string())));
    offline_member.set_post_melt_response(Err(crate::Error::Custom("member offline".to_string())));
    offline_member._set_restore_response(Err(crate::Error::Custom("member offline".to_string())));
    offline_member
        .set_check_state_response(Err(crate::Error::Custom("member offline".to_string())));
    connectors.insert(
        member_id(4),
        Arc::new(offline_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");

    let mint_response = connector
        .post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: "paid-quote".to_string(),
                outputs: outputs.clone(),
                signature: None,
            },
        )
        .await
        .expect("mint response");
    assert_aggregate_response_valid(&bundle, &outputs, &mint_response);

    let swap_response = connector
        .post_swap(SwapRequest::new(Vec::new(), outputs.clone()))
        .await
        .expect("swap response");
    assert_aggregate_response_valid(
        &bundle,
        &outputs,
        &MintResponse {
            signatures: swap_response.signatures,
        },
    );

    let melt_response = connector
        .post_melt(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MeltRequest::new(
                "melt-quote".to_string(),
                Vec::new(),
                Some(change_outputs.clone()),
            ),
        )
        .await
        .expect("melt response");
    let change = melt_response.change().cloned().expect("change signatures");
    assert_eq!(melt_response.state(), MeltQuoteState::Paid);
    assert_aggregate_response_valid(
        &bundle,
        &signed_change_outputs(&change_outputs),
        &MintResponse { signatures: change },
    );

    let restore_response = connector
        .post_restore(RestoreRequest {
            outputs: restore_outputs.clone(),
        })
        .await
        .expect("restore response");
    assert_eq!(restore_response.outputs, restore_outputs);
    assert_aggregate_response_valid(
        &bundle,
        &signed_change_outputs(&restore_outputs),
        &MintResponse {
            signatures: restore_response.signatures,
        },
    );

    let state_response = connector
        .post_check_state(CheckStateRequest { ys: check_ys })
        .await
        .expect("check-state response");
    assert_eq!(state_response, check_response);
}

#[tokio::test(start_paused = true)]
async fn federated_mint_quote_timeout_falls_back_to_next_member() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let stalled_quote = bolt11_quote_response(&QuoteId::new(), 3);
    let expected = bolt11_quote_response(&QuoteId::new(), 3);
    let fallback_timeout = Duration::from_secs(2);
    let mut connectors = BTreeMap::new();

    let stalled_member = Arc::new(MockMintConnector::new());
    stalled_member.set_active_keyset(advertised.clone());
    stalled_member.set_post_mint_quote_delay(fallback_timeout + Duration::from_secs(60));
    stalled_member.set_post_mint_quote_response(Ok(stalled_quote));
    set_mint_quote_status_from_response(&stalled_member, &expected);
    connectors.insert(
        member_id(1),
        stalled_member.clone() as Arc<dyn MintConnector + Send + Sync>,
    );

    let healthy_member = Arc::new(MockMintConnector::new());
    healthy_member.set_active_keyset(advertised.clone());
    healthy_member.set_post_mint_quote_response(Ok(expected.clone()));
    set_mint_quote_status_from_response(&healthy_member, &expected);
    connectors.insert(
        member_id(2),
        healthy_member.clone() as Arc<dyn MintConnector + Send + Sync>,
    );

    let unused_member = MockMintConnector::new();
    unused_member.set_active_keyset(advertised);
    set_mint_quote_status_from_response(&unused_member, &expected);
    connectors.insert(
        member_id(3),
        Arc::new(unused_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            member_fallback_timeout: fallback_timeout,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let mint_quote =
        tokio::spawn(async move { connector.post_mint_quote(bolt11_quote_request(3)).await });

    tokio::task::yield_now().await;
    tokio::time::advance(fallback_timeout).await;

    let response = mint_quote
        .await
        .expect("mint quote task")
        .expect("mint quote");

    QuoteId::from_str(response.quote()).expect("valid mint quote id");
    assert_eq!(response.request(), expected.request());
    assert_eq!(response.method(), expected.method());
    assert_eq!(stalled_member.post_mint_quote_call_count(), 1);
    assert_eq!(healthy_member.post_mint_quote_call_count(), 1);
}

#[tokio::test(start_paused = true)]
async fn federated_mint_quote_timeout_falls_back_when_quote_is_not_visible() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let expected = bolt11_quote_response(&QuoteId::new(), 3);
    let fallback_timeout = Duration::from_secs(2);

    let stalled_member = Arc::new(MockMintConnector::new());
    stalled_member.set_active_keyset(advertised.clone());
    stalled_member.set_post_mint_quote_delay(fallback_timeout + Duration::from_secs(60));
    stalled_member.set_post_mint_quote_response(Ok(expected.clone()));

    let healthy_member = Arc::new(MockMintConnector::new());
    healthy_member.set_active_keyset(advertised.clone());
    healthy_member.set_post_mint_quote_response(Ok(expected.clone()));

    let third = Arc::new(MockMintConnector::new());
    third.set_active_keyset(advertised);

    for connector in [&stalled_member, &healthy_member, &third] {
        connector.push_mint_quote_status_response(Err(crate::Error::UnknownQuote));
        push_mint_quote_status_from_response(connector, &expected);
    }

    let connectors = BTreeMap::from([
        (
            member_id(1),
            stalled_member.clone() as Arc<dyn MintConnector + Send + Sync>,
        ),
        (
            member_id(2),
            healthy_member.clone() as Arc<dyn MintConnector + Send + Sync>,
        ),
        (member_id(3), third as Arc<dyn MintConnector + Send + Sync>),
    ]);
    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            member_fallback_timeout: fallback_timeout,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let mint_quote =
        tokio::spawn(async move { connector.post_mint_quote(bolt11_quote_request(3)).await });

    tokio::task::yield_now().await;
    tokio::time::advance(fallback_timeout).await;

    let response = mint_quote
        .await
        .expect("mint quote task")
        .expect("fallback mint quote");

    QuoteId::from_str(response.quote()).expect("valid mint quote id");
    assert_eq!(stalled_member.post_mint_quote_call_count(), 1);
    assert_eq!(healthy_member.post_mint_quote_call_count(), 1);
}

#[tokio::test(start_paused = true)]
async fn federated_mint_connector_applies_configured_quote_status_timeout() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let status_timeout = Duration::from_secs(2);
    let mut connectors = BTreeMap::new();

    let slow_member = MockMintConnector::new();
    slow_member.set_active_keyset(advertised.clone());
    slow_member.set_mint_quote_status_delay(status_timeout + Duration::from_secs(60));
    slow_member
        .set_mint_quote_status_response(Ok(mint_quote_status_response(MintQuoteState::Paid)));
    connectors.insert(
        member_id(1),
        Arc::new(slow_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for id in [member_id(2), member_id(3)] {
        let healthy_member = MockMintConnector::new();
        healthy_member.set_active_keyset(advertised.clone());
        healthy_member
            .set_mint_quote_status_response(Ok(mint_quote_status_response(MintQuoteState::Paid)));
        connectors.insert(
            id,
            Arc::new(healthy_member) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            quote_status_member_timeout: status_timeout,
            fanout_concurrency: 1,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let status = tokio::spawn(async move {
        connector
            .get_mint_quote_status(PaymentMethod::Known(KnownMethod::Bolt11), "mint-quote")
            .await
    });

    tokio::task::yield_now().await;
    tokio::time::advance(status_timeout).await;

    let response = status
        .await
        .expect("mint quote status task")
        .expect("mint quote status");
    assert_eq!(response.state(), Some(MintQuoteState::Paid));
}

#[tokio::test]
async fn federated_mint_connector_honors_configured_fanout_concurrency() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let active_calls = Arc::new(AtomicUsize::new(0));
    let max_active_calls = Arc::new(AtomicUsize::new(0));
    let total_calls = Arc::new(AtomicUsize::new(0));
    let mut connectors = BTreeMap::new();

    for id in [member_id(1), member_id(2), member_id(3)] {
        connectors.insert(
            id,
            Arc::new(PostMintConcurrencyProbeConnector::new(
                advertised.clone(),
                federation_mint_info(&bundle, id),
                share_response(&bundle, id, &outputs),
                Duration::from_millis(20),
                active_calls.clone(),
                max_active_calls.clone(),
                total_calls.clone(),
            )) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            fanout_concurrency: 1,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let response = connector
        .post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: "paid-quote".to_string(),
                outputs: outputs.clone(),
                signature: None,
            },
        )
        .await
        .expect("mint response");

    assert_aggregate_response_valid(&bundle, &outputs, &response);
    assert_eq!(max_active_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        total_calls.load(Ordering::SeqCst),
        bundle.public_config.members.len()
    );
    assert_eq!(active_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn federated_mint_connector_creates_melt_quote_with_first_healthy_member() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let expected = melt_quote_create_response();
    let status = melt_quote_status_from_create_response(&expected);
    let fallback_timeout = Duration::from_secs(30);

    let unavailable_member = Arc::new(MockMintConnector::new());
    unavailable_member.set_active_keyset(advertised.clone());
    unavailable_member.set_post_melt_quote_response(Err(crate::Error::Custom(
        "quote coordinator unavailable".to_string(),
    )));
    unavailable_member.set_melt_quote_status_response(Ok(status.clone()));
    unavailable_member.push_melt_quote_status_response(Err(crate::Error::UnknownQuote));

    let healthy_member = Arc::new(MockMintConnector::new());
    healthy_member.set_active_keyset(advertised.clone());
    healthy_member.set_post_melt_quote_response(Ok(expected.clone()));
    healthy_member.set_melt_quote_status_response(Ok(status.clone()));
    healthy_member.push_melt_quote_status_response(Err(crate::Error::UnknownQuote));

    let unused_member = Arc::new(MockMintConnector::new());
    unused_member.set_active_keyset(advertised);
    unused_member.set_melt_quote_status_response(Ok(status));
    unused_member.push_melt_quote_status_response(Err(crate::Error::UnknownQuote));

    let connectors = BTreeMap::from([
        (
            member_id(1),
            unavailable_member.clone() as Arc<dyn MintConnector + Send + Sync>,
        ),
        (
            member_id(2),
            healthy_member.clone() as Arc<dyn MintConnector + Send + Sync>,
        ),
        (
            member_id(3),
            unused_member as Arc<dyn MintConnector + Send + Sync>,
        ),
    ]);
    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            member_fallback_timeout: fallback_timeout,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let started = tokio::time::Instant::now();
    let response = connector
        .post_melt_quote(bolt11_melt_quote_request())
        .await
        .expect("melt quote");

    assert!(started.elapsed() < fallback_timeout);
    assert_melt_quote_matches_expected(&response, &expected);
    assert_eq!(unavailable_member.post_melt_quote_call_count(), 1);
    assert_eq!(healthy_member.post_melt_quote_call_count(), 1);
}

#[tokio::test]
async fn federated_melt_quote_pending_submission_falls_back_to_next_member() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let expected = melt_quote_create_response();
    let status = melt_quote_status_from_create_response(&expected);
    let quote_coordinator = Arc::new(MockMintConnector::new());
    quote_coordinator.set_active_keyset(advertised.clone());
    quote_coordinator.set_post_melt_quote_response(Err(crate::Error::PendingQuote));
    quote_coordinator.set_melt_quote_status_response(Ok(status.clone()));

    let second = Arc::new(MockMintConnector::new());
    second.set_active_keyset(advertised.clone());
    second.set_post_melt_quote_response(Ok(expected.clone()));
    second.set_melt_quote_status_response(Ok(status.clone()));

    let third = Arc::new(MockMintConnector::new());
    third.set_active_keyset(advertised);
    third.set_melt_quote_status_response(Ok(status));

    let connectors = BTreeMap::from([
        (
            member_id(1),
            quote_coordinator.clone() as Arc<dyn MintConnector + Send + Sync>,
        ),
        (
            member_id(2),
            second.clone() as Arc<dyn MintConnector + Send + Sync>,
        ),
        (
            member_id(3),
            third.clone() as Arc<dyn MintConnector + Send + Sync>,
        ),
    ]);
    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            member_fallback_timeout: Duration::from_secs(1),
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");

    let response = connector
        .post_melt_quote(bolt11_melt_quote_request())
        .await
        .expect("threshold-visible pending quote");

    assert_melt_quote_matches_expected(&response, &expected);
    assert_eq!(quote_coordinator.post_melt_quote_call_count(), 1);
    assert_eq!(second.post_melt_quote_call_count(), 1);
    assert_eq!(third.post_melt_quote_call_count(), 0);
}

#[tokio::test(start_paused = true)]
async fn federated_melt_quote_timeout_falls_back_to_next_member() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let expected = melt_quote_create_response();
    let status = melt_quote_status_from_create_response(&expected);
    let fallback_timeout = Duration::from_secs(2);

    let stalled_member = Arc::new(MockMintConnector::new());
    stalled_member.set_active_keyset(advertised.clone());
    stalled_member.set_post_melt_quote_delay(fallback_timeout + Duration::from_secs(60));
    stalled_member.set_post_melt_quote_response(Ok(expected.clone()));
    stalled_member.set_melt_quote_status_response(Ok(status.clone()));

    let second = Arc::new(MockMintConnector::new());
    second.set_active_keyset(advertised.clone());
    second.set_post_melt_quote_response(Ok(expected.clone()));
    second.set_melt_quote_status_response(Ok(status.clone()));

    let third = Arc::new(MockMintConnector::new());
    third.set_active_keyset(advertised);
    third.set_melt_quote_status_response(Ok(status));

    let connectors = BTreeMap::from([
        (
            member_id(1),
            stalled_member.clone() as Arc<dyn MintConnector + Send + Sync>,
        ),
        (
            member_id(2),
            second.clone() as Arc<dyn MintConnector + Send + Sync>,
        ),
        (member_id(3), third as Arc<dyn MintConnector + Send + Sync>),
    ]);
    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            member_fallback_timeout: fallback_timeout,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let melt_quote =
        tokio::spawn(async move { connector.post_melt_quote(bolt11_melt_quote_request()).await });

    tokio::task::yield_now().await;
    tokio::time::advance(fallback_timeout).await;

    let response = melt_quote
        .await
        .expect("melt quote task")
        .expect("threshold-visible melt quote");

    assert_melt_quote_matches_expected(&response, &expected);
    assert_eq!(stalled_member.post_melt_quote_call_count(), 1);
    assert_eq!(second.post_melt_quote_call_count(), 1);
}

#[tokio::test(start_paused = true)]
async fn federated_melt_quote_timeout_falls_back_when_quote_is_not_visible() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let expected = melt_quote_create_response();
    let status = melt_quote_status_from_create_response(&expected);
    let fallback_timeout = Duration::from_secs(2);

    let stalled_member = Arc::new(MockMintConnector::new());
    stalled_member.set_active_keyset(advertised.clone());
    stalled_member.set_post_melt_quote_delay(fallback_timeout + Duration::from_secs(60));
    stalled_member.set_post_melt_quote_response(Ok(expected.clone()));

    let healthy_member = Arc::new(MockMintConnector::new());
    healthy_member.set_active_keyset(advertised.clone());
    healthy_member.set_post_melt_quote_response(Ok(expected.clone()));

    let third = Arc::new(MockMintConnector::new());
    third.set_active_keyset(advertised);

    for connector in [&stalled_member, &healthy_member, &third] {
        connector.push_melt_quote_status_response(Err(crate::Error::UnknownQuote));
        connector.push_melt_quote_status_response(Ok(status.clone()));
    }

    let connectors = BTreeMap::from([
        (
            member_id(1),
            stalled_member.clone() as Arc<dyn MintConnector + Send + Sync>,
        ),
        (
            member_id(2),
            healthy_member.clone() as Arc<dyn MintConnector + Send + Sync>,
        ),
        (member_id(3), third as Arc<dyn MintConnector + Send + Sync>),
    ]);
    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            member_fallback_timeout: fallback_timeout,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let melt_quote =
        tokio::spawn(async move { connector.post_melt_quote(bolt11_melt_quote_request()).await });

    tokio::task::yield_now().await;
    tokio::time::advance(fallback_timeout).await;

    let response = melt_quote
        .await
        .expect("melt quote task")
        .expect("fallback melt quote");

    assert_melt_quote_matches_expected(&response, &expected);
    assert_eq!(stalled_member.post_melt_quote_call_count(), 1);
    assert_eq!(healthy_member.post_melt_quote_call_count(), 1);
}

#[tokio::test]
async fn federated_mint_connector_requires_threshold_melt_quote_visibility_after_creation() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let expected = melt_quote_create_response();
    let mut connectors = BTreeMap::new();

    let quote_coordinator = MockMintConnector::new();
    quote_coordinator.set_active_keyset(advertised.clone());
    quote_coordinator.set_post_melt_quote_response(Ok(expected.clone()));
    quote_coordinator
        .set_melt_quote_status_response(Ok(melt_quote_status_from_create_response(&expected)));
    connectors.insert(
        member_id(1),
        Arc::new(quote_coordinator) as Arc<dyn MintConnector + Send + Sync>,
    );

    for member_id in [member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        connector.set_active_keyset(advertised.clone());
        connector.set_melt_quote_status_response(Err(crate::Error::UnknownQuote));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            mint_quote_visibility_attempts: 1,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let err = connector
        .post_melt_quote(bolt11_melt_quote_request())
        .await
        .expect_err("melt quote should not be threshold visible");

    assert!(
        err.to_string()
            .contains("insufficient federation melt quote status responses"),
        "{err}"
    );
}

#[tokio::test]
async fn federated_mint_connector_retries_melt_quote_visibility_after_creation() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let expected = melt_quote_create_response();
    let status = melt_quote_status_from_create_response(&expected);
    let mut connectors = BTreeMap::new();

    let quote_coordinator = MockMintConnector::new();
    quote_coordinator.set_active_keyset(advertised.clone());
    quote_coordinator.set_post_melt_quote_response(Ok(expected.clone()));
    quote_coordinator.push_melt_quote_status_response(Ok(status.clone()));
    quote_coordinator.push_melt_quote_status_response(Ok(status.clone()));
    connectors.insert(
        member_id(1),
        Arc::new(quote_coordinator) as Arc<dyn MintConnector + Send + Sync>,
    );

    let catching_up_member = MockMintConnector::new();
    catching_up_member.set_active_keyset(advertised.clone());
    catching_up_member.push_melt_quote_status_response(Err(crate::Error::UnknownQuote));
    catching_up_member.push_melt_quote_status_response(Ok(status));
    connectors.insert(
        member_id(2),
        Arc::new(catching_up_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let unavailable_member = MockMintConnector::new();
    unavailable_member.set_active_keyset(advertised);
    unavailable_member.push_melt_quote_status_response(Err(crate::Error::UnknownQuote));
    unavailable_member.push_melt_quote_status_response(Err(crate::Error::UnknownQuote));
    connectors.insert(
        member_id(3),
        Arc::new(unavailable_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            mint_quote_visibility_attempts: 2,
            mint_quote_visibility_retry_delay: Duration::from_millis(1),
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let response = connector
        .post_melt_quote(bolt11_melt_quote_request())
        .await
        .expect("melt quote should become threshold visible");

    assert_melt_quote_matches_expected(&response, &expected);
}

#[tokio::test]
async fn federated_mint_connector_does_not_retry_definitive_quote_errors() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let expected = bolt11_quote_response(&QuoteId::new(), 3);
    let mut connectors = BTreeMap::new();

    let rejecting_member = MockMintConnector::new();
    rejecting_member.set_active_keyset(advertised.clone());
    rejecting_member.set_post_mint_quote_response(Err(crate::Error::InvalidPaymentMethod));
    connectors.insert(
        member_id(1),
        Arc::new(rejecting_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let unused_member = MockMintConnector::new();
    unused_member.set_active_keyset(advertised.clone());
    unused_member.set_post_mint_quote_response(Ok(expected));
    connectors.insert(
        member_id(2),
        Arc::new(unused_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let final_member = MockMintConnector::new();
    final_member.set_active_keyset(advertised);
    connectors.insert(
        member_id(3),
        Arc::new(final_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let err = connector
        .post_mint_quote(bolt11_quote_request(3))
        .await
        .expect_err("definitive error");

    assert!(matches!(err, crate::Error::InvalidPaymentMethod));
}

#[tokio::test]
async fn federated_mint_connector_uses_threshold_batch_mint_quote_status_ignoring_updated_at() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let statuses = vec![
        MintQuoteResponse::Bolt11(mint_quote_status_response_for(
            "mint-quote-1",
            MintQuoteState::Paid,
        )),
        MintQuoteResponse::Bolt11(mint_quote_status_response_for(
            "mint-quote-2",
            MintQuoteState::Unpaid,
        )),
    ];
    let mut connectors = BTreeMap::new();

    let failing_member = MockMintConnector::new();
    failing_member.set_active_keyset(advertised.clone());
    failing_member.set_batch_mint_quote_status_response(Err(crate::Error::Custom(
        "first member unavailable".to_string(),
    )));
    connectors.insert(
        member_id(1),
        Arc::new(failing_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for (member_id, updated_at) in [(member_id(2), 10), (member_id(3), 20)] {
        let connector = MockMintConnector::new();
        connector.set_active_keyset(advertised.clone());
        let mut member_statuses = statuses.clone();
        for response in &mut member_statuses {
            let MintQuoteResponse::Bolt11(response) = response else {
                panic!("test response must be bolt11");
            };
            response.updated_at = updated_at;
        }
        connector.set_batch_mint_quote_status_response(Ok(member_statuses));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = connector
        .post_batch_check_mint_quote_status(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            BatchCheckMintQuoteRequest {
                quotes: vec!["mint-quote-1".to_string(), "mint-quote-2".to_string()],
            },
        )
        .await
        .expect("batch mint quote status");

    assert_eq!(response[0].state(), Some(MintQuoteState::Paid));
    assert_eq!(response[1].state(), Some(MintQuoteState::Unpaid));
}

#[tokio::test]
async fn federated_mint_connector_rejects_single_member_batch_mint_quote_status() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let statuses = vec![
        MintQuoteResponse::Bolt11(mint_quote_status_response_for(
            "mint-quote-1",
            MintQuoteState::Paid,
        )),
        MintQuoteResponse::Bolt11(mint_quote_status_response_for(
            "mint-quote-2",
            MintQuoteState::Unpaid,
        )),
    ];
    let mut connectors = BTreeMap::new();

    let lone_member = MockMintConnector::new();
    lone_member.set_active_keyset(advertised.clone());
    lone_member.set_batch_mint_quote_status_response(Ok(statuses));
    connectors.insert(
        member_id(1),
        Arc::new(lone_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for member_id in [member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        connector.set_active_keyset(advertised.clone());
        connector.set_batch_mint_quote_status_response(Err(crate::Error::UnknownQuote));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let error = connector
        .post_batch_check_mint_quote_status(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            BatchCheckMintQuoteRequest {
                quotes: vec!["mint-quote-1".to_string(), "mint-quote-2".to_string()],
            },
        )
        .await
        .expect_err("one member must not decide batch mint quote status");

    assert!(matches!(error, crate::Error::InvalidMintResponse(_)));
}

#[tokio::test]
async fn federated_mint_connector_uses_threshold_melt_quote_status() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let status = match melt_response_without_change() {
        MeltQuoteResponse::Bolt11(response) => response,
        _ => panic!("expected bolt11 melt response"),
    };
    let mut connectors = BTreeMap::new();

    let failing_member = MockMintConnector::new();
    failing_member.set_active_keyset(advertised.clone());
    failing_member.set_melt_quote_status_response(Err(crate::Error::Custom(
        "first member unavailable".to_string(),
    )));
    connectors.insert(
        member_id(1),
        Arc::new(failing_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for member_id in [member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        connector.set_active_keyset(advertised.clone());
        connector.set_melt_quote_status_response(Ok(status.clone()));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = connector
        .get_melt_quote_status(PaymentMethod::Known(KnownMethod::Bolt11), "melt-quote")
        .await
        .expect("melt quote status");

    assert_eq!(response.state(), MeltQuoteState::Paid);
    assert!(response.change().is_none());
}

#[tokio::test]
async fn federated_mint_connector_waits_for_matching_melt_quote_status_quorum() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    let stale_member = MockMintConnector::new();
    stale_member.set_active_keyset(advertised.clone());
    stale_member.set_melt_quote_status_response(Ok(melt_response_without_change_with_state(
        MeltQuoteState::Pending,
    )));
    connectors.insert(
        member_id(1),
        Arc::new(stale_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for member_id in [member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        connector.set_active_keyset(advertised.clone());
        connector.set_melt_quote_status_response(Ok(melt_response_without_change_with_state(
            MeltQuoteState::Paid,
        )));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = connector
        .get_melt_quote_status(PaymentMethod::Known(KnownMethod::Bolt11), "melt-quote")
        .await
        .expect("melt quote status");

    assert_eq!(response.state(), MeltQuoteState::Paid);
    assert!(response.change().is_none());
}

#[tokio::test]
async fn federated_mint_connector_accepts_melt_quote_status_expiry_skew() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    let stale_member = MockMintConnector::new();
    stale_member.set_active_keyset(advertised.clone());
    stale_member.set_melt_quote_status_response(Ok(melt_response_without_change_with_state(
        MeltQuoteState::Pending,
    )));
    connectors.insert(
        member_id(1),
        Arc::new(stale_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for (member_id, expiry) in [(member_id(2), 42), (member_id(3), 45)] {
        let connector = MockMintConnector::new();
        connector.set_active_keyset(advertised.clone());
        let mut response = melt_response_without_change_with_state(MeltQuoteState::Unpaid);
        response.expiry = expiry;
        connector.set_melt_quote_status_response(Ok(response));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = connector
        .get_melt_quote_status(PaymentMethod::Known(KnownMethod::Bolt11), "melt-quote")
        .await
        .expect("melt quote status");

    assert_eq!(response.state(), MeltQuoteState::Unpaid);
    assert!(response.change().is_none());
}

#[tokio::test]
async fn federated_mint_connector_aggregates_melt_quote_status_change_shares() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let change_outputs = blinded_change_outputs(&bundle);
    let connectors = [member_id(1), member_id(2), member_id(3)]
        .into_iter()
        .map(|member_id| {
            let connector = MockMintConnector::new();
            connector.set_active_keyset(advertised.clone());
            let status = match melt_response(&bundle, member_id, &change_outputs) {
                MeltQuoteResponse::Bolt11(response) => response,
                _ => panic!("expected bolt11 melt response"),
            };
            connector.set_melt_quote_status_response(Ok(status));
            (
                member_id,
                Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
            )
        })
        .collect();

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = connector
        .get_melt_quote_status(PaymentMethod::Known(KnownMethod::Bolt11), "melt-quote")
        .await
        .expect("melt quote status");

    let change = response.change().cloned().expect("change signatures");
    assert_eq!(response.state(), MeltQuoteState::Paid);
    assert_aggregate_response_valid(
        &bundle,
        &signed_change_outputs(&change_outputs),
        &MintResponse { signatures: change },
    );
}

#[tokio::test]
async fn federated_melt_status_recovery_ignores_malformed_change_share() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let change_outputs = blinded_change_outputs(&bundle);
    let mut connectors = BTreeMap::new();
    for id in [member_id(1), member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, id, advertised.clone());
        connector.set_post_melt_response(Ok(melt_response(&bundle, id, &change_outputs)));
        let mut status = match melt_response(&bundle, id, &change_outputs) {
            MeltQuoteResponse::Bolt11(response) => response,
            _ => panic!("expected bolt11 melt response"),
        };
        if id == member_id(3) {
            // The Byzantine member returns matching status metadata with a
            // malformed `c` on every change signature (wrong key kind: it
            // cannot even be parsed as a BLS share).
            for signature in status
                .change
                .iter_mut()
                .flat_map(|change| change.iter_mut())
            {
                signature.c = PublicKey::from_hex(
                    "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
                )
                .expect("secp256k1 public key");
            }
        }
        connector.set_melt_quote_status_response(Ok(status));
        connectors.insert(
            id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    connector
        .post_melt(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MeltRequest::new(
                "melt-quote".to_string(),
                Vec::new(),
                Some(change_outputs.clone()),
            ),
        )
        .await
        .expect("melt response");

    let response = connector
        .get_melt_quote_status(PaymentMethod::Known(KnownMethod::Bolt11), "melt-quote")
        .await
        .expect(
            "status recovery must ignore the invalid member and aggregate the honest threshold",
        );

    assert_aggregate_response_valid(
        &bundle,
        &signed_change_outputs(&change_outputs),
        &MintResponse {
            signatures: response.change().cloned().expect("change signatures"),
        },
    );
}

#[tokio::test]
async fn federated_melt_status_recovery_ignores_wrong_change_share() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let change_outputs = blinded_change_outputs(&bundle);
    let mut connectors = BTreeMap::new();
    for id in [member_id(1), member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, id, advertised.clone());
        connector.set_post_melt_response(Ok(melt_response(&bundle, id, &change_outputs)));
        let mut status = match melt_response(&bundle, id, &change_outputs) {
            MeltQuoteResponse::Bolt11(response) => response,
            _ => panic!("expected bolt11 melt response"),
        };
        if id == member_id(3) {
            // The Byzantine member returns matching status metadata with a
            // well-formed but wrong `c` on every change signature.
            for signature in status
                .change
                .iter_mut()
                .flat_map(|change| change.iter_mut())
            {
                signature.c = PublicKey::from(BlsG1PublicKey::hash_to_curve(
                    b"byzantine invalid change share".as_slice(),
                ));
            }
        }
        connector.set_melt_quote_status_response(Ok(status));
        connectors.insert(
            id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    connector
        .post_melt(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MeltRequest::new(
                "melt-quote".to_string(),
                Vec::new(),
                Some(change_outputs.clone()),
            ),
        )
        .await
        .expect("melt response");

    let response = connector
        .get_melt_quote_status(PaymentMethod::Known(KnownMethod::Bolt11), "melt-quote")
        .await
        .expect(
            "status recovery must ignore the invalid member and aggregate the honest threshold",
        );

    assert_aggregate_response_valid(
        &bundle,
        &signed_change_outputs(&change_outputs),
        &MintResponse {
            signatures: response.change().cloned().expect("change signatures"),
        },
    );
}

#[tokio::test]
async fn federated_mint_connector_rejects_single_member_melt_quote_status() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let status = match melt_response_without_change() {
        MeltQuoteResponse::Bolt11(response) => response,
        _ => panic!("expected bolt11 melt response"),
    };
    let mut connectors = BTreeMap::new();

    let lone_member = MockMintConnector::new();
    lone_member.set_active_keyset(advertised.clone());
    lone_member.set_melt_quote_status_response(Ok(status));
    connectors.insert(
        member_id(1),
        Arc::new(lone_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for member_id in [member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        connector.set_active_keyset(advertised.clone());
        connector.set_melt_quote_status_response(Err(crate::Error::UnknownQuote));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let error = connector
        .get_melt_quote_status(PaymentMethod::Known(KnownMethod::Bolt11), "melt-quote")
        .await
        .expect_err("one member must not decide melt quote status");

    assert!(matches!(error, crate::Error::InvalidMintResponse(_)));
}

#[tokio::test]
async fn federated_mint_connector_aggregates_restore_signature_shares() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let restore_outputs = blinded_change_outputs(&bundle);
    let connectors = [member_id(1), member_id(2), member_id(3)]
        .into_iter()
        .map(|member_id| {
            let connector = MockMintConnector::new();
            set_member_advertisement_with_keyset(
                &connector,
                &bundle,
                member_id,
                advertised.clone(),
            );
            connector._set_restore_response(Ok(restore_response(
                &bundle,
                member_id,
                &restore_outputs,
            )));
            (
                member_id,
                Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
            )
        })
        .collect();
    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = connector
        .post_restore(RestoreRequest {
            outputs: restore_outputs.clone(),
        })
        .await
        .expect("restore response");

    assert_eq!(response.outputs, restore_outputs);
    assert_aggregate_response_valid(
        &bundle,
        &signed_change_outputs(&restore_outputs),
        &MintResponse {
            signatures: response.signatures,
        },
    );
}

#[tokio::test]
async fn federated_mint_connector_accepts_threshold_matching_restore_views() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let restore_outputs = blinded_change_outputs(&bundle);
    let mut connectors = BTreeMap::new();

    for member_id in [member_id(1), member_id(2)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        let member_outputs = if member_id.get() == 2 {
            restore_outputs[0..1].to_vec()
        } else {
            restore_outputs.clone()
        };
        connector._set_restore_response(Ok(restore_response(&bundle, member_id, &member_outputs)));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = MockMintConnector::new();
    set_member_advertisement_with_keyset(&connector, &bundle, member_id(3), advertised);
    connector._set_restore_response(Ok(restore_response(
        &bundle,
        member_id(3),
        &restore_outputs,
    )));
    connectors.insert(
        member_id(3),
        Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = connector
        .post_restore(RestoreRequest {
            outputs: restore_outputs.clone(),
        })
        .await
        .expect("restore response");

    assert_eq!(response.outputs, restore_outputs);
    assert_aggregate_response_valid(
        &bundle,
        &signed_change_outputs(&restore_outputs),
        &MintResponse {
            signatures: response.signatures,
        },
    );
}

#[tokio::test]
async fn federated_mint_connector_rejects_restore_without_matching_threshold() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let restore_outputs = blinded_change_outputs(&bundle);
    let mut connectors = BTreeMap::new();

    for (member_id, member_outputs) in [
        (member_id(1), restore_outputs.clone()),
        (member_id(2), restore_outputs[0..1].to_vec()),
        (member_id(3), Vec::new()),
    ] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector._set_restore_response(Ok(restore_response(&bundle, member_id, &member_outputs)));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let result = connector
        .post_restore(RestoreRequest {
            outputs: restore_outputs,
        })
        .await;

    assert!(matches!(result, Err(crate::Error::InvalidMintResponse(_))));
}

#[tokio::test]
async fn federated_mint_connector_collects_threshold_check_state_views() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let ys = vec![
        PublicKey::from(BlsG1PublicKey::hash_to_curve(b"check-state-y-1")),
        PublicKey::from(BlsG1PublicKey::hash_to_curve(b"check-state-y-2")),
    ];
    let expected = proof_state_response(&ys, &[State::Unspent, State::Pending]);
    let mut connectors = BTreeMap::new();

    let failing_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(
        &failing_member,
        &bundle,
        member_id(1),
        advertised.clone(),
    );
    failing_member
        .set_check_state_response(Err(crate::Error::Custom("member unavailable".to_string())));
    connectors.insert(
        member_id(1),
        Arc::new(failing_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for member_id in [member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.set_check_state_response(Ok(expected.clone()));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = connector
        .post_check_state(CheckStateRequest { ys })
        .await
        .expect("check-state response");

    assert_eq!(response, expected);
}

#[tokio::test]
async fn federated_mint_connector_accepts_threshold_matching_check_state_views() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let ys = vec![
        PublicKey::from(BlsG1PublicKey::hash_to_curve(b"check-state-lag-y-1")),
        PublicKey::from(BlsG1PublicKey::hash_to_curve(b"check-state-lag-y-2")),
    ];
    let stale = proof_state_response(&ys, &[State::Unspent, State::Pending]);
    let expected = proof_state_response(&ys, &[State::Spent, State::Pending]);
    let mut connectors = BTreeMap::new();

    for (member_id, response) in [
        (member_id(1), expected.clone()),
        (member_id(2), stale),
        (member_id(3), expected.clone()),
    ] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.set_check_state_response(Ok(response));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = connector
        .post_check_state(CheckStateRequest { ys })
        .await
        .expect("check-state response");

    assert_eq!(response, expected);
}

#[tokio::test]
async fn federated_mint_connector_polls_check_state_members_concurrently() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let ys = vec![
        PublicKey::from(BlsG1PublicKey::hash_to_curve(b"check-state-parallel-y-1")),
        PublicKey::from(BlsG1PublicKey::hash_to_curve(b"check-state-parallel-y-2")),
    ];
    let expected = proof_state_response(&ys, &[State::Unspent, State::Spent]);
    let mut connectors = BTreeMap::new();

    for member_id in [member_id(1), member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.set_check_state_barrier(barrier.clone());
        connector.set_check_state_response(Ok(expected.clone()));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        connector.post_check_state(CheckStateRequest { ys }),
    )
    .await
    .expect("check-state probes should run concurrently")
    .expect("check-state response");

    assert_eq!(response, expected);
}

#[tokio::test]
async fn federated_mint_connector_rejects_mismatched_check_state_views() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let ys = vec![
        PublicKey::from(BlsG1PublicKey::hash_to_curve(b"check-state-mismatch-y-1")),
        PublicKey::from(BlsG1PublicKey::hash_to_curve(b"check-state-mismatch-y-2")),
    ];
    let first = proof_state_response(&ys, &[State::Unspent, State::Pending]);
    let second = proof_state_response(&ys, &[State::Spent, State::Pending]);
    let mut connectors = BTreeMap::new();

    for (member_id, response) in [(member_id(1), first), (member_id(2), second)] {
        let connector = MockMintConnector::new();
        connector.set_active_keyset(advertised.clone());
        connector.set_check_state_response(Ok(response));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = MockMintConnector::new();
    connector.set_active_keyset(advertised);
    connector.set_check_state_response(Ok(proof_state_response(
        &ys,
        &[State::Unspent, State::Spent],
    )));
    connectors.insert(
        member_id(3),
        Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let result = connector.post_check_state(CheckStateRequest { ys }).await;

    assert!(matches!(result, Err(crate::Error::InvalidMintResponse(_))));
}

#[tokio::test]
async fn federated_mint_connector_sets_auth_wallet_on_all_members() {
    let bundle = threshold_bundle();
    let auth_wallet = test_auth_wallet().await;
    let mut mocks = BTreeMap::new();
    let mut connectors = BTreeMap::new();

    for member_id in [member_id(1), member_id(2), member_id(3)] {
        let connector = Arc::new(MockMintConnector::new());
        mocks.insert(member_id, connector.clone());
        connectors.insert(member_id, connector as Arc<dyn MintConnector + Send + Sync>);
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    connector.set_auth_wallet(Some(auth_wallet.clone())).await;

    for member in mocks.values() {
        let stored = member.get_auth_wallet().await.expect("auth wallet");
        assert_eq!(stored.mint_url, auth_wallet.mint_url);
        assert_eq!(member.auth_wallet_set_count(), 1);
    }

    connector.set_auth_wallet(None).await;

    for member in mocks.values() {
        assert!(member.get_auth_wallet().await.is_none());
        assert_eq!(member.auth_wallet_set_count(), 2);
    }
}

#[tokio::test]
async fn federated_mint_connector_reads_auth_wallet_from_first_available_member() {
    let bundle = threshold_bundle();
    let auth_wallet = test_auth_wallet().await;
    let mut connectors = BTreeMap::new();

    let first_member = Arc::new(MockMintConnector::new());
    connectors.insert(
        member_id(1),
        first_member.clone() as Arc<dyn MintConnector + Send + Sync>,
    );

    let fallback = Arc::new(MockMintConnector::new());
    fallback.set_stored_auth_wallet(Some(auth_wallet.clone()));
    connectors.insert(
        member_id(2),
        fallback as Arc<dyn MintConnector + Send + Sync>,
    );

    let final_member = Arc::new(MockMintConnector::new());
    connectors.insert(
        member_id(3),
        final_member as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let stored = connector.get_auth_wallet().await.expect("auth wallet");

    assert_eq!(stored.mint_url, auth_wallet.mint_url);
    assert!(first_member.get_auth_wallet().await.is_none());
}

#[tokio::test]
async fn federated_mint_connector_rejects_non_v3_melt_change_outputs() {
    let bundle = threshold_bundle();
    let change_outputs = blinded_change_outputs(&bundle);
    let mut bad_change_outputs = change_outputs.clone();
    let keyset = &bundle.public_config.keysets[0];
    bad_change_outputs[0].keyset_id = Id::v2_from_data(
        &keyset.keys(),
        &keyset.unit,
        keyset.input_fee_ppk,
        keyset.final_expiry,
    );
    let connector = FederatedMintConnector::new(
        bundle.public_config.clone(),
        member_connectors_with_melt_change(&bundle, &change_outputs),
    )
    .expect("federated mint connector");
    let result = connector
        .post_melt(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MeltRequest::new(
                "melt-quote".to_string(),
                Vec::new(),
                Some(bad_change_outputs),
            ),
        )
        .await;

    assert!(matches!(
        result,
        Err(crate::Error::Federation(
            FederationError::InvalidFederatedKeysetVersion { .. }
        ))
    ));
}

#[tokio::test]
async fn federated_mint_connector_rejects_non_v3_mint_swap_and_restore_outputs_before_fanout() {
    let bundle = threshold_bundle();
    let connector = || {
        FederatedMintConnector::new(
            bundle.public_config.clone(),
            member_connectors_with_only_keysets(&bundle),
        )
        .expect("federated mint connector")
    };

    let mint_result = connector()
        .post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: "mint-quote".to_string(),
                outputs: non_v3_blinded_outputs(&bundle, b"non-v3 mint output"),
                signature: None,
            },
        )
        .await;
    assert!(matches!(
        mint_result,
        Err(crate::Error::Federation(
            FederationError::InvalidFederatedKeysetVersion { .. }
        ))
    ));

    let batch_mint_result = connector()
        .post_batch_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            BatchMintRequest {
                quotes: vec!["batch-mint-quote".to_string()],
                quote_amounts: None,
                outputs: non_v3_blinded_outputs(&bundle, b"non-v3 batch mint output"),
                signatures: None,
            },
        )
        .await;
    assert!(matches!(
        batch_mint_result,
        Err(crate::Error::Federation(
            FederationError::InvalidFederatedKeysetVersion { .. }
        ))
    ));

    let swap_result = connector()
        .post_swap(SwapRequest::new(
            Vec::new(),
            non_v3_blinded_outputs(&bundle, b"non-v3 swap output"),
        ))
        .await;
    assert!(matches!(
        swap_result,
        Err(crate::Error::Federation(
            FederationError::InvalidFederatedKeysetVersion { .. }
        ))
    ));

    let restore_result = connector()
        .post_restore(RestoreRequest {
            outputs: non_v3_blinded_outputs(&bundle, b"non-v3 restore output"),
        })
        .await;
    assert!(matches!(
        restore_result,
        Err(crate::Error::Federation(
            FederationError::InvalidFederatedKeysetVersion { .. }
        ))
    ));
}

#[tokio::test]
async fn federated_mint_connector_rejects_missing_federation_mint_info_before_fanout() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let mut connectors = member_connectors(&bundle, &outputs);
    let advertised = advertised_keyset(&bundle);

    for member_id in [member_id(1), member_id(2)] {
        let bad_member = MockMintConnector::new();
        bad_member.set_active_keyset(advertised.clone());
        connectors.insert(
            member_id,
            Arc::new(bad_member) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        connector.post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: "paid-quote".to_string(),
                outputs,
                signature: None,
            },
        ),
    )
    .await
    .expect("mint should fail before signing fanout");

    assert!(matches!(
        result,
        Err(crate::Error::InvalidMintResponse(message))
            if message.contains("insufficient federation keyset advertisements")
                && message.contains("did not advertise federation mint info")
    ));
}

#[tokio::test]
async fn federated_mint_connector_rejects_mismatched_federation_mint_info_before_fanout() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let mut connectors = member_connectors(&bundle, &outputs);
    let advertised = advertised_keyset(&bundle);

    for member_id in [member_id(1), member_id(2)] {
        let bad_member = MockMintConnector::new();
        bad_member.set_active_keyset(advertised.clone());
        bad_member.set_mint_info_response(Ok(mismatched_federation_mint_info(&bundle, member_id)));
        connectors.insert(
            member_id,
            Arc::new(bad_member) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        connector.post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: "paid-quote".to_string(),
                outputs,
                signature: None,
            },
        ),
    )
    .await
    .expect("mint should fail before signing fanout");

    assert!(matches!(
        result,
        Err(crate::Error::InvalidMintResponse(message))
            if message.contains("insufficient federation keyset advertisements")
                && message.contains("Federation id mismatch")
    ));
}

#[tokio::test]
async fn federated_mint_connector_fails_without_threshold_v3_member_advertisements() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let mut connectors = member_connectors(&bundle, &outputs);
    let mut advertised = advertised_keyset(&bundle);
    let expected = &bundle.public_config.keysets[0];
    advertised.id = Id::v2_from_data(
        &advertised.keys,
        &expected.unit,
        expected.input_fee_ppk,
        expected.final_expiry,
    );
    for member_id in [member_id(1), member_id(2)] {
        let bad_member = MockMintConnector::new();
        set_member_advertisement_with_keyset(&bad_member, &bundle, member_id, advertised.clone());
        connectors.insert(
            member_id,
            Arc::new(bad_member) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        connector.post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: "paid-quote".to_string(),
                outputs,
                signature: None,
            },
        ),
    )
    .await
    .expect("mint should fail before signing fanout");

    assert!(matches!(
        result,
        Err(crate::Error::InvalidMintResponse(message))
            if message.contains("insufficient federation keyset advertisements")
    ));
}

#[tokio::test]
async fn federated_mint_connector_fails_without_threshold_matching_member_advertisements() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let mut connectors = member_connectors(&bundle, &outputs);
    let mut advertised = advertised_keyset(&bundle);
    advertised.keys = cdk_common::Keys::new(BTreeMap::new());
    for member_id in [member_id(1), member_id(2)] {
        let bad_member = MockMintConnector::new();
        set_member_advertisement_with_keyset(&bad_member, &bundle, member_id, advertised.clone());
        connectors.insert(
            member_id,
            Arc::new(bad_member) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        connector.post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: "paid-quote".to_string(),
                outputs,
                signature: None,
            },
        ),
    )
    .await
    .expect("mint should fail before signing fanout");

    assert!(matches!(
        result,
        Err(crate::Error::InvalidMintResponse(message))
            if message.contains("insufficient federation keyset advertisements")
    ));
}
