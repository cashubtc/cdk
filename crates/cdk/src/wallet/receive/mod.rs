//! Receive module for the wallet.
//!
//! This module provides functionality for receiving ecash tokens and proofs.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

use cdk_common::wallet::KeysetLoadPolicy;
use tracing::instrument;

use crate::nuts::nut00::{self, ProofsMethods};
use crate::nuts::{verify_proof_signatures, Id, Keys, LegacyDleq, Proofs, Token};
use crate::{ensure_cdk, Amount, Error, Wallet};

pub(crate) mod saga;

pub use cdk_common::wallet::ReceiveOptions;
use saga::ReceiveSaga;

/// Wallet ceiling for proofs in a single receipt, matching the default CDK mint input limit.
pub const MAX_RECEIVE_PROOFS: usize = 1000;

// A binary token or payment payload is bounded before cloning or decoding, including
// fields outside the proof arrays. The text limit allows Base64 expansion and its prefix.
const MAX_RECEIVE_BYTES: usize = 1_048_576;
const MAX_ENCODED_TOKEN_BYTES: usize = 6 + 4 * MAX_RECEIVE_BYTES.div_ceil(3);

pub(super) fn check_proof_count(actual: usize, max: usize) -> Result<(), Error> {
    ensure_cdk!(actual <= max, Error::MaxInputsExceeded { actual, max });
    Ok(())
}

fn check_size(actual: usize, max: usize) -> Result<(), Error> {
    ensure_cdk!(
        actual <= max,
        Error::RequestFieldTooLarge {
            field: "token".to_owned(),
            actual,
            max,
        }
    );
    Ok(())
}

fn token_error(error: nut00::Error) -> Error {
    match error {
        nut00::Error::TooManyProofs { actual, max } => Error::MaxInputsExceeded { actual, max },
        error => error.into(),
    }
}

/// Bound simultaneous CPU-heavy wallet work across wallets, including after
/// cancellation of a caller while its blocking job is still running.
pub(super) async fn proof_work<T, F>(work: F) -> Result<T, Error>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, Error> + Send + 'static,
{
    #[cfg(not(target_arch = "wasm32"))]
    {
        static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
        let slot = SLOTS.acquire().await.map_err(|_| Error::Internal)?;
        tokio::task::spawn_blocking(move || {
            let _slot = slot;
            work()
        })
        .await
        .map_err(|_| Error::Internal)?
    }
    #[cfg(target_arch = "wasm32")]
    work()
}

/// Decode an incoming token with wallet receipt limits, before inspecting its mint or proofs.
pub async fn decode_received_token(encoded_token: &str) -> Result<Token, Error> {
    check_size(encoded_token.len(), MAX_ENCODED_TOKEN_BYTES)?;
    let encoded_token = encoded_token.to_owned();
    proof_work(move || {
        Token::from_str_with_proof_limit(&encoded_token, MAX_RECEIVE_PROOFS).map_err(token_error)
    })
    .await
}

/// Decode incoming payment request JSON under the wallet's receipt limits.
pub async fn decode_payment_request_payload(
    json: String,
) -> Result<crate::nuts::PaymentRequestPayload, Error> {
    check_size(json.len(), MAX_RECEIVE_BYTES)?;
    proof_work(move || {
        #[derive(serde::Deserialize)]
        struct Envelope {
            #[serde(default, rename = "id")]
            _id: Option<serde::de::IgnoredAny>,
            #[serde(default, rename = "memo")]
            _memo: Option<serde::de::IgnoredAny>,
            #[serde(rename = "mint")]
            _mint: serde::de::IgnoredAny,
            #[serde(rename = "unit")]
            _unit: serde::de::IgnoredAny,
            proofs: Vec<serde::de::IgnoredAny>,
        }

        let envelope: Envelope = serde_json::from_str(&json)?;
        check_proof_count(envelope.proofs.len(), MAX_RECEIVE_PROOFS)?;
        Ok(serde_json::from_str(&json)?)
    })
    .await
}

impl Wallet {
    pub(super) async fn verify_received_signatures(
        &self,
        proofs: Proofs,
        policy: KeysetLoadPolicy,
        dleq: LegacyDleq,
    ) -> Result<Proofs, Error> {
        let mut keysets: HashMap<Id, Keys> = HashMap::new();
        let mut signed = Vec::new();
        for (index, proof) in proofs.iter().enumerate() {
            if proof.requires_signature_verification(dleq) {
                let keys = match keysets.entry(proof.keyset_id) {
                    Entry::Occupied(entry) => entry.into_mut(),
                    Entry::Vacant(entry) => {
                        entry.insert(self.keyset_with_policy(proof.keyset_id, policy).await?.keys)
                    }
                };
                signed.push((
                    index,
                    keys.amount_key(proof.amount).ok_or(Error::AmountKey)?,
                ));
            }
        }
        if signed.is_empty() {
            return Ok(proofs);
        }
        proof_work(move || {
            verify_proof_signatures(signed.into_iter().map(|(i, k)| (&proofs[i], k)), dleq)
                .map_err(|_| Error::CouldNotVerifyDleq)?;
            Ok(proofs)
        })
        .await
    }

    /// Receive proofs using the saga pattern
    ///
    /// This is the internal implementation that uses the saga pattern
    /// for proper crash recovery and compensation. Duplicate proof identifiers
    /// are rejected before any mint requests or database writes. Wallets accept at most
    /// [`MAX_RECEIVE_PROOFS`] per receipt, subject to the mint's advertised array limit.
    #[instrument(skip_all)]
    pub async fn receive_proofs(
        &self,
        proofs: Proofs,
        opts: ReceiveOptions,
        memo: Option<String>,
        token: Option<String>,
    ) -> Result<Amount, Error> {
        check_proof_count(proofs.len(), MAX_RECEIVE_PROOFS)?;
        let proofs = proof_work(move || {
            let ys = proofs.ys()?;
            ensure_cdk!(
                ys.iter().collect::<HashSet<_>>().len() == ys.len(),
                nut00::Error::DuplicateProofs.into()
            );
            Ok(proofs)
        })
        .await?;

        self.retry_on_inactive_keyset(|| async {
            let saga = ReceiveSaga::new(self);
            let saga = saga
                .prepare(proofs.clone(), opts.clone(), memo.clone(), token.clone())
                .await?;
            let saga = saga.execute().await?;
            Ok(saga.into_amount())
        })
        .await
    }

    /// Receive
    ///
    /// For a requested v3 payment, set `ReceiveOptions::v3_payment_request`
    /// to check the exact nutroot tree. Script-only payments use the supplied
    /// signing keys and hashlock preimages to sweep a satisfiable leaf.
    /// # Synopsis
    /// ```rust, no_run
    ///  use std::sync::Arc;
    ///
    ///  use cdk::amount::SplitTarget;
    ///  use cdk_sqlite::wallet::memory;
    ///  use cdk::nuts::CurrencyUnit;
    ///  use cdk::wallet::{ReceiveOptions, Wallet};
    ///  use rand::random;
    ///
    /// #[tokio::main]
    /// async fn main() -> anyhow::Result<()> {
    ///  let seed = random::<[u8; 64]>();
    ///  let mint_url = "https://testnut.cashudevkit.org";
    ///  let unit = CurrencyUnit::Sat;
    ///
    ///  let localstore = memory::empty().await?;
    ///  let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), seed, None).unwrap();
    ///  let token = "cashuAeyJ0b2tlbiI6W3sicHJvb2ZzIjpbeyJhbW91bnQiOjEsInNlY3JldCI6ImI0ZjVlNDAxMDJhMzhiYjg3NDNiOTkwMzU5MTU1MGYyZGEzZTQxNWEzMzU0OTUyN2M2MmM5ZDc5MGVmYjM3MDUiLCJDIjoiMDIzYmU1M2U4YzYwNTMwZWVhOWIzOTQzZmRhMWEyY2U3MWM3YjNmMGNmMGRjNmQ4NDZmYTc2NWFhZjc3OWZhODFkIiwiaWQiOiIwMDlhMWYyOTMyNTNlNDFlIn1dLCJtaW50IjoiaHR0cHM6Ly90ZXN0bnV0LmNhc2h1LnNwYWNlIn1dLCJ1bml0Ijoic2F0In0=";
    ///  let amount_receive = wallet.receive(token, ReceiveOptions::default()).await?;
    ///  Ok(())
    /// }
    /// ```
    #[instrument(skip_all)]
    pub async fn receive(
        &self,
        encoded_token: &str,
        opts: ReceiveOptions,
    ) -> Result<Amount, Error> {
        let token = decode_received_token(encoded_token).await?;
        self.receive_token(token, opts, encoded_token.to_owned())
            .await
    }

    async fn receive_token(
        &self,
        token: Token,
        opts: ReceiveOptions,
        encoded_token: String,
    ) -> Result<Amount, Error> {
        let unit = token.unit().unwrap_or_default();

        ensure_cdk!(unit == self.unit, Error::UnsupportedUnit);

        let proofs = self.token_proofs(&token).await?;

        if let Token::TokenV3(token) = &token {
            ensure_cdk!(!token.is_multi_mint(), Error::MultiMintTokenNotSupported);
        }

        ensure_cdk!(self.mint_url == token.mint_url()?, Error::IncorrectMint);

        let amount = self
            .receive_proofs(proofs, opts, token.memo().clone(), Some(encoded_token))
            .await?;

        Ok(amount)
    }

    /// Receive
    /// # Synopsis
    /// ```rust, no_run
    ///  use std::sync::Arc;
    ///
    ///  use cdk::amount::SplitTarget;
    ///  use cdk_sqlite::wallet::memory;
    ///  use cdk::nuts::CurrencyUnit;
    ///  use cdk::wallet::{ReceiveOptions, Wallet};
    ///  use cdk::util::hex;
    ///  use rand::random;
    ///
    /// #[tokio::main]
    /// async fn main() -> anyhow::Result<()> {
    ///  let seed = random::<[u8; 64]>();
    ///  let mint_url = "https://testnut.cashudevkit.org";
    ///  let unit = CurrencyUnit::Sat;
    ///
    ///  let localstore = memory::empty().await?;
    ///  let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), seed, None).unwrap();
    ///  let token_raw = hex::decode("6372617742a4617481a261694800ad268c4d1f5826617081a3616101617378403961366462623834376264323332626137366462306466313937323136623239643362386363313435353363643237383237666331636339343266656462346561635821038618543ffb6b8695df4ad4babcde92a34a96bdcd97dcee0d7ccf98d4721267926164695468616e6b20796f75616d75687474703a2f2f6c6f63616c686f73743a33333338617563736174").unwrap();
    ///  let amount_receive = wallet.receive_raw(&token_raw, ReceiveOptions::default()).await?;
    ///  Ok(())
    /// }
    /// ```
    #[instrument(skip_all)]
    pub async fn receive_raw(
        &self,
        binary_token: &[u8],
        opts: ReceiveOptions,
    ) -> Result<Amount, Error> {
        check_size(binary_token.len(), MAX_RECEIVE_BYTES + 5)?;
        let bytes = binary_token.to_vec();
        let (token, encoded) = proof_work(move || {
            let token = Token::from_raw_with_proof_limit(&bytes, MAX_RECEIVE_PROOFS)
                .map_err(token_error)?;
            let encoded = token.to_string();
            Ok((token, encoded))
        })
        .await?;
        self.receive_token(token, opts, encoded).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn payment_payload_rejects_excess_proofs_before_point_decoding() {
        let limit = MAX_RECEIVE_PROOFS;
        let payload = serde_json::json!({
            "mint": "https://mint.test",
            "unit": "sat",
            "proofs": vec![serde_json::Value::Null; limit + 1],
        });
        let result = decode_payment_request_payload(payload.to_string()).await;
        assert!(
            matches!(result, Err(Error::MaxInputsExceeded { actual, max }) if actual == limit + 1 && max == limit)
        );
    }
}
