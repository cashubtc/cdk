mod client;
mod error;
mod mint;
mod types;
mod wallet;

mod ffi {
    pub use cashu_ffi::{
        Amount, BlindedMessage, BlindedMessages, BlindedSignature, Bolt11Invoice, CashuError,
        CheckFeesRequest, CheckFeesResponse, CheckSpendableRequest, CheckSpendableResponse, Id,
        InvoiceStatus, KeyPair, KeySet, KeySetInfo, KeySetResponse, Keys, KeysResponse,
        MeltRequest, MeltResponse, MintInfo, MintKeySet, MintProof, MintProofs, MintRequest,
        MintVersion, Nut05MeltRequest, Nut05MeltResponse, PostMintResponse, Proof, PublicKey,
        RequestMintResponse, Secret, SecretKey, SplitRequest, SplitResponse, Token,
    };

    pub use crate::client::Client;
    pub use crate::error::CashuSdkError;
    pub use crate::mint::Mint;
    pub use crate::types::{Melted, ProofsStatus, SendProofs};
    pub use crate::wallet::Wallet;

    // UDL
    uniffi::include_scaffolding!("cashu_sdk");
}

pub use ffi::*;
