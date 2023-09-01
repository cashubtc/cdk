mod client;
mod error;
mod types;
mod wallet;

mod ffi {
    pub use cashu_ffi::{
        Amount, BlindedMessage, BlindedMessages, BlindedSignature, Bolt11Invoice, CashuError,
        CheckFeesRequest, CheckFeesResponse, CheckSpendableRequest, CheckSpendableResponse,
        InvoiceStatus, KeyPair, KeySet, KeySetResponse, Keys, MeltRequest, MeltResponse, MintInfo,
        MintProof, MintProofs, MintRequest, MintVersion, Nut05MeltRequest, Nut05MeltResponse,
        PostMintResponse, Proof, PublicKey, RequestMintResponse, SecretKey, SplitRequest,
        SplitResponse, Token,
    };

    pub use crate::client::Client;
    pub use crate::error::CashuSdkError;
    pub use crate::types::{Melted, SendProofs};
    pub use crate::wallet::Wallet;

    // UDL
    uniffi::include_scaffolding!("cashu_sdk");
}

pub use ffi::*;
