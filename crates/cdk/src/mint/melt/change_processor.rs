use cdk_common::database::MintTransaction;
use cdk_common::mint::MeltQuote;
use cdk_common::{Amount, BlindSignature, BlindedMessage, PublicKey};
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
        mut tx: Box<dyn MintTransaction<'_, cdk_common::database::Error> + Send + Sync + '_>,
        quote: &MeltQuote,
        inputs_amount: Amount,
        inputs_fee: Amount,
        total_spent: Amount,
        outputs: Vec<BlindedMessage>,
    ) -> Result<Option<Vec<BlindSignature>>, Error> {
        if inputs_amount <= total_spent {
            tracing::debug!("No change required for melt {}", quote.id);
            return Ok(None);
        }

        if outputs.is_empty() {
            tracing::info!(
                "Inputs for {} {} greater than spent on melt {} but change outputs not provided.",
                quote.id,
                inputs_amount,
                total_spent
            );
            return Ok(None);
        }

        let blinded_messages: Vec<PublicKey> = outputs.iter().map(|b| b.blinded_secret).collect();

        if tx
            .get_blind_signatures(&blinded_messages)
            .await?
            .iter()
            .flatten()
            .next()
            .is_some()
        {
            tracing::info!("Output has already been signed");
            return Err(Error::BlindedMessageAlreadySigned);
        }

        let change_target = inputs_amount - total_spent - inputs_fee;

        let fee_and_amounts = self
            .mint
            .keysets
            .load()
            .iter()
            .filter_map(|keyset| {
                if keyset.active && Some(keyset.id) == outputs.first().map(|x| x.keyset_id) {
                    Some((keyset.input_fee_ppk, keyset.amounts.clone()).into())
                } else {
                    None
                }
            })
            .next()
            .unwrap_or_else(|| (0, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into());

        let mut amounts = change_target.split(&fee_and_amounts);

        if outputs.len() < amounts.len() {
            tracing::debug!(
                "Providing change requires {} blinded messages, but only {} provided",
                amounts.len(),
                outputs.len()
            );

            amounts.sort_by(|a, b| b.cmp(a));
        }

        let mut blinded_messages = vec![];

        for (amount, mut blinded_message) in amounts.iter().zip(outputs.clone()) {
            blinded_message.amount = *amount;
            blinded_messages.push(blinded_message);
        }

        tx.commit().await?;

        let change_sigs = self.mint.blind_sign(blinded_messages).await?;

        let mut tx = self.mint.localstore.begin_transaction().await?;

        tx.add_blind_signatures(
            &outputs[0..change_sigs.len()]
                .iter()
                .map(|o| o.blinded_secret)
                .collect::<Vec<PublicKey>>(),
            &change_sigs,
            Some(quote.id.clone()),
        )
        .await?;

        tx.commit().await?;

        Ok(Some(change_sigs))
    }
}
