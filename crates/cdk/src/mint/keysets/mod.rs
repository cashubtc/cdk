use std::str::FromStr;

use cdk_common::mint::OperationKind;
use cdk_common::QuoteId;
use cdk_signatory::signatory::RotateKeyArguments;
use tracing::instrument;

use super::{
    CurrencyUnit, Id, KeySet, KeySetInfo, KeysResponse, KeysetResponse, Mint, MintKeySetInfo,
};
use crate::Error;

mod auth;

impl Mint {
    async fn ensure_no_pending_melt_change_outputs(
        &self,
        unit: &CurrencyUnit,
    ) -> Result<(), Error> {
        let sagas = self.localstore.get_incomplete_sagas(OperationKind::Melt).await?;

        for saga in sagas {
            let Some(quote_id) = saga.quote_id.as_ref() else {
                continue;
            };
            let quote_id = QuoteId::from_str(quote_id)
                .map_err(|err| Error::Custom(format!("Invalid melt saga quote id: {err}")))?;
            let mut tx = self.localstore.begin_transaction().await?;
            let request_info = match tx.get_melt_request_and_blinded_messages(&quote_id).await {
                Ok(request_info) => {
                    tx.rollback().await?;
                    request_info
                }
                Err(err) => {
                    tx.rollback().await?;
                    return Err(err.into());
                }
            };

            let Some(request_info) = request_info else {
                continue;
            };

            let has_change_for_unit = request_info.change_outputs.iter().any(|output| {
                self.get_keyset_info(&output.keyset_id)
                    .is_some_and(|keyset| &keyset.unit == unit)
            });

            if has_change_for_unit {
                return Err(Error::Custom(format!(
                    "Cannot rotate keyset for unit {unit}: melt quote {quote_id} has pending change outputs"
                )));
            }
        }

        Ok(())
    }

    /// Retrieve the public keys of the active keyset for distribution to wallet
    /// clients
    #[instrument(skip(self))]
    pub fn keyset_pubkeys(&self, keyset_id: &Id) -> Result<KeysResponse, Error> {
        self.keysets
            .load()
            .iter()
            .find(|keyset| &keyset.id == keyset_id)
            .ok_or(Error::UnknownKeySet)
            .map(|key| KeysResponse {
                keysets: vec![key.into()],
            })
    }

    /// Retrieve the public keys of the active keyset for distribution to wallet
    /// clients
    #[instrument(skip_all)]
    pub fn pubkeys(&self) -> KeysResponse {
        KeysResponse {
            keysets: self
                .keysets
                .load()
                .iter()
                .filter(|keyset| keyset.active && keyset.unit != CurrencyUnit::Auth)
                .map(|key| key.into())
                .collect::<Vec<_>>(),
        }
    }

    /// Return a list of all supported keysets
    #[instrument(skip_all)]
    pub fn keysets(&self) -> KeysetResponse {
        KeysetResponse {
            keysets: self
                .keysets
                .load()
                .iter()
                .filter(|k| k.unit != CurrencyUnit::Auth)
                .map(|k| KeySetInfo {
                    id: k.id,
                    unit: k.unit.clone(),
                    active: k.active,
                    input_fee_ppk: k.input_fee_ppk,
                    final_expiry: k.final_expiry,
                })
                .collect(),
        }
    }

    /// Get keysets
    #[instrument(skip(self))]
    pub fn keyset(&self, id: &Id) -> Option<KeySet> {
        self.keysets
            .load()
            .iter()
            .find(|key| &key.id == id)
            .map(|x| x.into())
    }

    /// Add current keyset to inactive keysets
    /// Generate new keyset
    #[instrument(skip(self))]
    pub async fn rotate_keyset(
        &self,
        unit: CurrencyUnit,
        amounts: Vec<u64>,
        input_fee_ppk: u64,
        use_keyset_v2: bool,
        final_expiry: Option<u64>,
    ) -> Result<MintKeySetInfo, Error> {
        self.ensure_no_pending_melt_change_outputs(&unit).await?;

        let result = self
            .signatory
            .rotate_keyset(RotateKeyArguments {
                unit,
                amounts,
                input_fee_ppk,
                keyset_id_type: if use_keyset_v2 {
                    cdk_common::nut02::KeySetVersion::Version01
                } else {
                    cdk_common::nut02::KeySetVersion::Version00
                },
                final_expiry,
            })
            .await?;

        let new_keyset = self.signatory.keysets().await?;
        self.keysets.store(new_keyset.keysets.into());

        Ok(result.into())
    }
}

#[cfg(test)]
mod tests {
    use cdk_common::melt::MeltQuoteRequest;
    use cdk_common::nut00::KnownMethod;
    use cdk_common::nuts::{MeltQuoteBolt11Request, MeltQuoteState, MeltRequest};
    use cdk_common::{Amount, PaymentMethod};
    use cdk_fake_wallet::{create_fake_invoice, FakeInvoiceDescription};

    use crate::mint::melt::melt_saga::MeltSaga;
    use crate::test_helpers::mint::{
        create_test_blinded_messages, create_test_mint, mint_test_proofs,
    };
    use crate::CurrencyUnit;

    #[tokio::test]
    async fn rotate_keyset_rejects_pending_melt_change_outputs() {
        let mint = create_test_mint().await.expect("mint");
        let proofs = mint_test_proofs(&mint, Amount::from(10_000))
            .await
            .expect("proofs");
        let (change_outputs, _) = create_test_blinded_messages(&mint, Amount::from(1_000))
            .await
            .expect("change outputs");

        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Paid,
            check_payment_state: MeltQuoteState::Paid,
            pay_err: false,
            check_err: false,
        };
        let invoice = create_fake_invoice(
            9_000,
            serde_json::to_string(&fake_description).expect("fake invoice description"),
        );
        let quote_response = mint
            .get_melt_quote(MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
                request: invoice,
                unit: CurrencyUnit::Sat,
                options: None,
            }))
            .await
            .expect("melt quote");

        let request = MeltRequest::new(quote_response.quote, proofs, Some(change_outputs));
        let verification = mint
            .verify_inputs(request.inputs())
            .await
            .expect("input verification");
        let saga = MeltSaga::new(
            std::sync::Arc::new(mint.clone()),
            mint.localstore(),
            mint.pubsub_manager(),
        );
        let _setup = saga
            .setup_melt(
                &request,
                verification,
                PaymentMethod::Known(KnownMethod::Bolt11),
            )
            .await
            .expect("setup melt");

        let err = mint
            .rotate_keyset(CurrencyUnit::Sat, vec![1], 0, true, None)
            .await
            .expect_err("rotation should be blocked");

        assert!(
            err.to_string().contains("pending change outputs"),
            "unexpected error: {err}"
        );
    }
}
