//! Util

#[cfg(not(target_arch = "wasm32"))]
use std::time::{SystemTime, UNIX_EPOCH};

use bitcoin::hashes::sha256::Hash as Sha256;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::rand::{self, RngCore};
use bitcoin::secp256k1::{All, Secp256k1};
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

pub fn random_hash() -> Vec<u8> {
    let mut rng = rand::thread_rng();
    let mut random_bytes: [u8; 32] = [0u8; Sha256::LEN];
    rng.fill_bytes(&mut random_bytes);

    let hash = Sha256::hash(&random_bytes);
    hash.to_byte_array().to_vec()
}

pub fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
