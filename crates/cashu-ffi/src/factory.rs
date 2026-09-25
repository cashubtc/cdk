//! A stateful object that holds a seed so callers do not re-cross the FFI with it.

use std::fmt;
use std::sync::Arc;

use cashu::nuts::nut02::Id;
use cashu::util::wipe::ZeroOnDrop;

use crate::crypto::check_keyset_id;
use crate::error::CashuFfiError;
use crate::outputs::{derive_output, derive_range, parse_seed, restore_amounts};
use crate::types::BlindedOutput;

/// Derives NUT-13 outputs for one keyset from a seed held in Rust.
///
/// Keeping the seed here means a wallet copies it across the FFI once instead of
/// on every derivation.
#[derive(uniffi::Object)]
pub struct DeterministicOutputFactory {
    seed: ZeroOnDrop<[u8; 64]>,
    keyset_id: Id,
    keyset_id_text: String,
}

#[uniffi::export]
impl DeterministicOutputFactory {
    /// Bind a 64 byte BIP39 seed to one keyset.
    #[uniffi::constructor]
    pub fn new(seed: Vec<u8>, keyset_id: String) -> Result<Arc<Self>, CashuFfiError> {
        let seed = ZeroOnDrop::new(seed);
        Ok(Arc::new(Self {
            seed: parse_seed(&seed)?,
            keyset_id: check_keyset_id(&keyset_id)?,
            keyset_id_text: keyset_id,
        }))
    }

    /// The keyset this factory derives for.
    pub fn keyset_id(&self) -> String {
        self.keyset_id_text.clone()
    }

    /// Deterministic outputs, one per denomination, walking `counter` upward.
    pub fn outputs(
        &self,
        amounts: Vec<u64>,
        counter: u32,
    ) -> Result<Vec<BlindedOutput>, CashuFfiError> {
        derive_range(
            &self.keyset_id_text,
            self.keyset_id,
            &self.seed,
            counter,
            &amounts,
        )
    }

    /// A single deterministic output of exactly `amount` at `counter`.
    pub fn single_output(&self, amount: u64, counter: u32) -> Result<BlindedOutput, CashuFfiError> {
        derive_output(
            &self.keyset_id_text,
            self.keyset_id,
            amount,
            &self.seed,
            counter,
        )
    }

    /// NUT-09 restore batch: `count` blank outputs from `start_counter` up.
    pub fn restore_batch(
        &self,
        start_counter: u32,
        count: u32,
    ) -> Result<Vec<BlindedOutput>, CashuFfiError> {
        let amounts = restore_amounts(count)?;
        derive_range(
            &self.keyset_id_text,
            self.keyset_id,
            &self.seed,
            start_counter,
            &amounts,
        )
    }
}

/// Redacts the seed so it cannot reach a log through a derived formatter.
impl fmt::Debug for DeterministicOutputFactory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeterministicOutputFactory")
            .field("keyset_id", &self.keyset_id_text)
            .field("seed", &"<redacted>")
            .finish()
    }
}
