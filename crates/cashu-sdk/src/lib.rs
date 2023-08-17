#[cfg(feature = "blocking")]
use once_cell::sync::Lazy;
#[cfg(feature = "blocking")]
use tokio::runtime::Runtime;

#[cfg(feature = "blocking")]
use futures_util::Future;

#[cfg(feature = "wallet")]
pub(crate) mod client;

#[cfg(feature = "mint")]
pub mod mint;
#[cfg(feature = "wallet")]
pub mod wallet;

pub use cashu::{self, *};

#[cfg(all(feature = "blocking", feature = "wallet"))]
use self::client::blocking;

#[cfg(feature = "blocking")]
static RUNTIME: Lazy<Runtime> = Lazy::new(|| Runtime::new().expect("Can't start Tokio runtime"));

#[cfg(feature = "blocking")]
pub fn block_on<F: Future>(future: F) -> F::Output {
    RUNTIME.block_on(future)
}
