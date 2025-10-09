use cdk_common::database::mint::MeltRequestInfo;
use cdk_common::mint::MeltQuote;
use cdk_common::{database, Amount, BlindSignature, PublicKey};
use tracing::instrument;

use crate::mint::Mint;
use crate::Error;

pub struct ChangeProcessor<'a> {
    mint: &'a Mint,
}

impl<'a> ChangeProcessor<'a> {
    pub fn new(mint: &'a Mint) -> Self {
        Self { mint }
    }

    #[instrument(skip_all)]
    pub async fn calculate_and_sign_change(
        &self,
        quote: &MeltQuote,
        total_spent: Amount,
        mut tx: Box<dyn database::MintTransaction<'a, database::Error> + Send + Sync + 'a>,
    ) -> Result<
        (
            Option<Vec<BlindSignature>>,
            Box<dyn database::MintTransaction<'a, database::Error> + Send + Sync + 'a>,
        ),
        Error,
    > {
        let MeltRequestInfo {
            inputs_amount,
            inputs_fee,
            change_outputs,
        } = tx
            .get_melt_request_and_blinded_messages(&quote.id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        if inputs_amount <= total_spent {
            tracing::debug!("No change required for melt {}", quote.id);
            return Ok((None, tx));
        }

        if change_outputs.is_empty() {
            tracing::info!(
                "Inputs for {} {} greater than spent on melt {} but change outputs not provided.",
                quote.id,
                inputs_amount,
                total_spent
            );
            return Ok((None, tx));
        }

        let change_target = inputs_amount - total_spent - inputs_fee;

        let fee_and_amounts = self
            .mint
            .keysets
            .load()
            .iter()
            .filter_map(|keyset| {
                if keyset.active && Some(keyset.id) == change_outputs.first().map(|x| x.keyset_id) {
                    Some((keyset.input_fee_ppk, keyset.amounts.clone()).into())
                } else {
                    None
                }
            })
            .next()
            .unwrap_or_else(|| (0, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into());

        let mut amounts = change_target.split(&fee_and_amounts);

        if change_outputs.len() < amounts.len() {
            tracing::debug!(
                "Providing change requires {} blinded messages, but only {} provided",
                amounts.len(),
                change_outputs.len()
            );

            amounts.sort_by(|a, b| b.cmp(a));
        }

        let mut blinded_messages = vec![];

        for (amount, mut blinded_message) in amounts.iter().zip(change_outputs.clone()) {
            blinded_message.amount = *amount;
            blinded_messages.push(blinded_message);
        }

        // Commit the transaction before the external blind_sign call
        // We don't want to hold a transaction open during a potentially blocking external call
        tx.commit().await?;

        // External call that can block - no transaction held here
        let change_sigs = self.mint.blind_sign(blinded_messages).await?;

        // Create a new transaction to add the blind signatures
        let mut new_tx = self.mint.localstore.begin_transaction().await?;

        new_tx
            .add_blind_signatures(
                &change_outputs[0..change_sigs.len()]
                    .iter()
                    .map(|o| o.blinded_secret)
                    .collect::<Vec<PublicKey>>(),
                &change_sigs,
                Some(quote.id.clone()),
            )
            .await?;

        Ok((Some(change_sigs), new_tx))
    }
}
