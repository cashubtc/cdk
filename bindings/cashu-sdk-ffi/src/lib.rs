mod ffi {
    pub use cashu_ffi::{
        Amount, BlindedMessage, BlindedMessages, BlindedSignature, CashuError, CheckFeesRequest,
        CheckFeesResponse, CheckSpendableRequest, CheckSpendableResponse, InvoiceStatus, KeyPair,
        KeySet, KeySetResponse, Keys, MeltRequest, MeltResponse, MintInfo, MintProof, MintProofs,
        MintRequest, MintVersion, Nut05MeltRequest, Nut05MeltResponse, PostMintResponse, Proof,
        PublicKey, RequestMintResponse, SecretKey, SplitRequest, SplitResponse, Token,
    };

    // UDL
    uniffi::include_scaffolding!("cashu_sdk");
}

pub use ffi::*;
