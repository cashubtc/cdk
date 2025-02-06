use std::collections::HashMap;

use cdk_common::database::{Transaction, TransactionDirection};
use cdk_common::util::unix_time;
use tracing::instrument;

use super::SendKind;
use crate::amount::SplitTarget;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::{Proofs, SpendingConditions, State, Token};
use crate::{Amount, Error, Wallet};

impl Wallet {
    /// Send specific proofs
    #[instrument(skip(self))]
    pub async fn send_proofs(&self, proofs: Proofs, opts: SendOptions) -> Result<Token, Error> {
        let ys = proofs.ys()?;
        self.proof_db.reserve_proofs(ys.clone()).await?;
        self.transaction_db
            .add_transaction(Transaction {
                amount: proofs.total_amount()?,
                direction: TransactionDirection::Outgoing,
                fee: Amount::ZERO, // TODO track this?
                mint_url: self.mint_url.clone(),
                timestamp: unix_time(),
                unit: self.unit.clone(),
                ys,
                memo: opts.memo.clone(),
                metadata: opts.metadata,
            })
            .await?;

        Ok(Token::new(
            self.mint_url.clone(),
            proofs,
            opts.memo,
            self.unit.clone(),
        ))
    }

    /// Send
    #[instrument(skip(self))]
    pub async fn send(&self, amount: Amount, opts: SendOptions) -> Result<Token, Error> {
        // If online send check mint for current keysets fees
        if matches!(
            opts.send_kind,
            SendKind::OnlineExact | SendKind::OnlineTolerance(_)
        ) {
            if let Err(e) = self.get_active_mint_keyset().await {
                tracing::error!(
                    "Error fetching active mint keyset: {:?}. Using stored keysets",
                    e
                );
            }
        }

        let available_proofs = self
            .get_proofs_with(
                Some(vec![State::Unspent]),
                opts.spending_conditions.clone().map(|c| vec![c]),
            )
            .await?;

        let proofs_sum = available_proofs.total_amount()?;

        let available_proofs = if proofs_sum < amount {
            match &opts.spending_conditions {
                Some(conditions) => {
                    tracing::debug!("Insufficient prrofs matching conditions attempting swap");
                    let unspent_proofs = self.get_unspent_proofs().await?;
                    let proofs_to_swap = self.select_proofs_to_swap(amount, unspent_proofs).await?;

                    // TODO determine fees of this action
                    let proofs_with_conditions = self
                        .swap(
                            Some(amount),
                            SplitTarget::default(),
                            proofs_to_swap,
                            Some(conditions.clone()),
                            opts.include_fees,
                        )
                        .await?;
                    proofs_with_conditions.ok_or(Error::InsufficientFunds)
                }
                None => Err(Error::InsufficientFunds),
            }?
        } else {
            available_proofs
        };

        let selected = self
            .select_proofs_to_send(amount, available_proofs, opts.include_fees)
            .await;

        let send_proofs: Proofs = match (opts.send_kind, selected, opts.spending_conditions.clone())
        {
            // Handle exact matches offline
            (SendKind::OfflineExact, Ok(selected_proofs), _) => {
                let selected_proofs_amount = selected_proofs.total_amount()?;

                let amount_to_send = match opts.include_fees {
                    true => amount + self.get_proofs_fee(&selected_proofs).await?,
                    false => amount,
                };

                if selected_proofs_amount == amount_to_send {
                    selected_proofs
                } else {
                    return Err(Error::InsufficientFunds);
                }
            }

            // Handle exact matches
            (SendKind::OnlineExact, Ok(selected_proofs), _) => {
                let selected_proofs_amount = selected_proofs.total_amount()?;

                let amount_to_send = match opts.include_fees {
                    true => amount + self.get_proofs_fee(&selected_proofs).await?,
                    false => amount,
                };

                if selected_proofs_amount == amount_to_send {
                    selected_proofs
                } else {
                    tracing::info!("Could not select proofs exact while offline.");
                    tracing::info!("Attempting to select proofs and swapping");

                    self.swap_from_unspent(
                        amount,
                        opts.spending_conditions.clone(),
                        opts.include_fees,
                    )
                    .await?
                }
            }

            // Handle offline tolerance
            (SendKind::OfflineTolerance(tolerance), Ok(selected_proofs), _) => {
                let selected_proofs_amount = selected_proofs.total_amount()?;

                let amount_to_send = match opts.include_fees {
                    true => amount + self.get_proofs_fee(&selected_proofs).await?,
                    false => amount,
                };
                if selected_proofs_amount - amount_to_send <= tolerance {
                    selected_proofs
                } else {
                    tracing::info!("Selected proofs greater than tolerance. Must swap online");
                    return Err(Error::InsufficientFunds);
                }
            }

            // Handle online tolerance when selection fails and conditions are present
            (SendKind::OnlineTolerance(_), Err(_), Some(_)) => {
                tracing::info!("Could not select proofs with conditions while offline.");
                tracing::info!("Attempting to select proofs without conditions and swapping");

                self.swap_from_unspent(amount, opts.spending_conditions.clone(), opts.include_fees)
                    .await?
            }

            // Handle online tolerance with successful selection
            (SendKind::OnlineTolerance(tolerance), Ok(selected_proofs), _) => {
                let selected_proofs_amount = selected_proofs.total_amount()?;
                let amount_to_send = match opts.include_fees {
                    true => amount + self.get_proofs_fee(&selected_proofs).await?,
                    false => amount,
                };
                if selected_proofs_amount - amount_to_send <= tolerance {
                    selected_proofs
                } else {
                    tracing::info!("Could not select proofs while offline. Attempting swap");
                    self.swap_from_unspent(
                        amount,
                        opts.spending_conditions.clone(),
                        opts.include_fees,
                    )
                    .await?
                }
            }

            // Handle all other cases where selection fails
            (
                SendKind::OfflineExact
                | SendKind::OnlineExact
                | SendKind::OfflineTolerance(_)
                | SendKind::OnlineTolerance(_),
                Err(_),
                _,
            ) => {
                tracing::debug!("Could not select proofs");
                return Err(Error::InsufficientFunds);
            }
        };

        self.send_proofs(send_proofs, opts).await
    }
}

/// Send Options
#[derive(Debug, Clone, Default)]
pub struct SendOptions {
    /// Include Fees
    pub include_fees: bool,
    /// Memo
    pub memo: Option<String>,
    /// User-defined Metadata
    pub metadata: HashMap<String, String>,
    /// Send Kind
    pub send_kind: SendKind,
    /// Spending Conditions
    pub spending_conditions: Option<SpendingConditions>,
    /// Split Target
    pub split_target: SplitTarget,
}
