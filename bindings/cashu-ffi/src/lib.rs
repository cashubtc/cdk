mod error;
mod nuts;
mod types;

mod ffi {
    pub use cashu::types::InvoiceStatus;

    pub use crate::error::CashuError;
    pub use crate::nuts::nut00::blinded_message::BlindedMessage;
    pub use crate::nuts::nut00::blinded_signature::BlindedSignature;
    pub use crate::nuts::nut00::mint_proofs::MintProofs;
    pub use crate::nuts::nut00::premint_secrets::PreMintSecrets;
    pub use crate::nuts::nut00::proof::mint::Proof as MintProof;
    pub use crate::nuts::nut00::proof::Proof;
    pub use crate::nuts::nut00::token::{CurrencyUnit, Token};
    pub use crate::nuts::nut01::key_pair::KeyPair;
    pub use crate::nuts::nut01::keys::{Keys, KeysResponse};
    pub use crate::nuts::nut01::public_key::PublicKey;
    pub use crate::nuts::nut01::secret_key::SecretKey;
    pub use crate::nuts::nut02::{Id, KeySet, KeySetResponse, MintKeySet};
    pub use crate::nuts::nut03::{SwapRequest, SwapResponse};
    pub use crate::nuts::nut04::{
        MintBolt11Request, MintBolt11Response, MintQuoteBolt11Request, MintQuoteBolt11Response,
    };
    pub use crate::nuts::nut05::{
        MeltBolt11Request as Nut05MeltBolt11Request, MeltBolt11Response as Nut05MeltBolt11Response,
        MeltQuoteBolt11Request, MeltQuoteBolt11Response,
    };
    pub use crate::nuts::nut06::{MintInfo, MintVersion};
    pub use crate::nuts::nut07::{CheckSpendableRequest, CheckSpendableResponse};
    pub use crate::nuts::nut08::{MeltBolt11Request, MeltBolt11Response};
    pub use crate::types::{Amount, Bolt11Invoice, KeySetInfo, MeltQuote, MintQuote, Secret};

    // UDL
    uniffi::include_scaffolding!("cashu");
}

pub use ffi::*;
