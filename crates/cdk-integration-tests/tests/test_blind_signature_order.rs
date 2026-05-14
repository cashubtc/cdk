use std::str::FromStr;

use anyhow::Result;
use cdk_common::mint::{MeltPaymentRequest, MeltQuote, Operation, OperationKind};
use cdk_common::quote_id::QuoteId;
use cdk_common::{Amount, BlindedMessage, CurrencyUnit, Id, PaymentMethod, SecretKey};
use cdk_integration_tests::init_pure_tests::*;
use uuid::Uuid;

#[tokio::test]
async fn test_blind_signature_order_in_db() -> Result<()> {
    setup_tracing();
    let mint = create_and_start_test_mint().await?;
    let db = mint.localstore();

    let quote_id = QuoteId::from_str("00000000-0000-0000-0000-000000000000").unwrap();

    let msg1 = BlindedMessage {
        blinded_secret: SecretKey::generate().public_key(),
        keyset_id: Id::from_str("001711afb1de20cb").unwrap(),
        amount: Amount::from(10),
        witness: None,
    };
    let msg2 = BlindedMessage {
        blinded_secret: SecretKey::generate().public_key(),
        keyset_id: Id::from_str("001711afb1de20cb").unwrap(),
        amount: Amount::from(20),
        witness: None,
    };

    let messages = vec![msg1.clone(), msg2.clone()];

    let operation = Operation::new(
        Uuid::new_v4(),
        OperationKind::Melt,
        Amount::ZERO,
        Amount::ZERO,
        Amount::ZERO,
        None,
        None,
    );

    let mut tx = db.begin_transaction().await?;

    let quote = MeltQuote::new(
        Some(quote_id.clone()),
        MeltPaymentRequest::Custom {
            method: "test".to_string(),
            request: "test".to_string(),
        },
        CurrencyUnit::Sat,
        Amount::from(30).with_unit(CurrencyUnit::Sat),
        Amount::ZERO.with_unit(CurrencyUnit::Sat),
        0,
        None,
        None,
        PaymentMethod::Custom("test".to_string()),
        None,
        None,
    );
    tx.add_melt_quote(quote).await?;
    tx.add_blinded_messages(Some(&quote_id), &messages, &operation)
        .await?;

    tx.add_melt_request(
        &quote_id,
        Amount::from(30).with_unit(CurrencyUnit::Sat),
        Amount::from(0).with_unit(CurrencyUnit::Sat),
    )
    .await?;

    tx.commit().await?;

    let mut tx2 = db.begin_transaction().await?;
    let info = tx2
        .get_melt_request_and_blinded_messages(&quote_id)
        .await?
        .expect("Should find melt request");

    assert_eq!(info.change_outputs.len(), 2);
    assert_eq!(info.change_outputs[0].blinded_secret, msg1.blinded_secret);
    assert_eq!(info.change_outputs[1].blinded_secret, msg2.blinded_secret);

    Ok(())
}
