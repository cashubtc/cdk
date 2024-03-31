//! Utils

use std::time::SystemTime;

use bitcoin::hashes::sha256::Hash as Sha256;
use bitcoin::hashes::Hash;
use rand::prelude::*;

pub fn random_hash() -> Vec<u8> {
    let mut rng = rand::thread_rng();
    let mut random_bytes = [0u8; Sha256::LEN];
    rng.fill_bytes(&mut random_bytes);
    let hash = Sha256::hash(&random_bytes);
    hash.to_byte_array().to_vec()
}

pub fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|x| x.as_secs())
        .unwrap_or(0)
}
