mod error;
mod nuts;
mod types;

mod ffi {
    pub use cashu::types::InvoiceStatus;

    pub use crate::error::CashuError;
    pub use crate::nuts::nut00::blinded_message::BlindedMessage;
    pub use crate::nuts::nut00::blinded_messages::BlindedMessages;
    pub use crate::nuts::nut00::blinded_signature::BlindedSignature;
    pub use crate::nuts::nut00::mint_proofs::MintProofs;
    pub use crate::nuts::nut00::proof::mint::Proof as MintProof;
    pub use crate::nuts::nut00::proof::Proof;
    pub use crate::nuts::nut00::token::Token;
    pub use crate::nuts::nut01::key_pair::KeyPair;
    pub use crate::nuts::nut01::keys::{Keys, KeysResponse};
    pub use crate::nuts::nut01::public_key::PublicKey;
    pub use crate::nuts::nut01::secret_key::SecretKey;
    pub use crate::nuts::nut02::{Id, KeySet, KeySetResponse, MintKeySet};
    pub use crate::nuts::nut03::RequestMintResponse;
    pub use crate::nuts::nut04::{MintRequest, PostMintResponse};
    pub use crate::nuts::nut05::{
        CheckFeesRequest, CheckFeesResponse, MeltRequest as Nut05MeltRequest,
        MeltResponse as Nut05MeltResponse,
    };
    pub use crate::nuts::nut06::{SplitRequest, SplitResponse};
    pub use crate::nuts::nut07::{CheckSpendableRequest, CheckSpendableResponse};
    pub use crate::nuts::nut08::{MeltRequest, MeltResponse};
    pub use crate::nuts::nut09::{MintInfo, MintVersion};
    pub use crate::types::amount::Amount;
    pub use crate::types::{Bolt11Invoice, KeySetInfo, Secret};

    // UDL
    uniffi::include_scaffolding!("cashu");
}

pub use ffi::*;
