//! Util

#[cfg(not(target_arch = "wasm32"))]
use std::time::{SystemTime, UNIX_EPOCH};

use bitcoin::secp256k1::{rand, All, Secp256k1};
#[cfg(target_arch = "wasm32")]
use instant::SystemTime;
use once_cell::sync::Lazy;

pub mod hex;

#[cfg(target_arch = "wasm32")]
const UNIX_EPOCH: SystemTime = SystemTime::UNIX_EPOCH;

/// Secp256k1 global context
pub static SECP256K1: Lazy<Secp256k1<All>> = Lazy::new(|| {
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
