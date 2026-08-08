//! State types for the Mint (Issue) saga.
//!
//! Each state is a distinct type that holds the data relevant to that stage
//! of the mint operation. The type state pattern ensures that only valid
//! operations are available at each stage.

use core::fmt;

use cdk_common::wallet::{KeysetLoadPolicy, WalletSaga};
use uuid::Uuid;

use crate::nuts::{BatchMintRequest, Id, PaymentMethod, PreMintSecrets, Proofs};
use crate::wallet::MintQuote;

/// Type alias for MintRequest with String quote ID
pub type MintRequestString = crate::nuts::MintRequest<String>;

/// Initial state - operation ID assigned, no work done yet.
#[derive(Debug)]
pub struct Initial {
    /// Unique operation identifier for tracking and crash recovery
    pub operation_id: Uuid,
    /// Policy controlling how keysets are loaded during this saga
    pub keyset_policy: KeysetLoadPolicy,
}

/// The mint request type - either single quote or batch.
pub enum PreparedMintRequest {
    /// Single quote mint request (legacy NUT-04)
    Single {
        /// Quote ID being minted
        quote_id: String,
        /// Quote information
        quote_info: MintQuote,
        /// Mint request ready to send
        request: MintRequestString,
    },
    /// Batch mint request (NUT-29)
    Batch {
        /// Quote IDs being minted
        quote_ids: Vec<String>,
        /// Quote information for each quote
        quote_infos: Vec<MintQuote>,
        /// Batch mint request ready to send
        request: BatchMintRequest<String>,
    },
}

impl fmt::Debug for PreparedMintRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Single {
                quote_id,
                quote_info,
                request,
            } => f
                .debug_struct("PreparedMintRequest::Single")
                .field("quote_id", quote_id)
                .field("mint_url", &quote_info.mint_url)
                .field("payment_method", &quote_info.payment_method)
                .field("amount", &quote_info.amount)
                .field("unit", &quote_info.unit)
                .field("state", &quote_info.state)
                .field(
                    "output_amounts",
                    &request
                        .outputs
                        .iter()
                        .map(|output| output.amount)
                        .collect::<Vec<_>>(),
                )
                .field("has_signature", &request.signature.is_some())
                .finish(),
            Self::Batch {
                quote_ids,
                quote_infos,
                request,
            } => f
                .debug_struct("PreparedMintRequest::Batch")
                .field("quote_ids", quote_ids)
                .field(
                    "quote_amounts",
                    &quote_infos
                        .iter()
                        .map(|quote| quote.amount)
                        .collect::<Vec<_>>(),
                )
                .field(
                    "output_amounts",
                    &request
                        .outputs
                        .iter()
                        .map(|output| output.amount)
                        .collect::<Vec<_>>(),
                )
                .field("has_signatures", &request.signatures.is_some())
                .finish(),
        }
    }
}

/// Prepared state - quote validated, premint secrets created, ready to execute.
pub struct Prepared {
    /// Unique operation identifier
    pub operation_id: Uuid,
    /// Active keyset ID
    pub active_keyset_id: Id,
    /// Premint secrets
    pub premint_secrets: PreMintSecrets,
    /// Mint request (single or batch)
    pub mint_request: PreparedMintRequest,
    /// Payment method (Bolt11 or Bolt12)
    pub payment_method: PaymentMethod,
    /// Policy controlling how keysets are loaded
    pub keyset_policy: KeysetLoadPolicy,
    /// Persisted saga for optimistic locking and recovery
    pub saga: WalletSaga,
}

impl fmt::Debug for Prepared {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Prepared")
            .field("operation_id", &self.operation_id)
            .field("active_keyset_id", &self.active_keyset_id)
            .field(
                "premint_amounts",
                &self
                    .premint_secrets
                    .secrets
                    .iter()
                    .map(|secret| secret.amount)
                    .collect::<Vec<_>>(),
            )
            .field("mint_request", &self.mint_request)
            .field("payment_method", &self.payment_method)
            .field("keyset_policy", &self.keyset_policy)
            .field("saga_id", &self.saga.id)
            .field("saga_state", &self.saga.state)
            .field("saga_version", &self.saga.version)
            .finish()
    }
}

/// Finalized state - mint completed successfully, proofs available.
pub struct Finalized {
    /// Minted proofs
    pub proofs: Proofs,
}

impl fmt::Debug for Finalized {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Finalized")
            .field(
                "proof_amounts",
                &self
                    .proofs
                    .iter()
                    .map(|proof| proof.amount)
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use cdk_common::wallet::{
        IssueSagaState, KeysetLoadPolicy, MintOperationData, OperationData, WalletSaga,
        WalletSagaState,
    };
    use cdk_common::CurrencyUnit;
    use uuid::Uuid;

    use super::{Finalized, Prepared, PreparedMintRequest};
    use crate::nuts::{MintQuoteState, MintRequest, PaymentMethod, PreMintSecrets, SecretKey};
    use crate::secret::Secret;
    use crate::wallet::test_utils::{test_keyset_id, test_mint_url, test_proof};
    use crate::wallet::MintQuote;
    use crate::Amount;

    const SECRET_MARKER: &str = "super_secret_spending_material_xyz";
    const QUOTE_ID: &str = "debuggable-mint-quote-id";

    fn mint_saga(operation_id: Uuid) -> WalletSaga {
        WalletSaga::new(
            operation_id,
            WalletSagaState::Issue(IssueSagaState::SecretsPrepared),
            Amount::from(1),
            test_mint_url(),
            CurrencyUnit::Sat,
            OperationData::Mint(MintOperationData::new_single(
                QUOTE_ID.to_string(),
                Amount::from(1),
                Some(0),
                Some(1),
                None,
            )),
        )
    }

    #[allow(clippy::use_debug)]
    #[test]
    fn issue_state_debug_redacts_secrets_but_keeps_quote_ids() {
        let operation_id = Uuid::new_v4();
        let keyset_id = test_keyset_id();
        let premint_secrets = PreMintSecrets::from_secrets(
            keyset_id,
            vec![Amount::from(1)],
            vec![Secret::new(SECRET_MARKER)],
        )
        .expect("valid pre-mint secret");
        let outputs = premint_secrets
            .secrets
            .iter()
            .map(|secret| secret.blinded_message.clone())
            .collect();
        let quote_signing_key = SecretKey::generate();
        let quote_signing_key_hex = quote_signing_key.to_secret_hex();
        let mint_request = PreparedMintRequest::Single {
            quote_id: QUOTE_ID.to_string(),
            quote_info: MintQuote {
                id: QUOTE_ID.to_string(),
                mint_url: test_mint_url(),
                payment_method: PaymentMethod::BOLT11,
                amount: Some(Amount::from(1)),
                unit: CurrencyUnit::Sat,
                request: "lnbc1debug-payment-request".to_string(),
                state: MintQuoteState::Paid,
                expiry: 0,
                secret_key: Some(quote_signing_key),
                amount_issued: Amount::ZERO,
                amount_paid: Amount::from(1),
                updated_at: 0,
                estimated_blocks: None,
                used_by_operation: None,
                version: 0,
            },
            request: MintRequest {
                quote: QUOTE_ID.to_string(),
                outputs,
                signature: Some("debug-signature".to_string()),
            },
        };
        let prepared = Prepared {
            operation_id,
            active_keyset_id: keyset_id,
            premint_secrets,
            mint_request,
            payment_method: PaymentMethod::BOLT11,
            keyset_policy: KeysetLoadPolicy::default(),
            saga: mint_saga(operation_id),
        };
        let mut proof = test_proof(keyset_id, 1);
        proof.secret = Secret::new(SECRET_MARKER);
        let finalized = Finalized {
            proofs: vec![proof],
        };

        let prepared_debug = format!("{prepared:?}");
        let finalized_debug = format!("{finalized:?}");
        assert!(!prepared_debug.contains(SECRET_MARKER));
        assert!(!prepared_debug.contains(&quote_signing_key_hex));
        assert!(!finalized_debug.contains(SECRET_MARKER));
        assert!(prepared_debug.contains(QUOTE_ID));
    }
}
