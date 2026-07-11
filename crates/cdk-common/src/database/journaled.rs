//! A journaling decorator for the mint database.
//!
//! [`JournaledDatabase`] wraps any mint database and makes journaling a property
//! of the storage layer rather than a convention at each call site. For every
//! mutation of a journaled entity it appends the matching [`Event`] on the same
//! transaction as the mutation, so the journal row commits or rolls back
//! atomically with the state it records. Because the decorator is the only way
//! to open a transaction, a mutation cannot be left un-journaled by omission.
//!
//! The wrapper lives in `cdk-common` so it is backend-agnostic: one
//! implementation journals for sqlite, postgres, and supabase without
//! duplicating the logic in each backend. Read methods and mutations that do not
//! touch a journaled entity forward verbatim to the inner database.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use cashu::quote_id::QuoteId;

use super::event_log::{Delta, Event};
use super::kvstore::{KVStoreDatabase, KVStoreTransaction};
use super::mint::{
    Acquired, CompletedOperationsDatabase, CompletedOperationsTransaction, Database,
    JournalTransaction, KeysDatabase, KeysDatabaseTransaction, LockedMeltQuotes, MeltRequestInfo,
    ProofsDatabase, ProofsTransaction, QuotesDatabase, QuotesTransaction, SagaDatabase,
    SagaTransaction, SignaturesDatabase, SignaturesTransaction, Transaction,
};
use super::{DbTransactionFinalizer, Error};
use crate::mint::{
    self, MeltQuote, MintKeySetInfo, MintQuote as MintMintQuote, Operation, ProofsWithState,
};
use crate::nut00::ProofsMethods;
use crate::nuts::{
    BlindSignature, BlindedMessage, CurrencyUnit, Id, MeltQuoteState, Proof, Proofs, PublicKey,
    State,
};
use crate::payment::PaymentIdentifier;

/// Journaling decorator over an inner mint database `D`.
///
/// Implements [`Database`] when `D: Database` and [`KeysDatabase`] when
/// `D: KeysDatabase`, wrapping each opened transaction so mutations of journaled
/// entities are recorded automatically.
///
/// `D` is `?Sized`, so the wrapper accepts either a concrete database or an
/// existing trait object. Wrapping `Arc<dyn Database<Error> + Send + Sync>` (the
/// `DynMintDatabase` handle the mint already threads around) lets journaling be
/// installed at a single construction choke point rather than at every backend.
pub struct JournaledDatabase<D: ?Sized> {
    inner: Arc<D>,
}

impl<D: ?Sized> JournaledDatabase<D> {
    /// Wraps `inner` so every transaction it opens auto-journals mutations.
    pub fn new(inner: Arc<D>) -> Self {
        Self { inner }
    }
}

impl<D: ?Sized> Clone for JournaledDatabase<D> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<D: ?Sized> std::fmt::Debug for JournaledDatabase<D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JournaledDatabase").finish_non_exhaustive()
    }
}

/// A transaction wrapper that journals mint-entity mutations.
///
/// Holds the inner transaction as a trait object; the journal append runs on the
/// same inner transaction, so it shares its atomicity.
struct JournaledTransaction {
    inner: Box<dyn Transaction<Error> + Send + Sync>,
}

#[async_trait]
impl DbTransactionFinalizer for JournaledTransaction {
    type Err = Error;

    async fn commit(self: Box<Self>) -> Result<(), Error> {
        self.inner.commit().await
    }

    async fn rollback(self: Box<Self>) -> Result<(), Error> {
        self.inner.rollback().await
    }
}

#[async_trait]
impl JournalTransaction for JournaledTransaction {
    type Err = Error;

    /// Rejected. Journaling is driven by the entity mutations, which call the
    /// inner transaction's `add_journal` directly. A direct call here comes from
    /// outside the decorator and would produce an unmanaged, possibly duplicate
    /// journal row, so it is not permitted.
    async fn add_journal(&mut self, _record: String, _event: Event) -> Result<(), Error> {
        Err(Error::JournalNotPermitted)
    }
}

#[async_trait]
impl QuotesTransaction for JournaledTransaction {
    type Err = Error;

    async fn add_melt_request(
        &mut self,
        quote_id: &QuoteId,
        inputs_amount: cashu::Amount<CurrencyUnit>,
        inputs_fee: cashu::Amount<CurrencyUnit>,
    ) -> Result<(), Error> {
        self.inner
            .add_melt_request(quote_id, inputs_amount, inputs_fee)
            .await
    }

    async fn add_blinded_messages(
        &mut self,
        quote_id: Option<&QuoteId>,
        blinded_messages: &[BlindedMessage],
        operation: &Operation,
    ) -> Result<(), Error> {
        self.inner
            .add_blinded_messages(quote_id, blinded_messages, operation)
            .await
    }

    async fn delete_blinded_messages(
        &mut self,
        blinded_secrets: &[PublicKey],
    ) -> Result<(), Error> {
        self.inner.delete_blinded_messages(blinded_secrets).await
    }

    async fn get_melt_request_and_blinded_messages(
        &mut self,
        quote_id: &QuoteId,
    ) -> Result<Option<MeltRequestInfo>, Error> {
        self.inner
            .get_melt_request_and_blinded_messages(quote_id)
            .await
    }

    async fn delete_melt_request(&mut self, quote_id: &QuoteId) -> Result<(), Error> {
        self.inner.delete_melt_request(quote_id).await
    }

    async fn get_mint_quote(
        &mut self,
        quote_id: &QuoteId,
    ) -> Result<Option<Acquired<MintMintQuote>>, Error> {
        self.inner.get_mint_quote(quote_id).await
    }

    async fn get_mint_quotes_by_ids(
        &mut self,
        quote_ids: &[QuoteId],
    ) -> Result<Vec<Option<Acquired<MintMintQuote>>>, Error> {
        self.inner.get_mint_quotes_by_ids(quote_ids).await
    }

    async fn add_mint_quote(
        &mut self,
        quote: MintMintQuote,
    ) -> Result<Acquired<MintMintQuote>, Error> {
        let acquired = self.inner.add_mint_quote(quote).await?;
        self.inner
            .add_journal(acquired.id.to_string(), (*acquired).clone().into())
            .await?;
        Ok(acquired)
    }

    async fn update_mint_quote(
        &mut self,
        quote: &mut Acquired<mint::MintQuote>,
    ) -> Result<(), Error> {
        let record = quote.id.to_string();
        // Peek the change buffer before delegating; `update_mint_quote` drains
        // it via `take_changes`, so it must be read first, then journaled from a
        // clone once the persist succeeds.
        let (payments, issuances) = match quote.pending_changes() {
            Some(changes) => (
                changes.payments.clone().unwrap_or_default(),
                changes.issuances.clone().unwrap_or_default(),
            ),
            None => (Vec::new(), Vec::new()),
        };
        self.inner.update_mint_quote(quote).await?;
        for payment in payments {
            self.inner
                .add_journal(record.clone(), Delta::MintQuotePayment(payment).into())
                .await?;
        }
        for amount in issuances {
            self.inner
                .add_journal(record.clone(), Delta::MintQuoteIssuance(amount).into())
                .await?;
        }
        Ok(())
    }

    async fn get_melt_quote(
        &mut self,
        quote_id: &QuoteId,
    ) -> Result<Option<Acquired<mint::MeltQuote>>, Error> {
        self.inner.get_melt_quote(quote_id).await
    }

    async fn add_melt_quote(&mut self, quote: mint::MeltQuote) -> Result<(), Error> {
        let record = quote.id.to_string();
        let event: Event = quote.clone().into();
        self.inner.add_melt_quote(quote).await?;
        self.inner.add_journal(record, event).await?;
        Ok(())
    }

    async fn get_melt_quotes_by_request_lookup_id(
        &mut self,
        request_lookup_id: &PaymentIdentifier,
    ) -> Result<Vec<Acquired<MeltQuote>>, Error> {
        self.inner
            .get_melt_quotes_by_request_lookup_id(request_lookup_id)
            .await
    }

    async fn lock_melt_quote_and_related(
        &mut self,
        quote_id: &QuoteId,
    ) -> Result<LockedMeltQuotes, Error> {
        self.inner.lock_melt_quote_and_related(quote_id).await
    }

    async fn update_melt_quote_request_lookup_id(
        &mut self,
        quote: &mut Acquired<mint::MeltQuote>,
        new_request_lookup_id: &PaymentIdentifier,
    ) -> Result<(), Error> {
        let record = quote.id.to_string();
        self.inner
            .update_melt_quote_request_lookup_id(quote, new_request_lookup_id)
            .await?;
        self.inner
            .add_journal(record, new_request_lookup_id.clone().into())
            .await?;
        Ok(())
    }

    async fn update_melt_quote_state(
        &mut self,
        quote: &mut Acquired<mint::MeltQuote>,
        new_state: MeltQuoteState,
        payment_proof: Option<String>,
    ) -> Result<MeltQuoteState, Error> {
        let record = quote.id.to_string();
        let previous = self
            .inner
            .update_melt_quote_state(quote, new_state, payment_proof.clone())
            .await?;
        self.inner
            .add_journal(record.clone(), new_state.into())
            .await?;
        if payment_proof.is_some() {
            self.inner
                .add_journal(record, Delta::MeltQuotePaymentProof(payment_proof).into())
                .await?;
        }
        Ok(previous)
    }

    async fn get_mint_quote_by_request(
        &mut self,
        request: &str,
    ) -> Result<Option<Acquired<MintMintQuote>>, Error> {
        self.inner.get_mint_quote_by_request(request).await
    }

    async fn get_mint_quote_by_request_lookup_id(
        &mut self,
        request_lookup_id: &PaymentIdentifier,
    ) -> Result<Option<Acquired<MintMintQuote>>, Error> {
        self.inner
            .get_mint_quote_by_request_lookup_id(request_lookup_id)
            .await
    }
}

#[async_trait]
impl SignaturesTransaction for JournaledTransaction {
    type Err = Error;

    async fn add_blind_signatures(
        &mut self,
        blinded_messages: &[PublicKey],
        blind_signatures: &[BlindSignature],
        quote_id: Option<QuoteId>,
    ) -> Result<(), Error> {
        self.inner
            .add_blind_signatures(blinded_messages, blind_signatures, quote_id)
            .await?;
        for (secret, signature) in blinded_messages.iter().zip(blind_signatures.iter()) {
            self.inner
                .add_journal(secret.to_hex(), signature.clone().into())
                .await?;
        }
        Ok(())
    }

    async fn get_blind_signatures(
        &mut self,
        blinded_messages: &[PublicKey],
    ) -> Result<Vec<Option<BlindSignature>>, Error> {
        self.inner.get_blind_signatures(blinded_messages).await
    }
}

#[async_trait]
impl ProofsTransaction for JournaledTransaction {
    type Err = Error;

    async fn add_proofs(
        &mut self,
        proof: Proofs,
        quote_id: Option<QuoteId>,
        operation: &Operation,
    ) -> Result<Acquired<ProofsWithState>, Error> {
        let mut events = Vec::with_capacity(proof.len());
        for p in &proof {
            events.push((p.y()?.to_hex(), p.clone().into()));
        }
        let acquired = self.inner.add_proofs(proof, quote_id, operation).await?;
        for (record, event) in events {
            self.inner.add_journal(record, event).await?;
        }
        Ok(acquired)
    }

    async fn update_proofs_state(
        &mut self,
        proofs: &mut Acquired<ProofsWithState>,
        new_state: State,
    ) -> Result<(), Error> {
        self.inner.update_proofs_state(proofs, new_state).await?;
        for y in proofs.ys()? {
            self.inner.add_journal(y.to_hex(), new_state.into()).await?;
        }
        Ok(())
    }

    async fn get_proofs(&mut self, ys: &[PublicKey]) -> Result<Acquired<ProofsWithState>, Error> {
        self.inner.get_proofs(ys).await
    }

    async fn remove_proofs(
        &mut self,
        ys: &[PublicKey],
        quote_id: Option<QuoteId>,
    ) -> Result<(), Error> {
        self.inner.remove_proofs(ys, quote_id).await?;
        for y in ys {
            self.inner
                .add_journal(y.to_hex(), Delta::ProofRemoved.into())
                .await?;
        }
        Ok(())
    }

    async fn get_proof_ys_by_quote_id(
        &mut self,
        quote_id: &QuoteId,
    ) -> Result<Vec<PublicKey>, Error> {
        self.inner.get_proof_ys_by_quote_id(quote_id).await
    }

    async fn get_proof_ys_by_operation_id(
        &mut self,
        operation_id: &uuid::Uuid,
    ) -> Result<Vec<PublicKey>, Error> {
        self.inner.get_proof_ys_by_operation_id(operation_id).await
    }
}

#[async_trait]
impl KVStoreTransaction<Error> for JournaledTransaction {
    async fn kv_read(
        &mut self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<Option<Vec<u8>>, Error> {
        self.inner
            .kv_read(primary_namespace, secondary_namespace, key)
            .await
    }

    async fn kv_write(
        &mut self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
        value: &[u8],
    ) -> Result<(), Error> {
        self.inner
            .kv_write(primary_namespace, secondary_namespace, key, value)
            .await
    }

    async fn kv_remove(
        &mut self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<(), Error> {
        self.inner
            .kv_remove(primary_namespace, secondary_namespace, key)
            .await
    }

    async fn kv_list(
        &mut self,
        primary_namespace: &str,
        secondary_namespace: &str,
    ) -> Result<Vec<String>, Error> {
        self.inner
            .kv_list(primary_namespace, secondary_namespace)
            .await
    }
}

#[async_trait]
impl SagaTransaction for JournaledTransaction {
    type Err = Error;

    async fn get_saga(&mut self, operation_id: &uuid::Uuid) -> Result<Option<mint::Saga>, Error> {
        self.inner.get_saga(operation_id).await
    }

    async fn add_saga(&mut self, saga: &mint::Saga) -> Result<(), Error> {
        self.inner.add_saga(saga).await
    }

    async fn update_saga(
        &mut self,
        operation_id: &uuid::Uuid,
        new_state: mint::SagaStateEnum,
    ) -> Result<(), Error> {
        self.inner.update_saga(operation_id, new_state).await
    }

    async fn update_saga_with_finalization_data(
        &mut self,
        operation_id: &uuid::Uuid,
        new_state: mint::SagaStateEnum,
        finalization_data: Option<&mint::MeltFinalizationData>,
    ) -> Result<(), Error> {
        self.inner
            .update_saga_with_finalization_data(operation_id, new_state, finalization_data)
            .await
    }

    async fn delete_saga(&mut self, operation_id: &uuid::Uuid) -> Result<(), Error> {
        self.inner.delete_saga(operation_id).await
    }
}

#[async_trait]
impl CompletedOperationsTransaction for JournaledTransaction {
    type Err = Error;

    async fn add_completed_operation(
        &mut self,
        operation: &mint::Operation,
        fee_by_keyset: &std::collections::HashMap<crate::nuts::Id, crate::Amount>,
    ) -> Result<(), Error> {
        self.inner
            .add_completed_operation(operation, fee_by_keyset)
            .await
    }
}

impl Transaction<Error> for JournaledTransaction {}

#[async_trait]
impl<D> KVStoreDatabase for JournaledDatabase<D>
where
    D: Database<Error> + Send + Sync + ?Sized,
{
    type Err = Error;

    async fn kv_read(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<Option<Vec<u8>>, Error> {
        self.inner
            .kv_read(primary_namespace, secondary_namespace, key)
            .await
    }

    async fn kv_list(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
    ) -> Result<Vec<String>, Error> {
        self.inner
            .kv_list(primary_namespace, secondary_namespace)
            .await
    }
}

#[async_trait]
impl<D> QuotesDatabase for JournaledDatabase<D>
where
    D: Database<Error> + Send + Sync + ?Sized,
{
    type Err = Error;

    async fn get_mint_quote(&self, quote_id: &QuoteId) -> Result<Option<MintMintQuote>, Error> {
        self.inner.get_mint_quote(quote_id).await
    }

    async fn get_mint_quotes_by_ids(
        &self,
        quote_ids: &[QuoteId],
    ) -> Result<Vec<Option<MintMintQuote>>, Error> {
        self.inner.get_mint_quotes_by_ids(quote_ids).await
    }

    async fn get_mint_quote_by_request(
        &self,
        request: &str,
    ) -> Result<Option<MintMintQuote>, Error> {
        self.inner.get_mint_quote_by_request(request).await
    }

    async fn get_mint_quote_by_request_lookup_id(
        &self,
        request_lookup_id: &PaymentIdentifier,
    ) -> Result<Option<MintMintQuote>, Error> {
        self.inner
            .get_mint_quote_by_request_lookup_id(request_lookup_id)
            .await
    }

    async fn get_mint_quotes(&self) -> Result<Vec<MintMintQuote>, Error> {
        self.inner.get_mint_quotes().await
    }

    async fn get_melt_quote(&self, quote_id: &QuoteId) -> Result<Option<mint::MeltQuote>, Error> {
        self.inner.get_melt_quote(quote_id).await
    }

    async fn get_melt_quotes(&self) -> Result<Vec<mint::MeltQuote>, Error> {
        self.inner.get_melt_quotes().await
    }
}

#[async_trait]
impl<D> ProofsDatabase for JournaledDatabase<D>
where
    D: Database<Error> + Send + Sync + ?Sized,
{
    type Err = Error;

    async fn get_proofs_by_ys(&self, ys: &[PublicKey]) -> Result<Vec<Option<Proof>>, Error> {
        self.inner.get_proofs_by_ys(ys).await
    }

    async fn get_proof_ys_by_quote_id(&self, quote_id: &QuoteId) -> Result<Vec<PublicKey>, Error> {
        self.inner.get_proof_ys_by_quote_id(quote_id).await
    }

    async fn get_proofs_states(&self, ys: &[PublicKey]) -> Result<Vec<Option<State>>, Error> {
        self.inner.get_proofs_states(ys).await
    }

    async fn get_proofs_by_keyset_id(
        &self,
        keyset_id: &Id,
    ) -> Result<(Proofs, Vec<Option<State>>), Error> {
        self.inner.get_proofs_by_keyset_id(keyset_id).await
    }

    async fn get_total_redeemed(&self) -> Result<HashMap<Id, crate::Amount>, Error> {
        self.inner.get_total_redeemed().await
    }

    async fn get_proof_ys_by_operation_id(
        &self,
        operation_id: &uuid::Uuid,
    ) -> Result<Vec<PublicKey>, Error> {
        self.inner.get_proof_ys_by_operation_id(operation_id).await
    }
}

#[async_trait]
impl<D> SignaturesDatabase for JournaledDatabase<D>
where
    D: Database<Error> + Send + Sync + ?Sized,
{
    type Err = Error;

    async fn get_blind_signatures(
        &self,
        blinded_messages: &[PublicKey],
    ) -> Result<Vec<Option<BlindSignature>>, Error> {
        self.inner.get_blind_signatures(blinded_messages).await
    }

    async fn get_blind_signatures_for_keyset(
        &self,
        keyset_id: &Id,
    ) -> Result<Vec<BlindSignature>, Error> {
        self.inner.get_blind_signatures_for_keyset(keyset_id).await
    }

    async fn get_blind_signatures_for_quote(
        &self,
        quote_id: &QuoteId,
    ) -> Result<Vec<BlindSignature>, Error> {
        self.inner.get_blind_signatures_for_quote(quote_id).await
    }

    async fn get_total_issued(&self) -> Result<HashMap<Id, crate::Amount>, Error> {
        self.inner.get_total_issued().await
    }

    async fn get_blinded_secrets_by_operation_id(
        &self,
        operation_id: &uuid::Uuid,
    ) -> Result<Vec<PublicKey>, Error> {
        self.inner
            .get_blinded_secrets_by_operation_id(operation_id)
            .await
    }
}

#[async_trait]
impl<D> SagaDatabase for JournaledDatabase<D>
where
    D: Database<Error> + Send + Sync + ?Sized,
{
    type Err = Error;

    async fn get_melt_saga_by_quote_id(
        &self,
        quote_id: &QuoteId,
    ) -> Result<Option<mint::Saga>, Error> {
        self.inner.get_melt_saga_by_quote_id(quote_id).await
    }

    async fn get_incomplete_sagas(
        &self,
        operation_kind: mint::OperationKind,
    ) -> Result<Vec<mint::Saga>, Error> {
        self.inner.get_incomplete_sagas(operation_kind).await
    }
}

#[async_trait]
impl<D> CompletedOperationsDatabase for JournaledDatabase<D>
where
    D: Database<Error> + Send + Sync + ?Sized,
{
    type Err = Error;

    async fn get_completed_operation(
        &self,
        operation_id: &uuid::Uuid,
    ) -> Result<Option<mint::Operation>, Error> {
        self.inner.get_completed_operation(operation_id).await
    }

    async fn get_completed_operations_by_kind(
        &self,
        operation_kind: mint::OperationKind,
    ) -> Result<Vec<mint::Operation>, Error> {
        self.inner
            .get_completed_operations_by_kind(operation_kind)
            .await
    }

    async fn get_completed_operations(&self) -> Result<Vec<mint::Operation>, Error> {
        self.inner.get_completed_operations().await
    }
}

#[async_trait]
impl<D> Database<Error> for JournaledDatabase<D>
where
    D: Database<Error> + Send + Sync + ?Sized + 'static,
{
    async fn begin_transaction(&self) -> Result<Box<dyn Transaction<Error> + Send + Sync>, Error> {
        let inner = self.inner.begin_transaction().await?;
        Ok(Box::new(JournaledTransaction { inner }))
    }
}

/// A keyset transaction wrapper that journals keyset creation and activation.
///
/// Captures the set of currently-active keysets when the transaction opens, so
/// [`set_active_keyset`](KeysDatabaseTransaction::set_active_keyset) can journal
/// the deactivation of the superseded keyset without an out-of-band read.
struct JournaledKeysTransaction<'a> {
    inner: Box<dyn KeysDatabaseTransaction<'a, Error> + Send + Sync + 'a>,
    active: HashMap<CurrencyUnit, Id>,
}

#[async_trait]
impl DbTransactionFinalizer for JournaledKeysTransaction<'_> {
    type Err = Error;

    async fn commit(self: Box<Self>) -> Result<(), Error> {
        self.inner.commit().await
    }

    async fn rollback(self: Box<Self>) -> Result<(), Error> {
        self.inner.rollback().await
    }
}

#[async_trait]
impl JournalTransaction for JournaledKeysTransaction<'_> {
    type Err = Error;

    /// Rejected, for the same reason as [`JournaledTransaction::add_journal`]:
    /// keyset mutations journal themselves, so a direct call is not permitted.
    async fn add_journal(&mut self, _record: String, _event: Event) -> Result<(), Error> {
        Err(Error::JournalNotPermitted)
    }
}

#[async_trait]
impl<'a> KeysDatabaseTransaction<'a, Error> for JournaledKeysTransaction<'a> {
    async fn set_active_keyset(&mut self, unit: CurrencyUnit, id: Id) -> Result<(), Error> {
        self.inner.set_active_keyset(unit.clone(), id).await?;
        if let Some(previous) = self.active.get(&unit).copied() {
            if previous != id {
                self.inner
                    .add_journal(previous.to_string(), Delta::KeysetActive(false).into())
                    .await?;
            }
        }
        self.inner
            .add_journal(id.to_string(), Delta::KeysetActive(true).into())
            .await?;
        self.active.insert(unit, id);
        Ok(())
    }

    async fn add_keyset_info(&mut self, keyset: MintKeySetInfo) -> Result<(), Error> {
        let record = keyset.id.to_string();
        let event: Event = keyset.clone().into();
        self.inner.add_keyset_info(keyset).await?;
        self.inner.add_journal(record, event).await?;
        Ok(())
    }
}

#[async_trait]
impl<D> KeysDatabase for JournaledDatabase<D>
where
    D: KeysDatabase<Err = Error> + Send + Sync + ?Sized + 'static,
{
    type Err = Error;

    async fn begin_transaction<'a>(
        &'a self,
    ) -> Result<Box<dyn KeysDatabaseTransaction<'a, Self::Err> + Send + Sync + 'a>, Error> {
        let active = self.inner.get_active_keysets().await?;
        let inner = self.inner.begin_transaction().await?;
        Ok(Box::new(JournaledKeysTransaction { inner, active }))
    }

    async fn get_active_keyset_id(&self, unit: &CurrencyUnit) -> Result<Option<Id>, Error> {
        self.inner.get_active_keyset_id(unit).await
    }

    async fn get_active_keysets(&self) -> Result<HashMap<CurrencyUnit, Id>, Error> {
        self.inner.get_active_keysets().await
    }

    async fn get_keyset_info(&self, id: &Id) -> Result<Option<MintKeySetInfo>, Error> {
        self.inner.get_keyset_info(id).await
    }

    async fn get_keyset_infos(&self) -> Result<Vec<MintKeySetInfo>, Error> {
        self.inner.get_keyset_infos().await
    }
}
