//! CDK Database

use std::collections::HashMap;

use async_trait::async_trait;
use cashu::kvac::{KvacIssuedMac, KvacNullifier};
use cashu_kvac::secp::{GroupElement, Scalar};
use uuid::Uuid;

use super::Error;
use crate::common::LnKey;
use crate::mint::{self, MintKeySetInfo, MintQuote as MintMintQuote};
use crate::nuts::{
    BlindSignature, CurrencyUnit, Id, MeltBolt11Request, MeltQuoteState, MintQuoteState, Proof,
    Proofs, PublicKey, State,
};

/// Mint Database trait
#[async_trait]
pub trait Database {
    /// Mint Database Error
    type Err: Into<Error> + From<Error>;

    /// Add Active Keyset
    async fn set_active_keyset(&self, unit: CurrencyUnit, id: Id) -> Result<(), Self::Err>;
    /// Add Active KVAC Keyset
    async fn set_active_kvac_keyset(&self, _unit: CurrencyUnit, _id: Id) -> Result<(), Self::Err> {
        Err(Self::Err::from(Error::Unimplemented))
    }
    /// Get Active Keyset
    async fn get_active_keyset_id(&self, unit: &CurrencyUnit) -> Result<Option<Id>, Self::Err>;
    /// Get Active KVAC Keyset
    async fn get_active_kvac_keyset_id(&self, _unit: &CurrencyUnit) -> Result<Option<Id>, Self::Err> {
        Err(Self::Err::from(Error::Unimplemented))
    }
    /// Get all Active Keyset
    async fn get_active_keysets(&self) -> Result<HashMap<CurrencyUnit, Id>, Self::Err>;
    /// Get all Active KVAC Keysets
    async fn get_active_kvac_keysets(&self) -> Result<HashMap<CurrencyUnit, Id>, Self::Err> {
        Err(Self::Err::from(Error::Unimplemented))
    }

    /// Add [`MintMintQuote`]
    async fn add_mint_quote(&self, quote: MintMintQuote) -> Result<(), Self::Err>;
    /// Get [`MintMintQuote`]
    async fn get_mint_quote(&self, quote_id: &Uuid) -> Result<Option<MintMintQuote>, Self::Err>;
    /// Update state of [`MintMintQuote`]
    async fn update_mint_quote_state(
        &self,
        quote_id: &Uuid,
        state: MintQuoteState,
    ) -> Result<MintQuoteState, Self::Err>;
    /// Get all [`MintMintQuote`]s
    async fn get_mint_quote_by_request(
        &self,
        request: &str,
    ) -> Result<Option<MintMintQuote>, Self::Err>;
    /// Get all [`MintMintQuote`]s
    async fn get_mint_quote_by_request_lookup_id(
        &self,
        request_lookup_id: &str,
    ) -> Result<Option<MintMintQuote>, Self::Err>;
    /// Get Mint Quotes
    async fn get_mint_quotes(&self) -> Result<Vec<MintMintQuote>, Self::Err>;
    /// Remove [`MintMintQuote`]
    async fn remove_mint_quote(&self, quote_id: &Uuid) -> Result<(), Self::Err>;

    /// Add [`mint::MeltQuote`]
    async fn add_melt_quote(&self, quote: mint::MeltQuote) -> Result<(), Self::Err>;
    /// Get [`mint::MeltQuote`]
    async fn get_melt_quote(&self, quote_id: &Uuid) -> Result<Option<mint::MeltQuote>, Self::Err>;
    /// Update [`mint::MeltQuote`] state
    async fn update_melt_quote_state(
        &self,
        quote_id: &Uuid,
        state: MeltQuoteState,
    ) -> Result<MeltQuoteState, Self::Err>;
    /// Get all [`mint::MeltQuote`]s
    async fn get_melt_quotes(&self) -> Result<Vec<mint::MeltQuote>, Self::Err>;
    /// Remove [`mint::MeltQuote`]
    async fn remove_melt_quote(&self, quote_id: &Uuid) -> Result<(), Self::Err>;

    /// Add melt request
    async fn add_melt_request(
        &self,
        melt_request: MeltBolt11Request<Uuid>,
        ln_key: LnKey,
    ) -> Result<(), Self::Err>;
    /// Get melt request
    async fn get_melt_request(
        &self,
        quote_id: &Uuid,
    ) -> Result<Option<(MeltBolt11Request<Uuid>, LnKey)>, Self::Err>;

    /// Add [`MintKeySetInfo`]
    async fn add_keyset_info(&self, keyset: MintKeySetInfo) -> Result<(), Self::Err>;
    /// Add KVAC [`MintKeySetInfo`]
    async fn add_kvac_keyset_info(&self, _keyset: MintKeySetInfo) -> Result<(), Self::Err> {
        Err(Self::Err::from(Error::Unimplemented))
    }
    /// Get [`MintKeySetInfo`]
    async fn get_keyset_info(&self, id: &Id) -> Result<Option<MintKeySetInfo>, Self::Err>;
    /// Get KVAC [`MintKeySetInfo`]
    async fn get_kvac_keyset_info(&self, _id: &Id) -> Result<Option<MintKeySetInfo>, Self::Err> {
        Err(Self::Err::from(Error::Unimplemented))
    }
    /// Get [`MintKeySetInfo`]s
    async fn get_keyset_infos(&self) -> Result<Vec<MintKeySetInfo>, Self::Err>;
    /// Get KVAC [`MintKeySetInfo`]s
    async fn get_kvac_keyset_infos(&self) -> Result<Vec<MintKeySetInfo>, Self::Err> {
        Err(Self::Err::from(Error::Unimplemented))
    }

    /// Add spent [`Proofs`]
    async fn add_proofs(&self, proof: Proofs, quote_id: Option<Uuid>) -> Result<(), Self::Err>;
    /// Add kvac nullifiers
    async fn add_kvac_nullifiers(&self, _nullifiers: &[KvacNullifier]) -> Result<(), Self::Err> {
        Err(Self::Err::from(Error::Unimplemented))
    }
    /// Get [`Proofs`] by ys
    async fn get_proofs_by_ys(&self, ys: &[PublicKey]) -> Result<Vec<Option<Proof>>, Self::Err>;
    /// Get kvac nullifiers
    async fn get_kvac_nullifiers(&self, _nullifiers: &[GroupElement]) -> Result<Vec<Option<KvacNullifier>>, Self::Err> {
        Err(Self::Err::from(Error::Unimplemented))
    }
    /// Get ys by quote id
    async fn get_proof_ys_by_quote_id(&self, quote_id: &Uuid) -> Result<Vec<PublicKey>, Self::Err>;
    /// Get nullifiers by quote id 
    async fn get_kvac_nullifiers_by_quote_id(&self, _quote_id: &Uuid) -> Result<Vec<KvacNullifier>, Self::Err> {
        Err(Self::Err::from(Error::Unimplemented))
    }
    /// Get [`Proofs`] state
    async fn get_proofs_states(&self, ys: &[PublicKey]) -> Result<Vec<Option<State>>, Self::Err>;
    /// Get kvac nullifiers state
    async fn get_kvac_nullifiers_states(&self, _nullifiers: &[GroupElement]) -> Result<Vec<Option<State>>, Self::Err> {
        Err(Self::Err::from(Error::Unimplemented))
    }
    /// Get [`Proofs`] state
    async fn update_proofs_states(
        &self,
        ys: &[PublicKey],
        proofs_state: State,
    ) -> Result<Vec<Option<State>>, Self::Err>;
    /// Get [`KvacNullifier`] state
    async fn update_kvac_nullifiers_states(
        &self,
        _nullifiers: &[GroupElement],
        _state: State,
    ) -> Result<Vec<Option<State>>, Self::Err> {
        Err(Self::Err::from(Error::Unimplemented))
    }
    /// Get [`Proofs`] by state
    async fn get_proofs_by_keyset_id(
        &self,
        keyset_id: &Id,
    ) -> Result<(Proofs, Vec<Option<State>>), Self::Err>;
    /// Get [`KvacNullifier`] by state
    async fn get_kvac_nullifiers_by_keyset_id(
        &self,
        _keyset_id: &Id,
    ) -> Result<(Vec<KvacNullifier>, Vec<Option<State>>), Self::Err> {
        Err(Self::Err::from(Error::Unimplemented))
    }

    /// Add [`BlindSignature`]
    async fn add_blind_signatures(
        &self,
        blinded_messages: &[PublicKey],
        blind_signatures: &[BlindSignature],
        quote_id: Option<Uuid>,
    ) -> Result<(), Self::Err>;
    /// Add [`KvacIssuedMac`]
    async fn add_kvac_issued_macs(
        &self,
        _mac: &[KvacIssuedMac],
        _quote_id: Option<Uuid>,
    ) -> Result<(), Self::Err> {
        Err(Self::Err::from(Error::Unimplemented))
    }
    /// Get [`BlindSignature`]s
    async fn get_blind_signatures(
        &self,
        blinded_messages: &[PublicKey],
    ) -> Result<Vec<Option<BlindSignature>>, Self::Err>;
    /// Get [`KvacIssuedMac`]
    async fn get_kvac_issued_macs_by_tags(
        &self,
        _tags: &[Scalar],
    ) -> Result<Vec<Option<BlindSignature>>, Self::Err> {
        Err(Self::Err::from(Error::Unimplemented))
    }
    /// Get [`BlindSignature`]s for keyset_id
    async fn get_blind_signatures_for_keyset(
        &self,
        keyset_id: &Id,
    ) -> Result<Vec<BlindSignature>, Self::Err>;
    /// Get [`KvacIssuedMac`] for keyset_id
    async fn get_kvac_issued_macs_for_keyset(
        &self,
        _keyset_id: &Id,
    ) -> Result<Vec<BlindSignature>, Self::Err> {
        Err(Self::Err::from(Error::Unimplemented))
    }
    /// Get [`BlindSignature`]s for quote
    async fn get_blind_signatures_for_quote(
        &self,
        quote_id: &Uuid,
    ) -> Result<Vec<BlindSignature>, Self::Err>;
    /// Get [`KvacIssuedMac`]s for quote
    async fn get_kvac_issued_macs_for_quote(
        &self,
        _quote_id: &Uuid,
    ) -> Result<Vec<KvacIssuedMac>, Self::Err> {
        Err(Self::Err::from(Error::Unimplemented))
    }
}
