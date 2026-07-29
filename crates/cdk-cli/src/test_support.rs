use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use cdk::mint_url::MintUrl;
use cdk::nuts::nut01::{BlsG1PublicKey, BlsSecretKey};
use cdk::nuts::{
    BatchCheckMintQuoteRequest, BatchMintRequest, BlindSignature, CheckStateRequest,
    CheckStateResponse, CurrencyUnit, Id, KeySet, KeySetInfo, KeysetResponse, MeltRequest,
    MintInfo, MintRequest, MintResponse, PaymentMethod, ProofState, PublicKey, RestoreRequest,
    RestoreResponse, State, SwapRequest, SwapResponse,
};
use cdk::wallet::federation::FederatedMintConnector;
use cdk::wallet::{AuthWallet, LnurlPayInvoiceResponse, LnurlPayResponse, MintConnector};
use cdk::{
    Amount, Error, MeltQuoteCreateResponse, MeltQuoteRequest, MeltQuoteResponse, MintQuoteRequest,
    MintQuoteResponse,
};
use cdk_common::federation::{
    generate_trusted_dealer_configs, FederationAcceptedSigningRequest, FederationOperation,
    FederationOperationEnvelope, FederationSwapOperation, InMemoryFederationJournal, MemberId,
    ThresholdParams, TrustedDealerConfigBundle, TrustedDealerKeysetSpec, TrustedDealerMember,
};
use cdk_common::SecretKey;

#[derive(Debug, Clone, Copy)]
pub enum MemberFault {
    CheckState(State),
    RestoreOutput(Option<usize>),
    SwapShare {
        signer_id: MemberId,
        delay: Duration,
    },
}

#[derive(Debug)]
struct ScriptedFederationMember {
    member_id: MemberId,
    keyset: KeySet,
    mint_info: MintInfo,
    fault: MemberFault,
    invalid_share_calls: Arc<AtomicUsize>,
}

impl ScriptedFederationMember {
    fn unsupported(operation: &str) -> Error {
        Error::Custom(format!("unsupported test operation: {operation}"))
    }
}

#[async_trait]
impl MintConnector for ScriptedFederationMember {
    async fn resolve_dns_txt(&self, _domain: &str) -> Result<Vec<String>, Error> {
        Err(Self::unsupported("resolve_dns_txt"))
    }

    async fn fetch_lnurl_pay_request(&self, _url: &str) -> Result<LnurlPayResponse, Error> {
        Err(Self::unsupported("fetch_lnurl_pay_request"))
    }

    async fn fetch_lnurl_invoice(&self, _url: &str) -> Result<LnurlPayInvoiceResponse, Error> {
        Err(Self::unsupported("fetch_lnurl_invoice"))
    }

    async fn get_mint_keys(&self) -> Result<Vec<KeySet>, Error> {
        Ok(vec![self.keyset.clone()])
    }

    async fn get_mint_keyset(&self, keyset_id: Id) -> Result<KeySet, Error> {
        if self.keyset.id == keyset_id {
            Ok(self.keyset.clone())
        } else {
            Err(Error::UnknownKeySet)
        }
    }

    async fn get_mint_keysets(&self) -> Result<KeysetResponse, Error> {
        Ok(KeysetResponse {
            keysets: vec![KeySetInfo {
                id: self.keyset.id,
                unit: self.keyset.unit.clone(),
                active: self.keyset.active.unwrap_or(true),
                input_fee_ppk: self.keyset.input_fee_ppk,
                final_expiry: self.keyset.final_expiry,
            }],
        })
    }

    async fn post_mint_quote(
        &self,
        _request: MintQuoteRequest,
    ) -> Result<MintQuoteResponse<String>, Error> {
        Err(Self::unsupported("post_mint_quote"))
    }

    async fn post_mint(
        &self,
        _method: &PaymentMethod,
        _request: MintRequest<String>,
    ) -> Result<MintResponse, Error> {
        Err(Self::unsupported("post_mint"))
    }

    async fn post_batch_check_mint_quote_status(
        &self,
        _method: &PaymentMethod,
        _request: BatchCheckMintQuoteRequest<String>,
    ) -> Result<Vec<MintQuoteResponse<String>>, Error> {
        Err(Self::unsupported("post_batch_check_mint_quote_status"))
    }

    async fn post_batch_mint(
        &self,
        _method: &PaymentMethod,
        _request: BatchMintRequest<String>,
    ) -> Result<MintResponse, Error> {
        Err(Self::unsupported("post_batch_mint"))
    }

    async fn post_melt_quote(
        &self,
        _request: MeltQuoteRequest,
    ) -> Result<MeltQuoteCreateResponse<String>, Error> {
        Err(Self::unsupported("post_melt_quote"))
    }

    async fn get_mint_quote_status(
        &self,
        _method: PaymentMethod,
        _quote_id: &str,
    ) -> Result<MintQuoteResponse<String>, Error> {
        Err(Self::unsupported("get_mint_quote_status"))
    }

    async fn get_melt_quote_status(
        &self,
        _method: PaymentMethod,
        _quote_id: &str,
    ) -> Result<MeltQuoteResponse<String>, Error> {
        Err(Self::unsupported("get_melt_quote_status"))
    }

    async fn post_melt(
        &self,
        _method: &PaymentMethod,
        _request: MeltRequest<String>,
    ) -> Result<MeltQuoteResponse<String>, Error> {
        Err(Self::unsupported("post_melt"))
    }

    async fn post_swap(&self, request: SwapRequest) -> Result<SwapResponse, Error> {
        let MemberFault::SwapShare { signer_id, delay } = self.fault else {
            return Err(Self::unsupported("post_swap"));
        };
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        if signer_id != self.member_id {
            self.invalid_share_calls.fetch_add(1, Ordering::Relaxed);
        }
        let private_config = test_bundle()
            .private_configs
            .get(&signer_id)
            .expect("member private config");
        let envelope = FederationOperationEnvelope::new(
            test_bundle().public_config.federation_id,
            FederationOperation::Swap(FederationSwapOperation {
                request: request.clone(),
            }),
        );
        let entry = InMemoryFederationJournal::new()
            .append(envelope)
            .expect("accepted swap")
            .entry()
            .clone();
        let signing_request = FederationAcceptedSigningRequest::from_journal_entry(
            &test_bundle().public_config,
            &entry,
            request.outputs(),
        )
        .expect("accepted signing request");
        let shares = private_config
            .sign_accepted_blinded_messages(&test_bundle().public_config, &signing_request)
            .expect("member signature shares");
        Ok(SwapResponse {
            signatures: shares
                .shares
                .into_iter()
                .map(|share| BlindSignature {
                    amount: share.amount,
                    keyset_id: share.keyset_id,
                    c: share.share.signature().into(),
                    dleq: None,
                })
                .collect(),
        })
    }

    async fn get_mint_info(&self) -> Result<MintInfo, Error> {
        Ok(self.mint_info.clone())
    }

    async fn post_check_state(
        &self,
        request: CheckStateRequest,
    ) -> Result<CheckStateResponse, Error> {
        let MemberFault::CheckState(state) = self.fault else {
            return Err(Self::unsupported("post_check_state"));
        };
        Ok(CheckStateResponse {
            states: request
                .ys
                .into_iter()
                .map(|y| ProofState {
                    y,
                    state,
                    witness: None,
                })
                .collect(),
        })
    }

    async fn post_restore(&self, request: RestoreRequest) -> Result<RestoreResponse, Error> {
        let MemberFault::RestoreOutput(output_index) = self.fault else {
            return Err(Self::unsupported("post_restore"));
        };
        let outputs = output_index
            .and_then(|index| request.outputs.get(index).cloned())
            .into_iter()
            .collect::<Vec<_>>();
        let signatures = outputs
            .iter()
            .map(|output| BlindSignature {
                amount: output.amount,
                keyset_id: output.keyset_id,
                c: output.blinded_secret,
                dleq: None,
            })
            .collect();
        Ok(RestoreResponse {
            outputs,
            signatures,
        })
    }

    async fn get_auth_wallet(&self) -> Option<AuthWallet> {
        None
    }

    async fn set_auth_wallet(&self, _wallet: Option<AuthWallet>) {}
}

pub struct FederatedConnectorFixture {
    pub connector: Arc<dyn MintConnector + Send + Sync>,
    pub invalid_share_calls: Arc<AtomicUsize>,
    pub keyset_id: Id,
    pub mint_url: MintUrl,
}

pub fn federated_connector_with_faults(faults: [MemberFault; 3]) -> FederatedConnectorFixture {
    let bundle = test_bundle();
    let public_config = bundle.public_config.clone();
    let federated_keyset = public_config
        .keysets
        .first()
        .expect("active federated keyset");
    let keyset = KeySet {
        id: federated_keyset.id,
        unit: federated_keyset.unit.clone(),
        active: Some(federated_keyset.active),
        keys: federated_keyset.keys(),
        input_fee_ppk: federated_keyset.input_fee_ppk,
        final_expiry: federated_keyset.final_expiry,
    };
    let keyset_id = keyset.id;
    let mint_url = public_config
        .members
        .get(&member_id(1))
        .expect("member one")
        .public_mint_url
        .clone();
    let invalid_share_calls = Arc::new(AtomicUsize::new(0));
    let connectors = [member_id(1), member_id(2), member_id(3)]
        .into_iter()
        .zip(faults)
        .map(|(member_id, fault)| {
            let mint_info = MintInfo::new().federation(
                public_config
                    .mint_info_for_member(member_id)
                    .expect("member mint info"),
            );
            (
                member_id,
                Arc::new(ScriptedFederationMember {
                    member_id,
                    keyset: keyset.clone(),
                    mint_info,
                    fault,
                    invalid_share_calls: Arc::clone(&invalid_share_calls),
                }) as Arc<dyn MintConnector + Send + Sync>,
            )
        })
        .collect();
    let connector = Arc::new(
        FederatedMintConnector::new(public_config, connectors)
            .expect("federated connector fixture"),
    );

    FederatedConnectorFixture {
        connector,
        invalid_share_calls,
        keyset_id,
        mint_url,
    }
}

fn test_bundle() -> &'static TrustedDealerConfigBundle {
    static BUNDLE: OnceLock<TrustedDealerConfigBundle> = OnceLock::new();
    BUNDLE.get_or_init(|| {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("threshold parameters");
        let members = [member_id(1), member_id(2), member_id(3)]
            .into_iter()
            .map(test_member)
            .collect();
        generate_trusted_dealer_configs(
            params,
            members,
            vec![TrustedDealerKeysetSpec::new(
                CurrencyUnit::Sat,
                true,
                0,
                None,
                BTreeMap::from([
                    (Amount::from(1), vec![bls_key(1), bls_key(2)]),
                    (Amount::from(2), vec![bls_key(3), bls_key(4)]),
                ]),
            )],
        )
        .expect("trusted dealer fixture")
    })
}

pub fn member_id(id: u16) -> MemberId {
    MemberId::new(id).expect("valid member id")
}

fn test_member(member_id: MemberId) -> TrustedDealerMember {
    TrustedDealerMember::new(
        member_id,
        MintUrl::from_str(&format!(
            "https://cli-state-mint{}.invalid",
            member_id.get()
        ))
        .expect("public mint URL"),
        MintUrl::from_str(&format!("https://cli-state-fed{}.invalid", member_id.get()))
            .expect("federation API URL"),
        identity_secret(member_id),
    )
}

fn identity_secret(member_id: MemberId) -> SecretKey {
    let mut bytes = [0u8; 32];
    bytes[31] = member_id.get() as u8;
    SecretKey::from_slice(&bytes).expect("identity secret")
}

fn bls_key(byte: u8) -> BlsSecretKey {
    BlsSecretKey::from_reduced_bytes(&[byte; 32])
}

pub fn test_bls_signature(label: &[u8]) -> PublicKey {
    PublicKey::from(BlsG1PublicKey::hash_to_curve(label))
}

#[cfg(feature = "npubcash")]
pub fn federation_public_config() -> cdk_common::federation::FederationConfig {
    test_bundle().public_config.clone()
}
