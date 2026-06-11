//! Send Saga - Type State Pattern Implementation
//!
//! This module implements the saga pattern for send operations using the typestate
//! pattern to enforce valid state transitions at compile-time.
//!
//! # State Flow
//!
//! ```text
//!                                  Normal Flow
//!                                  ===========
//!
//! [saga created] ──► ProofsReserved ──► TokenCreated ──► [recipient claims] ──► [completed]
//!                                            │
//!                                            └──► [user revokes]
//!                                                       │
//!                                                       ├─ proofs already spent ──► [completed] + error
//!                                                       │
//!                                                       └─ proofs not spent
//!                                                                 │
//!                                                                 ▼
//!                                                           RollingBack
//!                                                                 │
//!                                                       ┌─────────┴─────────┐
//!                                                       │                   │
//!                                                  swap succeeds       swap fails
//!                                                       │                   │
//!                                                       ▼                   ▼
//!                                                  [completed]        TokenCreated
//!                                                (proofs reclaimed)   (revert, retry)
//!
//!
//!                                  Recovery Flow
//!                                  =============
//!
//! ProofsReserved ─────────────────────────────────────────► [compensated]
//!
//! TokenCreated
//!     ├─ proofs spent ────────► [completed] (recipient claimed)
//!     ├─ proofs not spent ────► [completed] (saga deleted, token still valid)
//!     └─ mint unreachable ────► [skipped]
//!
//! RollingBack
//!     ├─ proofs spent ────────► [completed] (revoke swap succeeded)
//!     ├─ proofs not spent ────► TokenCreated (revert state, keep monitoring)
//!     └─ mint unreachable ────► [skipped]
//! ```
//!
//! # States
//!
//! | State | Description |
//! |-------|-------------|
//! | `ProofsReserved` | Proofs selected and reserved for sending, ready to create token |
//! | `TokenCreated` | Token created and ready to share, proofs marked as pending spent awaiting claim |
//! | `RollingBack` | Rollback in progress, reclaiming proofs via swap (transient state) |
//!
//! # Recovery Outcomes
//!
//! | Outcome | Description |
//! |---------|-------------|
//! | `[completed]` | Send finalized - either recipient claimed, or user successfully revoked |
//! | `[compensated]` | Send cancelled before token created, reserved proofs released |
//! | `[skipped]` | Recovery deferred (mint unreachable), will retry on next recovery |

use std::collections::{HashMap, HashSet};

use bitcoin::XOnlyPublicKey;
use cdk_common::amount::KeysetFeeAndAmounts;
use cdk_common::util::unix_time;
use cdk_common::wallet::{
    KeysetLoadPolicy, OperationData, P2PKLockedProofSendMode, SendOperationData, SendSagaState,
    Transaction, TransactionDirection, WalletSaga, WalletSagaState,
};
use cdk_common::Id;
use tracing::instrument;

use self::state::{Initial, Prepared, TokenCreated};
use super::{split_proofs_for_send, SendMemo, SendOptions};
use crate::amount::SplitTarget;
use crate::fees::calculate_fee;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::nut11::{enforce_sig_flag, SigFlag};
use crate::nuts::{Proofs, State, Token};
use crate::wallet::saga::{
    add_compensation, execute_compensations, new_compensations, Compensations,
    RevertProofReservation,
};
use crate::wallet::SendKind;
use crate::{Amount, Error, Wallet};

pub(crate) mod resume;
pub(crate) mod state;

fn verify_p2pk_proofs(proofs: &crate::nuts::Proofs) -> Result<(), Error> {
    for proof in proofs {
        if crate::wallet::util::is_p2pk_locked(proof) {
            proof.verify_p2pk()?;
        }
    }

    Ok(())
}

/// Filter a proof pool to retain only proofs that the wallet can sign and verify.
///
/// P2PK-locked proofs with no matching key (neither in `explicit_keys` nor the wallet keyring)
/// are removed. All other proofs (plain, HTLC) pass through unchanged.
async fn filter_signable_proofs(
    wallet: &Wallet,
    proofs: crate::nuts::Proofs,
    explicit_keys: &[crate::nuts::SecretKey],
) -> Result<crate::nuts::Proofs, Error> {
    let mut out = Vec::with_capacity(proofs.len());

    for proof in proofs {
        // Fast path: non-P2PK proofs are always included.
        if !crate::wallet::util::is_p2pk_locked(&proof) {
            out.push(proof);
            continue;
        }

        let mut signed = vec![proof.clone()];
        let keys = match merge_keyring_keys(wallet, &signed, explicit_keys).await {
            Ok(keys) => keys,
            Err(Error::NUT01(_)) => continue,
            Err(err) => return Err(err),
        };
        match crate::wallet::util::sign_proofs(&mut signed, &keys) {
            Ok(()) => {}
            Err(Error::NUT01(_)) => continue,
            Err(err) => return Err(err),
        }

        if verify_p2pk_proofs(&signed).is_ok() {
            out.push(proof);
        }
    }

    Ok(out)
}

/// Build the signing key list for the given proofs by merging explicitly-provided keys with
/// any matching keys found in the wallet keyring.
///
/// Explicit keys (from `SendOptions.p2pk_signing_keys`) take precedence; the keyring is only
/// consulted for pubkeys not already covered by the explicit set.
async fn merge_keyring_keys(
    wallet: &Wallet,
    proofs: &crate::nuts::Proofs,
    explicit_keys: &[crate::nuts::SecretKey],
) -> Result<Vec<crate::nuts::SecretKey>, Error> {
    let mut keys = explicit_keys.to_vec();
    let covered: HashSet<XOnlyPublicKey> = keys
        .iter()
        .map(|k| k.x_only_public_key(&crate::SECP256K1).0)
        .collect();

    let pubkeys = crate::wallet::util::collect_p2pk_pubkeys(proofs)?;
    for pubkey in pubkeys {
        let x_only = pubkey.x_only_public_key();
        if !covered.contains(&x_only) {
            if let Some(secret_key) = wallet.get_signing_key(&pubkey).await? {
                keys.push(secret_key);
            }
        }
    }

    Ok(keys)
}

struct SendSplitContext<'a> {
    send_amounts: &'a [Amount],
    amount: Amount,
    send_fee: Amount,
    keyset_fees: &'a HashMap<Id, u64>,
    force_swap: bool,
    is_exact_or_offline: bool,
}

struct InputFeeCoverageContext<'a> {
    amount: Amount,
    send_fee: Amount,
    active_keyset_ids: &'a Vec<Id>,
    keyset_fees: &'a KeysetFeeAndAmounts,
    send_amounts: &'a [Amount],
    force_swap: bool,
    is_exact_or_offline: bool,
}

fn ensure_selected_proofs_cover_input_fees(
    mut selected_proofs: Proofs,
    proof_pool: Proofs,
    context: InputFeeCoverageContext<'_>,
) -> Result<Proofs, Error> {
    let keyset_fee_map: HashMap<Id, u64> = context
        .keyset_fees
        .iter()
        .map(|(key, values)| (*key, values.fee()))
        .collect();
    let mut remaining_proofs: Proofs = proof_pool
        .into_iter()
        .filter(|proof| !selected_proofs.contains(proof))
        .collect();

    loop {
        let selected_net = selected_proofs_net_after_swap_fees(
            selected_proofs.clone(),
            SendSplitContext {
                send_amounts: context.send_amounts,
                amount: context.amount,
                send_fee: context.send_fee,
                keyset_fees: &keyset_fee_map,
                force_swap: context.force_swap,
                is_exact_or_offline: context.is_exact_or_offline,
            },
        )?;

        if selected_net >= context.amount + context.send_fee {
            return Ok(selected_proofs);
        }

        if remaining_proofs.is_empty() {
            return Err(Error::InsufficientFunds);
        }

        let shortfall = (context.amount + context.send_fee)
            .checked_sub(selected_net)
            .unwrap_or(Amount::ZERO);
        let additional = Wallet::select_proofs(
            shortfall,
            remaining_proofs.clone(),
            context.active_keyset_ids,
            context.keyset_fees,
            false,
        )?;

        if additional.is_empty() {
            return Err(Error::InsufficientFunds);
        }

        remaining_proofs.retain(|proof| !additional.contains(proof));
        selected_proofs.extend(additional);
    }
}

fn split_proofs_for_send_respecting_p2pk_locks(
    proofs: Proofs,
    p2pk_locked_proof_send_mode: P2PKLockedProofSendMode,
    context: SendSplitContext<'_>,
) -> Result<super::ProofSplitResult, Error> {
    // When the wallet holds P2PK-locked proofs and passthrough is not opted in, route them
    // through a swap so the token contains fresh, unlocked proofs. The signing key may come
    // from `SendOptions.p2pk_signing_keys` or be discovered automatically from the wallet
    // keyring at confirm time; we partition eagerly regardless so that the swap is set up
    // correctly even when only keyring keys will be used.
    //
    // Unlocked proofs bypass the swap entirely — they are already bearer.
    //
    // HTLC-locked proofs are intentionally excluded from this forced-swap path even though
    // they also carry a NUT-10 secret. Spending an HTLC requires a preimage that signing
    // keys alone cannot provide; routing HTLC proofs to a swap here would cause a mint
    // rejection. HTLC support in the send path (including a `htlc_preimages` field on
    // `SendOptions` and its own partition logic) is left for a follow-up PR.
    let has_p2pk_locked = proofs.iter().any(crate::wallet::util::is_p2pk_locked);
    if has_p2pk_locked && p2pk_locked_proof_send_mode == P2PKLockedProofSendMode::Swap {
        let (p2pk_locked, rest): (Proofs, Proofs) = proofs
            .into_iter()
            .partition(crate::wallet::util::is_p2pk_locked);
        let mut proofs_to_swap = p2pk_locked;
        let mut proofs_to_send = Proofs::new();

        if context.force_swap {
            proofs_to_swap.extend(rest);
        } else if context.is_exact_or_offline {
            proofs_to_send = rest;
        } else {
            let mut remaining_send_amounts: Vec<Amount> = context.send_amounts.to_vec();
            for proof in rest {
                if let Some(idx) = remaining_send_amounts
                    .iter()
                    .position(|a| a == &proof.amount)
                {
                    proofs_to_send.push(proof);
                    remaining_send_amounts.remove(idx);
                } else {
                    proofs_to_swap.push(proof);
                }
            }

            if !proofs_to_swap.is_empty() {
                let swap_output_needed = (context.amount + context.send_fee)
                    .checked_sub(proofs_to_send.total_amount()?)
                    .unwrap_or(Amount::ZERO);

                if swap_output_needed != Amount::ZERO {
                    loop {
                        let swap_input_fee =
                            calculate_fee(&proofs_to_swap.count_by_keyset(), context.keyset_fees)?
                                .total;
                        let swap_total = proofs_to_swap.total_amount()?;
                        let swap_can_produce = swap_total.checked_sub(swap_input_fee);

                        match swap_can_produce {
                            Some(can_produce) if can_produce >= swap_output_needed => {
                                break;
                            }
                            _ => {
                                if proofs_to_send.is_empty() {
                                    return Err(Error::InsufficientFunds);
                                }

                                proofs_to_send.sort_by_key(|a| a.amount);
                                let proof_to_move = proofs_to_send.remove(0);
                                proofs_to_swap.push(proof_to_move);
                            }
                        }
                    }
                }
            }
        }

        let swap_fee = calculate_fee(&proofs_to_swap.count_by_keyset(), context.keyset_fees)?.total;
        Ok(super::ProofSplitResult {
            proofs_to_send,
            proofs_to_swap,
            swap_fee,
        })
    } else {
        split_proofs_for_send(
            proofs,
            context.send_amounts,
            context.amount,
            context.send_fee,
            context.keyset_fees,
            context.force_swap,
            context.is_exact_or_offline,
        )
    }
}

fn selected_proofs_net_after_swap_fees(
    selected_proofs: Proofs,
    context: SendSplitContext<'_>,
) -> Result<Amount, Error> {
    let split = split_proofs_for_send_respecting_p2pk_locks(
        selected_proofs,
        P2PKLockedProofSendMode::Swap,
        context,
    )?;
    let direct_total = split.proofs_to_send.total_amount()?;
    let swap_total = split.proofs_to_swap.total_amount()?;
    let swap_net = swap_total
        .checked_sub(split.swap_fee)
        .unwrap_or(Amount::ZERO);

    Ok(direct_total + swap_net)
}

/// Saga pattern implementation for send operations.
///
/// Uses the typestate pattern to enforce valid state transitions at compile-time.
/// Each state (Initial, Prepared, Confirmed) is a distinct type, and operations
/// are only available on the appropriate type.
pub(crate) struct SendSaga<'a, S> {
    /// Wallet reference
    pub(crate) wallet: &'a Wallet,
    /// Compensating actions in LIFO order (most recent first)
    pub(crate) compensations: Compensations,
    /// State-specific data
    pub(crate) state_data: S,
}

impl<'a> SendSaga<'a, Initial> {
    /// Create a new send saga in the Initial state.
    pub fn new(wallet: &'a Wallet) -> Self {
        let operation_id = uuid::Uuid::now_v7();

        Self {
            wallet,
            compensations: new_compensations(),
            state_data: Initial {
                operation_id,
                keyset_policy: Default::default(),
            },
        }
    }

    /// Override the keyset load policy for this saga.
    pub fn with_keyset_policy(mut self, policy: KeysetLoadPolicy) -> Self {
        self.state_data.keyset_policy = policy;
        self
    }

    /// Prepare the send operation by selecting and reserving proofs.
    ///
    /// Refreshes keysets (if online), selects and reserves proofs for the
    /// requested amount, and splits proofs between direct send and swap.
    ///
    /// Registers compensation to revert proof reservation on failure.
    #[instrument(skip_all)]
    pub async fn prepare(
        self,
        amount: Amount,
        opts: SendOptions,
    ) -> Result<SendSaga<'a, Prepared>, Error> {
        tracing::info!(
            "Preparing send for {} with operation {}",
            amount,
            self.state_data.operation_id
        );

        let keyset_policy = self.state_data.keyset_policy;

        let all_keysets = self.wallet.keysets(keyset_policy).await?;

        let keyset_fees: KeysetFeeAndAmounts = all_keysets
            .iter()
            .map(|ks| {
                (
                    ks.id,
                    (
                        ks.input_fee_ppk,
                        ks.keys
                            .iter()
                            .map(|(amount, _)| amount.to_u64())
                            .collect::<Vec<_>>(),
                    )
                        .into(),
                )
            })
            .collect();

        let active_keyset_ids: Vec<Id> = all_keysets
            .iter()
            .filter(|k| k.active.unwrap_or(false))
            .map(|k| k.id)
            .collect();

        let active_keyset = all_keysets
            .into_iter()
            .filter(|k| k.active.unwrap_or(false))
            .min_by_key(|k| k.input_fee_ppk)
            .ok_or(Error::NoActiveKeyset)?;

        let active_keyset_id = active_keyset.id;
        let fee_and_amounts = keyset_fees
            .get(&active_keyset_id)
            .cloned()
            .ok_or(Error::UnknownKeySet)?;

        let mut available_proofs = self
            .wallet
            .get_proofs_with(
                Some(vec![State::Unspent]),
                opts.conditions.clone().map(|c| vec![c]),
            )
            .await?;

        // When passthrough is not opted in, exclude P2PK-locked proofs for which the wallet
        // holds no signing key (neither explicit nor in the keyring). Without a key, such proofs
        // cannot be signed before the swap and would cause a mint rejection at confirm time.
        // Excluding them here lets the selection algorithm work with only spendable proofs and
        // surfaces a clean InsufficientFunds error if nothing else is available.
        if opts.p2pk_locked_proof_send_mode == P2PKLockedProofSendMode::Swap {
            available_proofs =
                filter_signable_proofs(self.wallet, available_proofs, &opts.p2pk_signing_keys)
                    .await?;
            if opts.send_kind.is_offline() {
                available_proofs.retain(|proof| !crate::wallet::util::is_p2pk_locked(proof));
            }
        }

        let mut force_swap = false;
        let available_sum = available_proofs.total_amount()?;
        if available_sum < amount {
            if opts.conditions.is_none() || opts.send_kind.is_offline() {
                return Err(Error::InsufficientFunds);
            } else {
                tracing::debug!("Insufficient proofs matching conditions");
                force_swap = true;
                available_proofs = self
                    .wallet
                    .localstore
                    .get_proofs(
                        Some(self.wallet.mint_url.clone()),
                        Some(self.wallet.unit.clone()),
                        Some(vec![State::Unspent]),
                        Some(vec![]),
                    )
                    .await?
                    .into_iter()
                    .map(|p| p.proof)
                    .collect();

                if opts.p2pk_locked_proof_send_mode == P2PKLockedProofSendMode::Swap {
                    available_proofs = filter_signable_proofs(
                        self.wallet,
                        available_proofs,
                        &opts.p2pk_signing_keys,
                    )
                    .await?;
                }
            }
        }

        let send_amounts = if opts.include_fee {
            let send_split = amount.split_with_fee(&fee_and_amounts)?;
            let send_fee = self
                .wallet
                .get_proofs_fee_by_count(
                    vec![(active_keyset_id, send_split.len() as u64)]
                        .into_iter()
                        .collect(),
                )
                .await?;
            (send_split, send_fee.total)
        } else {
            (amount.split(&fee_and_amounts)?, Amount::ZERO)
        };
        let selection_amount = amount + send_amounts.1;

        let may_swap_p2pk_locked = opts.p2pk_locked_proof_send_mode
            == P2PKLockedProofSendMode::Swap
            && available_proofs
                .iter()
                .any(crate::wallet::util::is_p2pk_locked);

        let proof_pool = available_proofs.clone();
        let mut selected_proofs = Wallet::select_proofs(
            selection_amount,
            available_proofs,
            &active_keyset_ids,
            &keyset_fees,
            opts.include_fee || force_swap,
        )?;

        let send_fee = if opts.include_fee {
            self.wallet.get_proofs_fee(&selected_proofs).await?.total
        } else {
            Amount::ZERO
        };

        if may_swap_p2pk_locked {
            let is_exact_or_offline = selected_proofs.total_amount()? == amount + send_fee
                || opts.send_kind.is_offline()
                || opts.send_kind.has_tolerance();
            selected_proofs = ensure_selected_proofs_cover_input_fees(
                selected_proofs,
                proof_pool,
                InputFeeCoverageContext {
                    amount,
                    send_fee,
                    active_keyset_ids: &active_keyset_ids,
                    keyset_fees: &keyset_fees,
                    send_amounts: &send_amounts.0,
                    force_swap,
                    is_exact_or_offline,
                },
            )?;
        }

        let selected_total = selected_proofs.total_amount()?;

        if selected_total == amount + send_fee {
            return self
                .internal_prepare(amount, opts, selected_proofs, force_swap, keyset_policy)
                .await;
        } else if opts.send_kind == SendKind::OfflineExact {
            return Err(Error::InsufficientFunds);
        }

        let tolerance = match opts.send_kind {
            SendKind::OfflineTolerance(tolerance) => Some(tolerance),
            SendKind::OnlineTolerance(tolerance) => Some(tolerance),
            _ => None,
        };
        if let Some(tolerance) = tolerance {
            if selected_total - amount > tolerance && opts.send_kind.is_offline() {
                return Err(Error::InsufficientFunds);
            }
        }

        self.internal_prepare(amount, opts, selected_proofs, force_swap, keyset_policy)
            .await
    }

    async fn internal_prepare(
        mut self,
        amount: Amount,
        opts: SendOptions,
        proofs: Proofs,
        force_swap: bool,
        keyset_policy: KeysetLoadPolicy,
    ) -> Result<SendSaga<'a, Prepared>, Error> {
        let active_keyset_id = self
            .wallet
            .active_keyset_with_policy(keyset_policy)
            .await?
            .id;
        let fee_and_amounts = self
            .wallet
            .get_keyset_fees_and_amounts_with_policy(keyset_policy)
            .await?
            .get(&active_keyset_id)
            .cloned()
            .ok_or(Error::UnknownKeySet)?;

        let (send_amounts, send_fee) = if opts.include_fee {
            let send_split = amount.split_with_fee(&fee_and_amounts)?;
            let send_fee = self
                .wallet
                .get_proofs_fee_by_count(
                    vec![(active_keyset_id, send_split.len() as u64)]
                        .into_iter()
                        .collect(),
                )
                .await?;
            (send_split, send_fee)
        } else {
            let send_split = amount.split(&fee_and_amounts)?;
            let send_fee = crate::fees::ProofsFeeBreakdown {
                total: Amount::ZERO,
                per_keyset: std::collections::HashMap::new(),
            };
            (send_split, send_fee)
        };

        let mut exact_proofs = proofs.total_amount()? == amount + send_fee.total;
        if let Some(max_proofs) = opts.max_proofs {
            exact_proofs &= proofs.len() <= max_proofs;
        }

        let is_exact_or_offline =
            exact_proofs || opts.send_kind.is_offline() || opts.send_kind.has_tolerance();

        let keyset_fees_and_amounts = self
            .wallet
            .get_keyset_fees_and_amounts_with_policy(keyset_policy)
            .await?;
        let keyset_fees: HashMap<Id, u64> = keyset_fees_and_amounts
            .iter()
            .map(|(key, values)| (*key, values.fee()))
            .collect();

        let split_result = split_proofs_for_send_respecting_p2pk_locks(
            proofs,
            opts.p2pk_locked_proof_send_mode,
            SendSplitContext {
                send_amounts: &send_amounts,
                amount,
                send_fee: send_fee.total,
                keyset_fees: &keyset_fees,
                force_swap,
                is_exact_or_offline,
            },
        )?;

        let mut proof_ys = split_result.proofs_to_swap.ys()?;
        proof_ys.extend(split_result.proofs_to_send.ys()?);

        self.wallet
            .localstore
            .reserve_proofs(proof_ys.clone(), &self.state_data.operation_id)
            .await?;

        let memo_text = opts.memo.as_ref().map(|m| m.memo.clone());
        let saga = WalletSaga::new(
            self.state_data.operation_id,
            WalletSagaState::Send(SendSagaState::ProofsReserved),
            amount,
            self.wallet.mint_url.clone(),
            self.wallet.unit.clone(),
            OperationData::Send(SendOperationData {
                amount,
                memo: memo_text.clone(),
                counter_start: None,
                counter_end: None,
                token: None,
                proofs: None,
            }),
        );

        self.wallet.localstore.add_saga(saga.clone()).await?;

        add_compensation(
            &mut self.compensations,
            Box::new(RevertProofReservation {
                localstore: self.wallet.localstore.clone(),
                proof_ys,
                saga_id: self.state_data.operation_id,
            }),
        )
        .await;

        Ok(SendSaga {
            wallet: self.wallet,
            compensations: self.compensations,
            state_data: Prepared {
                operation_id: self.state_data.operation_id,
                amount,
                options: opts,
                proofs_to_swap: split_result.proofs_to_swap,
                swap_fee: split_result.swap_fee,
                proofs_to_send: split_result.proofs_to_send,
                send_fee: send_fee.total,
                saga,
            },
        })
    }
}

impl<'a> SendSaga<'a, Prepared> {
    /// Create a new send saga directly in the Prepared state.
    ///
    /// Used when reconstructing a saga from stored state for confirmation.
    #[allow(clippy::too_many_arguments)]
    pub fn from_prepared(
        wallet: &'a Wallet,
        operation_id: uuid::Uuid,
        amount: Amount,
        options: SendOptions,
        proofs_to_swap: Proofs,
        proofs_to_send: Proofs,
        swap_fee: Amount,
        send_fee: Amount,
        saga: WalletSaga,
    ) -> Result<Self, Error> {
        if saga.id != operation_id {
            return Err(Error::Custom(format!(
                "Saga id {} does not match operation id {}",
                saga.id, operation_id
            )));
        }

        if saga.state != WalletSagaState::Send(SendSagaState::ProofsReserved) {
            return Err(Error::Custom(
                "Operation is not a prepared send".to_string(),
            ));
        }

        Ok(Self {
            wallet,
            compensations: new_compensations(),
            state_data: Prepared {
                operation_id,
                amount,
                options,
                proofs_to_swap,
                proofs_to_send,
                swap_fee,
                send_fee,
                saga,
            },
        })
    }

    /// Get the operation ID
    pub fn operation_id(&self) -> uuid::Uuid {
        self.state_data.operation_id
    }

    /// Get the amount to be sent
    pub fn amount(&self) -> Amount {
        self.state_data.amount
    }

    /// Get the send options
    pub fn options(&self) -> &SendOptions {
        &self.state_data.options
    }

    /// Get the proofs that will be swapped
    pub fn proofs_to_swap(&self) -> &Proofs {
        self.state_data.proofs_to_swap.as_ref()
    }

    /// Get the swap fee
    pub fn swap_fee(&self) -> Amount {
        self.state_data.swap_fee
    }

    /// Get the proofs that will be sent directly
    pub fn proofs_to_send(&self) -> &Proofs {
        self.state_data.proofs_to_send.as_ref()
    }

    /// Get the send fee
    pub fn send_fee(&self) -> Amount {
        self.state_data.send_fee
    }

    /// Confirm the prepared send and create a token.
    ///
    /// Performs necessary swaps, marks proofs as pending spent, creates the
    /// token, and persists the saga in TokenCreated state.
    #[instrument(skip(self), err)]
    pub async fn confirm(
        mut self,
        memo: Option<SendMemo>,
    ) -> Result<(Token, SendSaga<'a, TokenCreated>), Error> {
        let operation_id = self.state_data.operation_id;
        let amount = self.state_data.amount;
        let options = self.state_data.options.clone();
        let mut proofs_to_swap = self.state_data.proofs_to_swap.clone();
        let proofs_to_send = self.state_data.proofs_to_send.clone();
        let swap_fee = self.state_data.swap_fee;
        let send_fee = self.state_data.send_fee;

        tracing::info!("Confirming prepared send for operation {}", operation_id);

        let logic_res = async {
            let total_send_fee = swap_fee + send_fee;
            let mut final_proofs_to_send = proofs_to_send.clone();

            let total_send_amount = amount + send_fee;

            let mut counter_start = None;
            let mut counter_end = None;

            // When locked-proof passthrough is opted in, sign proofs that bypass the swap
            // before including them in the token. Signing is not optional: an unsigned
            // P2PK-locked proof in a token is unspendable by the recipient because they
            // do not hold the private key. Once signed, the proof becomes bearer — the
            // mint will accept it from anyone who presents it.
            //
            // SIG_ALL is incompatible with passthrough: the signature would need to commit
            // to the swap outputs, which do not exist at signing time. The recipient cannot
            // create valid outputs for a proof signed with SIG_ALL, so any attempt to redeem
            // it at the mint would fail. Reject early with a clear error rather than silently
            // producing an unspendable token.
            if options.p2pk_locked_proof_send_mode == P2PKLockedProofSendMode::SignAndSend {
                let sig_flag = enforce_sig_flag(final_proofs_to_send.clone()).sig_flag;
                if sig_flag == SigFlag::SigAll {
                    return Err(crate::nuts::nut11::Error::SigAllNotSupportedHere.into());
                }
                let keys = merge_keyring_keys(
                    self.wallet,
                    &final_proofs_to_send,
                    &options.p2pk_signing_keys,
                )
                .await?;
                if !keys.is_empty() {
                    crate::wallet::util::sign_proofs(&mut final_proofs_to_send, &keys)?;
                }
                verify_p2pk_proofs(&final_proofs_to_send)?;
            }

            if !proofs_to_swap.is_empty() {
                let swap_amount = total_send_amount
                    .checked_sub(final_proofs_to_send.total_amount()?)
                    .unwrap_or(Amount::ZERO);

                tracing::debug!("Swapping proofs; swap_amount={:?}", swap_amount);

                let keys =
                    merge_keyring_keys(self.wallet, &proofs_to_swap, &options.p2pk_signing_keys)
                        .await?;
                if !keys.is_empty() {
                    crate::wallet::util::sign_proofs(&mut proofs_to_swap, &keys)?;
                }

                let keyset_id = self.wallet.active_keyset().await?.id;

                // Capture counter start before swap
                counter_start = Some(
                    self.wallet
                        .localstore
                        .increment_keyset_counter(&keyset_id, 0)
                        .await?,
                );

                if let Some(swapped_proofs) = self
                    .wallet
                    .swap_no_reserve(
                        Some(swap_amount),
                        SplitTarget::None,
                        proofs_to_swap,
                        options.conditions.clone(),
                        false,
                        options.use_p2bk,
                    )
                    .await?
                {
                    final_proofs_to_send.extend(swapped_proofs);
                }

                // Capture counter end after swap
                counter_end = Some(
                    self.wallet
                        .localstore
                        .increment_keyset_counter(&keyset_id, 0)
                        .await?,
                );
            }

            if amount > final_proofs_to_send.total_amount()? {
                return Err(Error::InsufficientFunds);
            }

            self.wallet
                .localstore
                .update_proofs_state(final_proofs_to_send.ys()?, State::PendingSpent)
                .await?;

            let send_memo = options.memo.clone().or(memo);
            let token_memo =
                send_memo.and_then(|m| if m.include_memo { Some(m.memo) } else { None });

            self.wallet
                .localstore
                .add_transaction(Transaction {
                    mint_url: self.wallet.mint_url.clone(),
                    direction: TransactionDirection::Outgoing,
                    amount,
                    fee: total_send_fee,
                    unit: self.wallet.unit.clone(),
                    ys: final_proofs_to_send.ys()?,
                    timestamp: unix_time(),
                    memo: token_memo.clone(),
                    metadata: options.metadata.clone(),
                    quote_id: None,
                    payment_request: None,
                    payment_proof: None,
                    payment_method: None,
                    saga_id: Some(operation_id),
                })
                .await?;

            let token = Token::new(
                self.wallet.mint_url.clone(),
                final_proofs_to_send.clone(),
                token_memo,
                self.wallet.unit.clone(),
            );

            let mut saga = self.state_data.saga.clone();
            saga.data = OperationData::Send(SendOperationData {
                amount,
                memo: options.memo.as_ref().map(|m| m.memo.clone()),
                counter_start,
                counter_end,
                token: Some(token.to_string()),
                proofs: Some(final_proofs_to_send.clone()),
            });
            saga.update_state(WalletSagaState::Send(SendSagaState::TokenCreated));

            if !self.wallet.localstore.update_saga(saga.clone()).await? {
                return Err(Error::ConcurrentUpdate);
            }

            Ok((token, final_proofs_to_send, saga))
        }
        .await;

        match logic_res {
            Ok((token, final_proofs_to_send, saga)) => {
                let send_saga = SendSaga {
                    wallet: self.wallet,
                    compensations: self.compensations,
                    state_data: TokenCreated {
                        operation_id,
                        proofs: final_proofs_to_send,
                        saga,
                    },
                };

                Ok((token, send_saga))
            }
            Err(e) => {
                if e.is_definitive_failure() {
                    tracing::warn!(
                        "Send saga confirmation failed (definitive): {}. Running compensations.",
                        e
                    );
                    execute_compensations(&mut self.compensations).await?;
                }
                Err(e)
            }
        }
    }

    /// Cancel the prepared send and release reserved proofs
    #[instrument(skip(self))]
    pub async fn cancel(self) -> Result<(), Error> {
        let operation_id = self.state_data.operation_id;
        tracing::info!("Cancelling prepared send for operation {}", operation_id);

        let mut all_ys = self.state_data.proofs_to_swap.ys()?;
        all_ys.extend(self.state_data.proofs_to_send.ys()?);

        self.wallet
            .localstore
            .update_proofs_state(all_ys, State::Unspent)
            .await?;

        if let Err(e) = self.wallet.localstore.delete_saga(&operation_id).await {
            tracing::warn!(
                "Failed to delete send saga {}: {}. Will be cleaned up on recovery.",
                operation_id,
                e
            );
        }

        Ok(())
    }
}

impl<'a> SendSaga<'a, TokenCreated> {
    /// Revoke the sent token if not yet claimed by recipient.
    ///
    /// Swaps proofs back to the wallet. On success, the saga is completed.
    pub async fn revoke(self) -> Result<Amount, Error> {
        tracing::info!("Revoking send operation {}", self.state_data.operation_id);

        // Check with mint if proofs are still unspent. Skip local check to force mint validation.
        let states = self
            .wallet
            .check_proofs_spent(self.state_data.proofs.clone())
            .await?;

        if states.iter().any(|s| s.state == State::Spent) {
            // Already spent by recipient
            tracing::info!("Cannot revoke: token already claimed by recipient");
            // We should finalize the saga as "Spent"
            self.finalize().await?;
            return Err(Error::Custom("Token already claimed".to_string()));
        }

        // Lock saga in RollingBack state to prevent proof watcher from treating swap as recipient claim
        let operation_id = self.state_data.operation_id;
        let mut saga = self.state_data.saga.clone();
        saga.update_state(WalletSagaState::Send(SendSagaState::RollingBack));
        if let OperationData::Send(ref mut data) = saga.data {
            data.proofs = Some(self.state_data.proofs.clone());
        }

        if !self.wallet.localstore.update_saga(saga).await? {
            return Err(Error::ConcurrentUpdate);
        }

        // Swap proofs back to wallet with fresh secrets
        let swap_result = self
            .wallet
            .swap_no_reserve(
                None, // Swap all
                SplitTarget::default(),
                self.state_data.proofs.clone(),
                None,
                false,
                false,
            )
            .await;

        match swap_result {
            Ok(swapped_proofs) => {
                let amount_recovered = match swapped_proofs {
                    Some(proofs) => proofs.total_amount()?,
                    None => {
                        // All proofs kept (refreshed). Recovered amount is input minus fees.
                        let input_amount = self.state_data.proofs.total_amount()?;
                        let fee = self
                            .wallet
                            .get_proofs_fee(&self.state_data.proofs)
                            .await?
                            .total;
                        input_amount.checked_sub(fee).unwrap_or(Amount::ZERO)
                    }
                };

                self.finalize().await?;

                Ok(amount_recovered)
            }
            Err(e) => {
                tracing::error!("Revoke swap failed: {}. Reverting lock.", e);

                // Revert state to TokenCreated and mark proofs as PendingSpent to resume monitoring.
                // Fetch fresh saga from DB since earlier update succeeded.
                let current_saga = self
                    .wallet
                    .localstore
                    .get_saga(&operation_id)
                    .await?
                    .ok_or(Error::Custom("Saga not found during revert".to_string()))?;

                let mut revert_saga = current_saga;
                revert_saga.update_state(WalletSagaState::Send(SendSagaState::TokenCreated));

                self.wallet.localstore.update_saga(revert_saga).await?;

                self.wallet
                    .localstore
                    .update_proofs_state(self.state_data.proofs.ys()?, State::PendingSpent)
                    .await?;

                Err(e)
            }
        }
    }

    /// Check the status of the sent token.
    ///
    /// Finalizes and removes the saga if the token has been claimed.
    /// Returns true if claimed, false if still pending.
    pub async fn check_status(self) -> Result<bool, Error> {
        let states = self
            .wallet
            .check_proofs_spent(self.state_data.proofs.clone())
            .await?;

        let all_spent = states.iter().all(|s| s.state == State::Spent);

        if all_spent {
            tracing::info!(
                "Token for operation {} has been claimed",
                self.state_data.operation_id
            );
            self.finalize().await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Finalize the saga (delete from DB)
    async fn finalize(self) -> Result<(), Error> {
        if let Err(e) = self
            .wallet
            .localstore
            .delete_saga(&self.state_data.operation_id)
            .await
        {
            tracing::warn!(
                "Failed to delete completed send saga {}: {}",
                self.state_data.operation_id,
                e
            );
        }
        Ok(())
    }
}

impl std::fmt::Debug for SendSaga<'_, Prepared> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SendSaga<Prepared>")
            .field("operation_id", &self.state_data.operation_id)
            .field("amount", &self.state_data.amount)
            .field("options", &self.state_data.options)
            .field(
                "proofs_to_swap",
                &self
                    .state_data
                    .proofs_to_swap
                    .iter()
                    .map(|p| p.amount)
                    .collect::<Vec<_>>(),
            )
            .field("swap_fee", &self.state_data.swap_fee)
            .field(
                "proofs_to_send",
                &self
                    .state_data
                    .proofs_to_send
                    .iter()
                    .map(|p| p.amount)
                    .collect::<Vec<_>>(),
            )
            .field("send_fee", &self.state_data.send_fee)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cdk_common::amount::KeysetFeeAndAmounts;
    use cdk_common::nuts::State;
    use cdk_common::wallet::{
        KeysetLoadPolicy, OperationData, ProofInfo, SendKind, SendOperationData, SendSagaState,
        WalletSaga, WalletSagaState,
    };
    use cdk_common::{CurrencyUnit, ProofsMethods};

    use super::{ensure_selected_proofs_cover_input_fees, InputFeeCoverageContext, SendSaga};
    use crate::nuts::{Proof, SecretKey, SpendingConditions};
    use crate::wallet::send::SendOptions;
    use crate::wallet::test_utils::{
        create_test_db, create_test_wallet_with_mock, test_keyset_id, test_mint_url, test_proof,
        test_proof_info, MockMintConnector,
    };
    use crate::Amount;

    #[tokio::test]
    async fn test_send_saga_new_uses_uuid_v7_operation_id() {
        let db = create_test_db().await;
        let mock_client = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db, mock_client).await;

        let saga = SendSaga::new(&wallet);

        assert_eq!(
            saga.state_data.operation_id.get_version(),
            Some(uuid::Version::SortRand)
        );
    }

    fn keyset_fees_with_ppk(fee_ppk: u64) -> KeysetFeeAndAmounts {
        let mut fees = KeysetFeeAndAmounts::new();
        fees.insert(
            test_keyset_id(),
            (fee_ppk, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into(),
        );
        fees
    }

    fn test_p2pk_proof(keyset_id: crate::nuts::Id, amount: u64) -> Proof {
        let secret_key = SecretKey::generate();
        let spending_conditions = SpendingConditions::new_p2pk(secret_key.public_key(), None);
        let nut10_secret: crate::nuts::nut10::Secret = spending_conditions.into();
        let secret: crate::secret::Secret = nut10_secret.try_into().unwrap();
        let mut proof = test_proof(keyset_id, amount);
        proof.secret = secret;
        proof
    }

    #[tokio::test]
    async fn test_prepare_send_reserves_proofs_for_operation() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let proof_info = test_proof_info(keyset_id, 100, mint_url);
        let proof_y = proof_info.y;

        db.update_proofs(vec![proof_info], vec![]).await.unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.reset_default_mint_state();

        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let saga = SendSaga::new(&wallet);
        let prepared = saga
            .prepare(Amount::from(100), SendOptions::default())
            .await
            .unwrap();

        let reserved = db
            .get_reserved_proofs(&prepared.operation_id())
            .await
            .unwrap();
        assert_eq!(reserved.len(), 1);
        assert_eq!(reserved[0].y, proof_y);
        assert_eq!(reserved[0].state, State::Reserved);

        let stored_proofs = db.get_proofs_by_ys(vec![proof_y]).await.unwrap();
        assert_eq!(stored_proofs.len(), 1);
        assert_eq!(stored_proofs[0].state, State::Reserved);
        assert_eq!(
            stored_proofs[0].used_by_operation,
            Some(prepared.operation_id())
        );
    }

    #[tokio::test]
    async fn test_internal_prepare_reserves_only_split_proofs_for_operation() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();

        let unused_proof = test_proof(keyset_id, 4);
        let send_8_proof = test_proof(keyset_id, 8);
        let send_2_proof = test_proof(keyset_id, 2);

        let unused_info = ProofInfo::new(
            unused_proof.clone(),
            mint_url.clone(),
            State::Unspent,
            CurrencyUnit::Sat,
        )
        .unwrap();
        let send_8_info = ProofInfo::new(
            send_8_proof.clone(),
            mint_url.clone(),
            State::Unspent,
            CurrencyUnit::Sat,
        )
        .unwrap();
        let send_2_info = ProofInfo::new(
            send_2_proof.clone(),
            mint_url,
            State::Unspent,
            CurrencyUnit::Sat,
        )
        .unwrap();
        let unused_y = unused_info.y;
        let send_8_y = send_8_info.y;
        let send_2_y = send_2_info.y;

        db.update_proofs(vec![unused_info, send_8_info, send_2_info], vec![])
            .await
            .unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.reset_default_mint_state();

        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let saga = SendSaga::new(&wallet);
        let prepared = saga
            .internal_prepare(
                Amount::from(10),
                SendOptions::default(),
                vec![unused_proof, send_8_proof, send_2_proof],
                false,
                KeysetLoadPolicy::default(),
            )
            .await
            .unwrap();

        let reserved = db
            .get_reserved_proofs(&prepared.operation_id())
            .await
            .unwrap();
        let mut reserved_amounts = reserved
            .iter()
            .map(|proof| proof.proof.amount)
            .collect::<Vec<_>>();
        reserved_amounts.sort();
        assert_eq!(reserved_amounts, vec![Amount::from(2), Amount::from(8)]);

        let stored_proofs = db
            .get_proofs_by_ys(vec![unused_y, send_8_y, send_2_y])
            .await
            .unwrap();
        assert_eq!(stored_proofs.len(), 3);

        for stored in stored_proofs {
            match stored.proof.amount {
                amount if amount == Amount::from(4) => {
                    assert_eq!(stored.state, State::Unspent);
                    assert_eq!(stored.used_by_operation, None);
                }
                amount if amount == Amount::from(8) || amount == Amount::from(2) => {
                    assert_eq!(stored.state, State::Reserved);
                    assert_eq!(stored.used_by_operation, Some(prepared.operation_id()));
                }
                amount => panic!("unexpected proof amount: {amount}"),
            }
        }
    }

    #[tokio::test]
    async fn test_cancel_send_rejects_token_created_saga() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let saga_id = uuid::Uuid::new_v4();

        let proof_info = test_proof_info(keyset_id, 100, mint_url.clone());
        let proof_y = proof_info.y;
        let proof = proof_info.proof.clone();
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();
        db.reserve_proofs(vec![proof_y], &saga_id).await.unwrap();
        db.update_proofs_state(vec![proof_y], State::PendingSpent)
            .await
            .unwrap();

        let saga_record = WalletSaga::new(
            saga_id,
            WalletSagaState::Send(SendSagaState::TokenCreated),
            Amount::from(100),
            mint_url,
            CurrencyUnit::Sat,
            OperationData::Send(SendOperationData {
                amount: Amount::from(100),
                memo: None,
                counter_start: None,
                counter_end: None,
                token: Some("cashuA...".to_string()),
                proofs: Some(vec![proof.clone()]),
            }),
        );
        db.add_saga(saga_record).await.unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.reset_default_mint_state();
        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;

        let err = wallet
            .cancel_send(saga_id, vec![], vec![proof])
            .await
            .expect_err("cancel_send must reject a token-created send saga");

        assert!(matches!(err, crate::Error::Custom(_)));

        let after = db.get_proofs_by_ys(vec![proof_y]).await.unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].state, State::PendingSpent);
        assert!(db.get_saga(&saga_id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn test_offline_send_excludes_locked_proofs_without_passthrough() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let proof = test_p2pk_proof(keyset_id, 8);
        let proof_info =
            ProofInfo::new(proof, mint_url, State::Unspent, CurrencyUnit::Sat).unwrap();

        db.update_proofs(vec![proof_info], vec![]).await.unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.reset_default_mint_state();

        let wallet = create_test_wallet_with_mock(db, mock_client).await;

        // Prime the keyset cache — offline sends use CacheOnly and require
        // keysets to already be cached.
        wallet
            .keysets(cdk_common::wallet::KeysetLoadPolicy::Refresh)
            .await
            .unwrap();

        let saga = SendSaga::new(&wallet).with_keyset_policy(KeysetLoadPolicy::CacheOnly);
        let err = saga
            .prepare(
                Amount::from(8),
                SendOptions {
                    send_kind: SendKind::OfflineExact,
                    ..Default::default()
                },
            )
            .await
            .expect_err("offline send must not prepare a locked-proof swap by default");

        assert!(matches!(err, crate::Error::InsufficientFunds));
    }

    /// Offline send with an empty keyset cache must fail because it uses
    /// CacheOnly and never contacts the network.
    #[tokio::test]
    async fn test_offline_send_fails_without_cached_keysets() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();

        let proof_info = test_proof_info(keyset_id, 64, mint_url);
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db, mock_client).await;

        // Do NOT prime the cache — offline send should fail with UnknownKeySet
        // because CacheOnly has nothing to return.
        let saga = SendSaga::new(&wallet).with_keyset_policy(KeysetLoadPolicy::CacheOnly);
        let err = saga
            .prepare(
                Amount::from(64),
                SendOptions {
                    send_kind: SendKind::OfflineExact,
                    ..Default::default()
                },
            )
            .await
            .expect_err("offline send without cached keysets should fail");

        assert!(
            matches!(err, crate::Error::UnknownKeySet),
            "expected UnknownKeySet, got: {err:?}"
        );
    }

    /// Offline send with a primed keyset cache succeeds without network access.
    #[tokio::test]
    async fn test_offline_send_succeeds_with_cached_keysets() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();

        let proof_info = test_proof_info(keyset_id, 64, mint_url);
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db, mock_client).await;

        // Prime the cache so CacheOnly can serve keyset data
        wallet
            .keysets(cdk_common::wallet::KeysetLoadPolicy::Refresh)
            .await
            .unwrap();

        let saga = SendSaga::new(&wallet).with_keyset_policy(KeysetLoadPolicy::CacheOnly);
        let prepared = saga
            .prepare(
                Amount::from(64),
                SendOptions {
                    send_kind: SendKind::OfflineExact,
                    ..Default::default()
                },
            )
            .await;

        assert!(
            prepared.is_ok(),
            "offline send with cached keysets should succeed"
        );
    }

    /// Online send loads keysets from network when cache is empty.
    #[tokio::test]
    async fn test_online_send_loads_keysets_from_network() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();

        let proof_info = test_proof_info(keyset_id, 64, mint_url);
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.set_post_swap_response(Ok(cdk_common::SwapResponse { signatures: vec![] }));
        let wallet = create_test_wallet_with_mock(db, mock_client).await;

        // Do NOT prime the cache — online send should fetch from the mock
        let saga = SendSaga::new(&wallet);
        let prepared = saga
            .prepare(
                Amount::from(64),
                SendOptions {
                    send_kind: SendKind::OnlineExact,
                    ..Default::default()
                },
            )
            .await;

        assert!(
            prepared.is_ok(),
            "online send should succeed by fetching keysets from network"
        );
    }

    #[test]
    fn test_ensure_selected_proofs_cover_input_fees_adds_remaining_proofs() {
        let keyset_id = test_keyset_id();
        let selected_proofs = vec![
            test_proof(keyset_id, 32),
            test_proof(keyset_id, 16),
            test_proof(keyset_id, 8),
            test_proof(keyset_id, 4),
            test_proof(keyset_id, 2),
            test_proof(keyset_id, 1),
        ];
        let mut proof_pool = selected_proofs.clone();
        proof_pool.push(test_proof(keyset_id, 8));
        let active_keyset_ids = vec![keyset_id];
        let keyset_fees = keyset_fees_with_ppk(1000);
        let send_amounts = vec![Amount::from(63)];

        let selected = ensure_selected_proofs_cover_input_fees(
            selected_proofs,
            proof_pool,
            InputFeeCoverageContext {
                amount: Amount::from(63),
                send_fee: Amount::ZERO,
                active_keyset_ids: &active_keyset_ids,
                keyset_fees: &keyset_fees,
                send_amounts: &send_amounts,
                force_swap: true,
                is_exact_or_offline: false,
            },
        )
        .unwrap();

        assert_eq!(selected.total_amount().unwrap(), Amount::from(71));
        assert_eq!(selected.len(), 7);
    }

    #[test]
    fn test_ensure_selected_proofs_cover_input_fees_errors_when_short() {
        let keyset_id = test_keyset_id();
        let selected_proofs = vec![
            test_proof(keyset_id, 32),
            test_proof(keyset_id, 16),
            test_proof(keyset_id, 8),
            test_proof(keyset_id, 4),
            test_proof(keyset_id, 2),
            test_proof(keyset_id, 1),
        ];
        let active_keyset_ids = vec![keyset_id];
        let keyset_fees = keyset_fees_with_ppk(1000);
        let send_amounts = vec![Amount::from(63)];

        let err = ensure_selected_proofs_cover_input_fees(
            selected_proofs.clone(),
            selected_proofs,
            InputFeeCoverageContext {
                amount: Amount::from(63),
                send_fee: Amount::ZERO,
                active_keyset_ids: &active_keyset_ids,
                keyset_fees: &keyset_fees,
                send_amounts: &send_amounts,
                force_swap: true,
                is_exact_or_offline: false,
            },
        )
        .expect_err("selected proofs cannot cover input fees without extra proofs");

        assert!(matches!(err, crate::Error::InsufficientFunds));
    }

    #[test]
    fn test_ensure_selected_proofs_only_charges_actual_swap_inputs() {
        let keyset_id = test_keyset_id();
        let selected_proofs = vec![
            test_p2pk_proof(keyset_id, 8),
            test_proof(keyset_id, 2),
            test_proof(keyset_id, 2),
        ];
        let active_keyset_ids = vec![keyset_id];
        let keyset_fees = keyset_fees_with_ppk(1000);
        let send_amounts = vec![Amount::from(8), Amount::from(2)];

        let selected = ensure_selected_proofs_cover_input_fees(
            selected_proofs.clone(),
            selected_proofs,
            InputFeeCoverageContext {
                amount: Amount::from(10),
                send_fee: Amount::ZERO,
                active_keyset_ids: &active_keyset_ids,
                keyset_fees: &keyset_fees,
                send_amounts: &send_amounts,
                force_swap: false,
                is_exact_or_offline: false,
            },
        )
        .unwrap();

        assert_eq!(selected.total_amount().unwrap(), Amount::from(12));
        assert_eq!(selected.len(), 3);
    }
}
