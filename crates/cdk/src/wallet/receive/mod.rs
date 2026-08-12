//! Receive module for the wallet.
//!
//! This module provides functionality for receiving ecash tokens and proofs.

use std::collections::HashMap;
use std::str::FromStr;

use cdk_common::wallet::KeysetLoadPolicy;
use tracing::instrument;
use uuid::Uuid;

use crate::nuts::{Proofs, Token};
use crate::{ensure_cdk, Amount, Error, Wallet};

pub(crate) mod saga;

pub use cdk_common::wallet::ReceiveOptions;
use saga::ReceiveSaga;

impl Wallet {
    /// Receive proofs using the saga pattern
    ///
    /// This is the internal implementation that uses the saga pattern
    /// for proper crash recovery and compensation.
    #[instrument(skip_all)]
    pub async fn receive_proofs(
        &self,
        proofs: Proofs,
        opts: ReceiveOptions,
        memo: Option<String>,
        token: Option<String>,
    ) -> Result<Amount, Error> {
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
        let token = Token::from_str(encoded_token)?;

        let unit = token.unit().unwrap_or_default();

        ensure_cdk!(unit == self.unit, Error::UnsupportedUnit);

        let proofs = self.token_proofs(&token).await?;

        if let Token::TokenV3(token) = &token {
            ensure_cdk!(!token.is_multi_mint(), Error::MultiMintTokenNotSupported);
        }

        ensure_cdk!(self.mint_url == token.mint_url()?, Error::IncorrectMint);

        let amount = self
            .receive_proofs(
                proofs,
                opts,
                token.memo().clone(),
                Some(encoded_token.to_string()),
            )
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
        binary_token: &Vec<u8>,
        opts: ReceiveOptions,
    ) -> Result<Amount, Error> {
        let token_str = Token::try_from(binary_token)?.to_string();
        self.receive(token_str.as_str(), opts).await
    }

    /// Receive an encoded token offline without contacting the mint
    #[instrument(skip_all)]
    pub async fn receive_offline(
        &self,
        encoded_token: &str,
        opts: cdk_common::wallet::OfflineReceiveOptions,
    ) -> Result<Amount, Error> {
        let token = Token::from_str(encoded_token)?;

        let unit = token.unit().unwrap_or_default();
        ensure_cdk!(unit == self.unit, Error::UnsupportedUnit);

        let mint_url = token.mint_url()?;
        ensure_cdk!(self.mint_url == mint_url, Error::IncorrectMint);

        if let Token::TokenV3(token) = &token {
            ensure_cdk!(!token.is_multi_mint(), Error::MultiMintTokenNotSupported);
        }

        // token_proofs loads ALL keysets (active + inactive) so tokens from
        // rotated keysets are decoded correctly — the same helper used by
        // online receive(), keeping the two paths consistent.
        let mut proofs = self.token_proofs(&token).await?;

        let mut total_amount = Amount::ZERO;

        for proof in &mut proofs {
            // DLEQ verification is always required for offline receive
            ensure_cdk!(proof.dleq.is_some(), Error::DleqProofNotProvided);

            // CacheOnly so offline receive never contacts the mint — the
            // default policy (CacheThenNetwork) would silently go online.
            let keys = self
                .keyset_with_policy(proof.keyset_id, KeysetLoadPolicy::CacheOnly)
                .await?
                .keys;
            let key = keys.amount_key(proof.amount).ok_or_else(|| {
                Error::Custom(format!(
                    "keyset {} not in local cache — connect to the mint once to sync keysets",
                    proof.keyset_id
                ))
            })?;
            proof.verify_dleq(key)?;

            if opts.require_locked {
                let secret: crate::nuts::nut10::Secret =
                    proof.secret.clone().try_into().map_err(|_| {
                        Error::InvalidSpendConditions("Token must be P2PK locked".to_string())
                    })?;
                ensure_cdk!(
                    matches!(secret.kind(), crate::nuts::nut10::Kind::P2PK),
                    Error::InvalidSpendConditions("Token must be P2PK locked".to_string())
                );
            }

            if let Some(min_locktime) = opts.minimum_locktime {
                let secret: crate::nuts::nut10::Secret = proof
                    .secret
                    .clone()
                    .try_into()
                    .map_err(|_| Error::LocktimeNotProvided)?;
                let conditions: crate::nuts::Conditions = secret
                    .secret_data()
                    .tags()
                    .cloned()
                    .unwrap_or_default()
                    .try_into()
                    .map_err(|_| Error::LocktimeNotProvided)?;
                let locktime = conditions.locktime.ok_or(Error::LocktimeNotProvided)?;
                ensure_cdk!(
                    locktime >= min_locktime,
                    Error::InvalidSpendConditions(format!(
                        "Locktime {} is less than required {}",
                        locktime, min_locktime
                    ))
                );
            }

            total_amount += proof.amount;
        }

        use cdk_common::wallet::{
            OperationData, ReceiveOperationData, ReceiveSagaState, WalletSaga, WalletSagaState,
        };

        use crate::nuts::State;
        use crate::wallet::ProofInfo;

        // One UUID ties all proofs from this token together so finalize_pending_receives
        // can process them as a single receive operation and recover the memo.
        let operation_id = Uuid::now_v7();

        let proofs_info = proofs
            .clone()
            .into_iter()
            .map(|p| {
                ProofInfo::new_with_operations(
                    p,
                    self.mint_url.clone(),
                    State::PendingReceive,
                    self.unit.clone(),
                    None,
                    Some(operation_id),
                )
            })
            .collect::<Result<Vec<ProofInfo>, _>>()?;

        // Store proofs as PendingReceive — no transaction is recorded here.
        // The transaction will be recorded by the ReceiveSaga when
        // `finalize_pending_receives` completes the swap with the mint.
        self.localstore.update_proofs(proofs_info, vec![]).await?;

        // Write a saga entry to preserve the original token string (and its memo)
        // until finalization. The OfflinePendingReceive state tells crash recovery
        // to skip this saga — finalize_pending_receives owns it.
        let saga = WalletSaga::new(
            operation_id,
            WalletSagaState::Receive(ReceiveSagaState::OfflinePendingReceive),
            total_amount,
            self.mint_url.clone(),
            self.unit.clone(),
            OperationData::Receive(ReceiveOperationData {
                token: Some(encoded_token.to_string()),
                counter_start: None,
                counter_end: None,
                amount: Some(total_amount),
                blinded_messages: None,
            }),
        );
        self.localstore.add_saga(saga).await?;

        Ok(total_amount)
    }

    /// Finalize pending offline receives by attempting to swap them with the mint.
    ///
    /// Proofs that were stored via [`Wallet::receive_offline`] are processed in
    /// their original token groups so that a multi-proof token produces a single
    /// transaction record and the sender's memo is preserved.
    ///
    /// Each group (and any ungrouped legacy proofs) is processed independently so
    /// that one bad token cannot block valid ones:
    ///
    /// - **Success**: all proofs in the group are swapped for fresh `Unspent`
    ///   proofs and one transaction record is written with the original memo.
    /// - **Definitive failure** (e.g. the sender double-spent the token): the
    ///   saga's compensation step removes the proofs from the database. The proof
    ///   Y values and error are logged at `WARN` level for diagnostics.
    /// - **Transient failure** (e.g. mint unreachable): the proofs are left in
    ///   `Pending` state with a `SwapRequested` saga entry. Call
    ///   [`Wallet::recover_incomplete_sagas`] once back online to retry.
    #[instrument(skip_all)]
    pub async fn finalize_pending_receives(&self) -> Result<Amount, Error> {
        use cdk_common::wallet::OperationData;

        use crate::nuts::State;

        let proofs_info = self
            .localstore
            .get_proofs(
                Some(self.mint_url.clone()),
                Some(self.unit.clone()),
                Some(vec![State::PendingReceive]),
                None,
            )
            .await?;

        if proofs_info.is_empty() {
            return Ok(Amount::ZERO);
        }

        let mut total = Amount::ZERO;

        // Group proofs by their offline-receive operation ID (created_by_operation).
        // Proofs without an ID were stored before this grouping was introduced and
        // are processed individually as a backward-compatible fallback.
        let mut grouped: HashMap<Uuid, Vec<_>> = HashMap::new();
        let mut ungrouped: Vec<_> = Vec::new();

        for proof_info in proofs_info {
            match proof_info.created_by_operation {
                Some(op_id) => grouped.entry(op_id).or_default().push(proof_info),
                None => ungrouped.push(proof_info),
            }
        }

        // Process each group as one receive operation, recovering the memo from
        // the OfflinePendingReceive saga written at receive_offline time.
        for (op_id, proof_infos) in grouped {
            let (memo, token_str) = match self.localstore.get_saga(&op_id).await? {
                Some(saga) => {
                    if let OperationData::Receive(ref data) = saga.data {
                        let token_str = data.token.clone();
                        let memo = token_str
                            .as_deref()
                            .and_then(|t| Token::from_str(t).ok())
                            .and_then(|tok| tok.memo().clone());
                        (memo, token_str)
                    } else {
                        (None, None)
                    }
                }
                None => (None, None),
            };

            let proof_ys: Vec<_> = proof_infos.iter().map(|p| p.y).collect();
            let proofs: Vec<_> = proof_infos.into_iter().map(|p| p.proof).collect();

            let result = self
                .receive_proofs(proofs, ReceiveOptions::default(), memo, token_str)
                .await;

            // Only drop the OfflinePendingReceive saga once the proofs have actually
            // been taken over. `receive_proofs` runs prepare() (no DB changes) before
            // execute() (which moves the proofs to Pending under a SwapRequested saga
            // and registers compensation). If it fails inside prepare() the proofs are
            // still in PendingReceive and this saga is the only record of their memo,
            // so deleting it here would lose the memo (transient) or strand the proofs
            // forever (definitive). Detect that by checking whether the proofs are
            // still PendingReceive under this operation.
            match result {
                Ok(amount) => {
                    total += amount;
                    self.delete_offline_receive_saga(op_id).await;
                }
                Err(e) => {
                    let still_pending = self
                        .localstore
                        .get_proofs(
                            Some(self.mint_url.clone()),
                            Some(self.unit.clone()),
                            Some(vec![State::PendingReceive]),
                            None,
                        )
                        .await?
                        .iter()
                        .any(|p| p.created_by_operation == Some(op_id));

                    if !still_pending {
                        // execute() took ownership: the proofs are now Pending under a
                        // SwapRequested saga (transient) or were removed by compensation
                        // (definitive). Either way the offline saga is redundant.
                        self.delete_offline_receive_saga(op_id).await;
                        if e.is_definitive_failure() {
                            tracing::warn!(
                                ys = ?proof_ys,
                                error = %e,
                                "Mint definitively rejected a pending offline receive; \
                                 proofs removed (token was double-spent or invalid)"
                            );
                        } else {
                            tracing::warn!(
                                ys = ?proof_ys,
                                error = %e,
                                "Transient failure finalizing pending offline receive; \
                                 will retry via recover_incomplete_sagas()"
                            );
                        }
                    } else if e.is_definitive_failure() {
                        // prepare() rejected the token for good; the proofs are orphaned
                        // in PendingReceive. Remove them, then drop the offline saga.
                        self.localstore
                            .update_proofs(vec![], proof_ys.clone())
                            .await?;
                        self.delete_offline_receive_saga(op_id).await;
                        tracing::warn!(
                            ys = ?proof_ys,
                            error = %e,
                            "Mint definitively rejected a pending offline receive; \
                             proofs removed (token was double-spent or invalid)"
                        );
                    } else {
                        // prepare() failed transiently: keep both the proofs and the
                        // saga so the next finalize_pending_receives retains the memo
                        // and can retry.
                        tracing::warn!(
                            ys = ?proof_ys,
                            error = %e,
                            "Transient failure finalizing pending offline receive; \
                             will retry on next finalize_pending_receives()"
                        );
                    }
                }
            }
        }

        // Backward-compatible path: proofs stored without a created_by_operation
        // (written before this fix) are processed one at a time with no memo.
        for proof_info in ungrouped {
            let proof_y = proof_info.y;

            let result = self
                .receive_proofs(
                    vec![proof_info.proof],
                    ReceiveOptions::default(),
                    None,
                    None,
                )
                .await;

            match result {
                Ok(amount) => total += amount,
                Err(e) => {
                    // Same prepare()-vs-execute() reasoning as the grouped path: if
                    // the proof is still PendingReceive the failure was in prepare()
                    // before execute() took ownership, so a definitive failure here
                    // left it orphaned and a transient one must be left for retry.
                    let still_pending = self
                        .localstore
                        .get_proofs(
                            Some(self.mint_url.clone()),
                            Some(self.unit.clone()),
                            Some(vec![State::PendingReceive]),
                            None,
                        )
                        .await?
                        .iter()
                        .any(|p| p.y == proof_y);

                    if still_pending && e.is_definitive_failure() {
                        self.localstore.update_proofs(vec![], vec![proof_y]).await?;
                        tracing::warn!(
                            y = %proof_y,
                            error = %e,
                            "Mint definitively rejected a pending offline receive; \
                             proof removed (token was double-spent or invalid)"
                        );
                    } else if e.is_definitive_failure() {
                        tracing::warn!(
                            y = %proof_y,
                            error = %e,
                            "Mint definitively rejected a pending offline receive; \
                             proof removed (token was double-spent or invalid)"
                        );
                    } else if still_pending {
                        tracing::warn!(
                            y = %proof_y,
                            error = %e,
                            "Transient failure finalizing pending offline receive; \
                             will retry on next finalize_pending_receives()"
                        );
                    } else {
                        tracing::warn!(
                            y = %proof_y,
                            error = %e,
                            "Transient failure finalizing pending offline receive; \
                             will retry via recover_incomplete_sagas()"
                        );
                    }
                }
            }
        }

        Ok(total)
    }

    /// Delete an `OfflinePendingReceive` saga, logging (but not failing) on error.
    /// An orphaned saga is harmless and cleaned up on recovery.
    async fn delete_offline_receive_saga(&self, op_id: Uuid) {
        if let Err(e) = self.localstore.delete_saga(&op_id).await {
            tracing::warn!(
                "Failed to delete offline pending receive saga {}: {}",
                op_id,
                e
            );
        }
    }
}
