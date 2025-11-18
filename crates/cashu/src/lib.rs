#![doc = include_str!("../README.md")]
#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

pub mod amount;
pub mod dhke;
pub mod mint_url;
pub mod nuts;
pub mod secret;
pub mod util;

pub use lightning_invoice::{self, Bolt11Invoice};

pub use self::amount::Amount;
pub use self::mint_url::MintUrl;
pub use self::nuts::*;
pub use self::util::SECP256K1;

pub mod quote_id;

#[doc(hidden)]
#[macro_export]
macro_rules! ensure_cdk {
    ($cond:expr, $err:expr) => {
        if !$cond {
            return Err($err);
        }
    };
}
