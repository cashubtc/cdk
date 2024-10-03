use tracing::instrument;

use crate::{
    amount::SplitTarget,
    nuts::{Proofs, PublicKey, SpendingConditions, State, Token},
    Amount, Error, Wallet,
};

use super::{proofs::SelectProofsMethod, SendKind};

impl Wallet {
    /// Send specific proofs
    #[instrument(skip(self))]
    pub async fn send_proofs(&self, memo: Option<String>, proofs: Proofs) -> Result<Token, Error> {
        let ys = proofs
            .iter()
            .map(|p| p.y())
            .collect::<Result<Vec<PublicKey>, _>>()?;
        self.localstore.reserve_proofs(ys).await?;

        Ok(Token::new(
            self.mint_url.clone(),
            proofs,
            memo,
            Some(self.unit),
        ))
    }

    /// Send
    #[instrument(skip(self))]
    pub async fn send(
        &self,
        amount: Amount,
        memo: Option<String>,
        conditions: Option<SpendingConditions>,
        amount_split_target: &SplitTarget,
        send_kind: &SendKind,
        include_fees: bool,
    ) -> Result<Token, Error> {
        // If online send check mint for current keysets fees
        if matches!(
            send_kind,
            SendKind::OnlineExact | SendKind::OnlineTolerance(_)
        ) {
            if let Err(e) = self.get_active_mint_keyset().await {
                tracing::error!(
                    "Error fetching active mint keyset: {:?}. Using stored keysets",
                    e
                );
            }
        }

        let mint_url = &self.mint_url;
        let unit = &self.unit;
        let available_proofs = self
            .localstore
            .get_proofs(
                Some(mint_url.clone()),
                Some(*unit),
                Some(vec![State::Unspent]),
                conditions.clone().map(|c| vec![c]),
            )
            .await?;

        let (available_proofs, proofs_sum) = available_proofs.into_iter().map(|p| p.proof).fold(
            (Vec::new(), Amount::ZERO),
            |(mut acc1, mut acc2), p| {
                acc2 += p.amount;
                acc1.push(p);
                (acc1, acc2)
            },
        );
        let available_proofs = if proofs_sum < amount {
            match &conditions {
                Some(conditions) => {
                    let available_proofs = self
                        .localstore
                        .get_proofs(
                            Some(mint_url.clone()),
                            Some(*unit),
                            Some(vec![State::Unspent]),
                            None,
                        )
                        .await?;

                    let available_proofs = available_proofs.into_iter().map(|p| p.proof).collect();

                    let proofs_to_swap = self
                        .select_proofs_to_swap(
                            amount,
                            available_proofs,
                            SelectProofsMethod::LargestFirst,
                        )
                        .await?;

                    let proofs_with_conditions = self
                        .swap(
                            Some(amount),
                            SplitTarget::default(),
                            proofs_to_swap,
                            Some(conditions.clone()),
                            include_fees,
                        )
                        .await?;
                    proofs_with_conditions.ok_or(Error::InsufficientFunds)?
                }
                None => {
                    return Err(Error::InsufficientFunds);
                }
            }
        } else {
            available_proofs
        };

        let selected = self
            .select_proofs_to_send(amount, available_proofs, include_fees)
            .await;

        let send_proofs: Proofs = match (send_kind, selected, conditions.clone()) {
            // Handle exact matches offline
            (SendKind::OfflineExact, Ok(selected_proofs), _) => {
                let selected_proofs_amount =
                    Amount::try_sum(selected_proofs.iter().map(|p| p.amount))?;

                let amount_to_send = match include_fees {
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
                let selected_proofs_amount =
                    Amount::try_sum(selected_proofs.iter().map(|p| p.amount))?;

                let amount_to_send = match include_fees {
                    true => amount + self.get_proofs_fee(&selected_proofs).await?,
                    false => amount,
                };

                if selected_proofs_amount == amount_to_send {
                    selected_proofs
                } else {
                    tracing::info!("Could not select proofs exact while offline.");
                    tracing::info!("Attempting to select proofs and swapping");

                    self.swap_from_unspent(amount, conditions, include_fees)
                        .await?
                }
            }

            // Handle offline tolerance
            (SendKind::OfflineTolerance(tolerance), Ok(selected_proofs), _) => {
                let selected_proofs_amount =
                    Amount::try_sum(selected_proofs.iter().map(|p| p.amount))?;

                let amount_to_send = match include_fees {
                    true => amount + self.get_proofs_fee(&selected_proofs).await?,
                    false => amount,
                };
                if selected_proofs_amount - amount_to_send <= *tolerance {
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

                self.swap_from_unspent(amount, conditions, include_fees)
                    .await?
            }

            // Handle online tolerance with successful selection
            (SendKind::OnlineTolerance(tolerance), Ok(selected_proofs), _) => {
                let selected_proofs_amount =
                    Amount::try_sum(selected_proofs.iter().map(|p| p.amount))?;
                let amount_to_send = match include_fees {
                    true => amount + self.get_proofs_fee(&selected_proofs).await?,
                    false => amount,
                };
                if selected_proofs_amount - amount_to_send <= *tolerance {
                    selected_proofs
                } else {
                    tracing::info!("Could not select proofs while offline. Attempting swap");
                    self.swap_from_unspent(amount, conditions, include_fees)
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

        self.send_proofs(memo, send_proofs).await
    }
}
