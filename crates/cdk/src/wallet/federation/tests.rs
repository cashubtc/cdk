use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use bip39::Mnemonic;
use cdk_common::federation::{
    generate_trusted_dealer_configs, FederatedKeyset, FederationAcceptedSigningRequest,
    FederationConfig, FederationError, FederationId, FederationInvite, FederationJournalEntry,
    FederationKeysetRotationOperation, FederationMeltQuoteMetadata, FederationMintOperation,
    FederationMintRequest, FederationOperation, FederationOperationEnvelope, FederationOperationId,
    InMemoryFederationJournal, MemberId, ThresholdParams, TrustedDealerConfigBundle,
    TrustedDealerKeysetSpec, TrustedDealerMember,
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
    has_threshold_matching_mint_quote_status, member_websocket_url, FederatedMintConnector,
    FederatedMintConnectorConfig, ShareAmountValidation, FEDERATION_CATCH_UP_MAX_RETRIES,
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

mod local_connector;

use self::local_connector::{http_member_connectors, LocalFederatedMemberConnector};

mod fixtures;

use self::fixtures::*;

mod admission;
mod connector_config;
mod keyset_discovery;
mod mint_quotes;
mod quote_fallback;
mod quote_status;
mod registry;
mod restore_and_check_state;
mod signature_aggregation;
mod threshold_responses;
