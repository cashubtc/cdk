//! Atomic wallet operation preparation.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::Error;
use crate::wallet::{
    IssueSagaState, MeltSagaState, OperationData, ProofInfo, WalletSaga, WalletSagaState,
};
use crate::{Id, PublicKey, State};

/// A quote snapshot that preparation must acquire without intervening updates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuoteReservation {
    /// Quote identifier.
    pub quote_id: String,
    /// Expected persisted quote version.
    pub version: u32,
}

/// A deterministic output range committed together with its recovery record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CounterReservation {
    /// Output keyset.
    pub keyset_id: Id,
    /// Expected counter value, inclusive start of the range.
    pub start: u32,
    /// New counter value, exclusive end of the range.
    pub end: u32,
}

/// All local changes belonging to a mint, melt or swap preparation transition.
///
/// Backends must commit this entire value in one transaction. Quote versions,
/// unowned/unspent inputs, counter expectations, and a fresh saga ID are checked
/// inside that transaction. A conflict aborts every change; callers may rebuild
/// the preparation from fresh snapshots. This operation sends no remote request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationPreparation {
    /// Complete recovery record. Version zero inserts an initial operation;
    /// a later version atomically advances an owned, prepared melt to requested.
    pub saga: WalletSaga,
    /// Existing wallet inputs to reserve or transfer from a nested swap's parent.
    pub proof_ys: Vec<PublicKey>,
    /// Explicit inputs to import or reserve, including their signed witnesses.
    /// Existing inputs must be unspent and unowned and retain their provenance.
    pub proofs: Vec<ProofInfo>,
    /// Every quote named by the recovery record, in the same order.
    pub quotes: Vec<QuoteReservation>,
    /// Optional deterministic output allocation. Random outputs need no counter.
    pub counter: Option<CounterReservation>,
}

impl OperationPreparation {
    /// Validate the request independently of backend state.
    pub fn validate(&self) -> Result<(), Error> {
        let invalid = || Error::Internal("Invalid operation preparation".to_owned());
        if self.saga.kind != self.saga.state.kind() || self.saga.kind != self.saga.data.kind() {
            return Err(invalid());
        }
        let ids = match (&self.saga.state, &self.saga.data) {
            (WalletSagaState::Melt(MeltSagaState::ProofsReserved), OperationData::Melt(data)) => {
                if self.saga.version != 0 || self.counter.is_some() {
                    return Err(invalid());
                }
                vec![data.quote_id.clone()]
            }
            (WalletSagaState::Melt(MeltSagaState::MeltRequested), OperationData::Melt(data)) => {
                if self.saga.version == 0
                    || !self.proofs.is_empty()
                    || data.final_proof_ys.as_ref() != Some(&self.proof_ys)
                {
                    return Err(invalid());
                }
                if let Some(counter) = &self.counter {
                    if counter.end < counter.start
                        || data.counter_start != Some(counter.start)
                        || data.counter_end != Some(counter.end)
                        || data.change_blinded_messages.as_ref().is_none_or(|outputs| {
                            outputs.len() != (counter.end - counter.start) as usize
                                || outputs
                                    .iter()
                                    .any(|output| output.keyset_id != counter.keyset_id)
                        })
                    {
                        return Err(invalid());
                    }
                }
                vec![data.quote_id.clone()]
            }
            (
                WalletSagaState::Issue(IssueSagaState::SecretsPrepared),
                OperationData::Mint(data),
            ) => {
                if self.saga.version != 0 || !self.proof_ys.is_empty() || !self.proofs.is_empty() {
                    return Err(invalid());
                }
                if let Some(counter) = &self.counter {
                    if counter.end < counter.start
                        || data.counter_start != Some(counter.start)
                        || data.counter_end != Some(counter.end)
                        || data.blinded_messages.as_ref().is_none_or(|outputs| {
                            outputs.len() != (counter.end - counter.start) as usize
                                || outputs
                                    .iter()
                                    .any(|output| output.keyset_id != counter.keyset_id)
                        })
                    {
                        return Err(invalid());
                    }
                }
                data.quote_ids()
            }
            (
                WalletSagaState::Swap(crate::wallet::SwapSagaState::ProofsReserved),
                OperationData::Swap(data),
            ) => {
                if self.saga.version != 0 || !self.proofs.is_empty() {
                    return Err(invalid());
                }
                if let Some(counter) = &self.counter {
                    if counter.end < counter.start
                        || data.counter_start != Some(counter.start)
                        || data.counter_end != Some(counter.end)
                    {
                        return Err(invalid());
                    }
                }
                vec![]
            }
            _ => return Err(invalid()),
        };
        if self.quotes.iter().any(|quote| quote.version == u32::MAX)
            || (ids.is_empty() && !matches!(self.saga.data, OperationData::Swap(_)))
            || ids.iter().collect::<HashSet<_>>().len() != ids.len()
            || ids
                .iter()
                .ne(self.quotes.iter().map(|quote| &quote.quote_id))
            || self.saga.quote_id.as_ref() != ids.first()
        {
            return Err(invalid());
        }
        let mut ys = HashSet::new();
        for y in &self.proof_ys {
            if !ys.insert(*y) {
                return Err(invalid());
            }
        }
        for proof in &self.proofs {
            if !ys.insert(proof.y)
                || proof.proof.y()? != proof.y
                || proof.mint_url != self.saga.mint_url
                || proof.unit != self.saga.unit
            {
                return Err(invalid());
            }
        }
        Ok(())
    }

    /// Check an input and attach this operation's reservation while retaining
    /// persisted provenance and derivation metadata.
    pub fn reserve_input(
        &self,
        mut proof: ProofInfo,
        imported: Option<&ProofInfo>,
    ) -> Result<ProofInfo, Error> {
        let initial = self.saga.version == 0;
        let parent = match &self.saga.data {
            OperationData::Swap(data) => data.parent.as_ref(),
            _ => None,
        };
        let available = if let Some(parent) = parent {
            proof.state == State::Reserved && proof.used_by_operation == Some(parent.id)
        } else if initial {
            proof.state == State::Unspent && proof.used_by_operation.is_none()
        } else {
            proof.state == State::Reserved && proof.used_by_operation == Some(self.saga.id)
        };
        if !available || proof.mint_url != self.saga.mint_url || proof.unit != self.saga.unit {
            return Err(Error::ProofNotUnspent);
        }
        if let Some(imported) = imported {
            if proof.proof.secret != imported.proof.secret
                || proof.proof.amount != imported.proof.amount
                || proof.proof.keyset_id != imported.proof.keyset_id
                || proof.proof.c != imported.proof.c
            {
                return Err(Error::ProofNotUnspent);
            }
            proof.proof.witness = imported.proof.witness.clone();
        }
        proof.state = if initial {
            State::Reserved
        } else {
            State::Pending
        };
        proof.used_by_operation = Some(self.saga.id);
        Ok(proof)
    }
}
