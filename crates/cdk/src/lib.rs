//! Rust implementation of the Cashu Protocol
#![doc = include_str!("../README.md")]
#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

// Disallow enabling `tor` feature on wasm32 with a clear error.
#[cfg(all(target_arch = "wasm32", feature = "tor"))]
compile_error!("The 'tor' feature is not supported on wasm32 targets (browser). Disable the 'tor' feature or use a non-wasm32 target.");

pub mod cdk_database {
    //! CDK Database
    pub use cdk_common::database::Error;
    #[cfg(all(feature = "mint", feature = "auth"))]
    pub use cdk_common::database::MintAuthDatabase;
    #[cfg(feature = "wallet")]
    pub use cdk_common::database::WalletDatabase;
    #[cfg(feature = "mint")]
    pub use cdk_common::database::{
        MintDatabase, MintKVStore, MintKVStoreDatabase, MintKVStoreTransaction, MintKeysDatabase,
        MintProofsDatabase, MintQuotesDatabase, MintSignaturesDatabase, MintTransaction,
    };
}

#[cfg(feature = "mint")]
pub mod mint;
#[cfg(feature = "wallet")]
pub mod wallet;

#[cfg(test)]
mod test_helpers;

#[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
mod bip353;

#[cfg(feature = "wallet")]
mod lightning_address;

#[cfg(all(any(feature = "wallet", feature = "mint"), feature = "auth"))]
mod oidc_client;

/// Re-export batch mint types
#[cfg(feature = "mint")]
pub use cdk_common::mint::{
    BatchMintRequest, BatchQuoteStatusItem, BatchQuoteStatusRequest, BatchQuoteStatusResponse,
    MintQuoteBolt12BatchStatusResponse,
};
#[cfg(feature = "mint")]
#[doc(hidden)]
pub use cdk_common::payment as cdk_payment;
/// Re-export amount type
#[doc(hidden)]
pub use cdk_common::{
    amount, common as types, dhke, ensure_cdk,
    error::{self, Error},
    lightning_invoice, mint_url, nuts, secret, util, ws, Amount, Bolt11Invoice,
};
#[cfg(all(any(feature = "wallet", feature = "mint"), feature = "auth"))]
pub use oidc_client::OidcClient;

#[cfg(any(feature = "wallet", feature = "mint"))]
pub mod event;
pub mod fees;
pub mod invoice;

#[doc(hidden)]
pub use bitcoin::secp256k1;
#[cfg(feature = "mint")]
#[doc(hidden)]
pub use mint::Mint;
#[cfg(feature = "wallet")]
#[doc(hidden)]
pub use wallet::{Wallet, WalletSubscription};

#[doc(hidden)]
pub use self::util::SECP256K1;
#[cfg(feature = "wallet")]
#[doc(hidden)]
pub use self::wallet::HttpClient;

/// Result
#[doc(hidden)]
pub type Result<T, E = Box<dyn std::error::Error>> = std::result::Result<T, E>;

/// Re-export subscription
pub use cdk_common::subscription;
/// Re-export futures::Stream
#[cfg(any(feature = "wallet", feature = "mint"))]
pub use futures::{Stream, StreamExt};
/// Payment Request
#[cfg(feature = "wallet")]
pub use wallet::payment_request;
