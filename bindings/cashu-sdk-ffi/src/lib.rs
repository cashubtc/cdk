mod error;
mod mint;
mod types;
mod wallet;

mod ffi {
    pub use cashu_ffi::{
        Amount, BlindedMessage, BlindedSignature, Bolt11Invoice, CashuError, CheckSpendableRequest,
        CheckSpendableResponse, CurrencyUnit, Id, InvoiceStatus, KeyPair, KeySet, KeySetInfo,
        KeySetResponse, Keys, KeysResponse, MeltBolt11Request, MeltBolt11Response, MeltQuote,
        MeltQuoteBolt11Request, MeltQuoteBolt11Response, MintBolt11Request, MintBolt11Response,
        MintInfo, MintKeySet, MintProof, MintProofs, MintQuote, MintQuoteBolt11Request,
        MintQuoteBolt11Response, MintVersion, Nut05MeltBolt11Request, Nut05MeltBolt11Response,
        PreMintSecrets, Proof, PublicKey, Secret, SecretKey, SwapRequest, SwapResponse, Token,
    };

    pub use crate::error::CashuSdkError;
    pub use crate::mint::Mint;
    pub use crate::types::{Melted, MintKeySetInfo, ProofsStatus, SendProofs};
    pub use crate::wallet::Wallet;

    // UDL
    uniffi::include_scaffolding!("cashu_sdk");
}

pub use ffi::*;
