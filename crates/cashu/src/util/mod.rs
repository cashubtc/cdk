//! Cashu utils

pub mod hex;
pub mod serde_helpers;
pub mod tagged_hash;

use std::sync::LazyLock;

use bitcoin::secp256k1::{rand, All, Secp256k1};
use web_time::{SystemTime, UNIX_EPOCH};

/// Secp256k1 global context
pub static SECP256K1: LazyLock<Secp256k1<All>> = LazyLock::new(|| {
    let mut ctx = Secp256k1::new();
    let mut rng = rand::thread_rng();
    ctx.randomize(&mut rng);
    ctx
});

/// Seconds since unix epoch
pub fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
