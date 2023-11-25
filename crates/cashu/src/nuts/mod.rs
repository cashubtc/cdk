pub mod nut00;
pub mod nut01;
pub mod nut02;
pub mod nut03;
pub mod nut04;
pub mod nut05;
pub mod nut06;
#[cfg(feature = "nut07")]
pub mod nut07;
#[cfg(feature = "nut08")]
pub mod nut08;
#[cfg(feature = "nut09")]
pub mod nut09;

#[cfg(feature = "wallet")]
pub use nut00::wallet::{BlindedMessages, Token};
pub use nut00::{BlindedMessage, BlindedSignature, Proof};
pub use nut01::{Keys, KeysResponse, PublicKey, SecretKey};
pub use nut02::mint::KeySet as MintKeySet;
pub use nut02::{Id, KeySet, KeySetInfo, KeysetResponse};
pub use nut03::RequestMintResponse;
pub use nut04::{MintRequest, PostMintResponse};
pub use nut05::{CheckFeesRequest, CheckFeesResponse};
#[cfg(not(feature = "nut08"))]
pub use nut05::{MeltRequest, MeltResponse};
#[cfg(feature = "wallet")]
pub use nut06::SplitPayload;
pub use nut06::{SplitRequest, SplitResponse};
#[cfg(feature = "nut07")]
pub use nut07::{CheckSpendableRequest, CheckSpendableResponse};
#[cfg(feature = "nut08")]
pub use nut08::{MeltRequest, MeltResponse};
#[cfg(feature = "nut09")]
pub use nut09::MintInfo;

pub type Proofs = Vec<Proof>;
