//! Quotes database implementation

use std::str::FromStr;

use async_trait::async_trait;
use cdk_common::database::mint::LockedMeltQuotes;
use cdk_common::database::{
    self, Acquired, ConversionError, Error, MintQuotesDatabase, MintQuotesTransaction,
};
use cdk_common::mint::{
    self, IncomingPayment, Issuance, MeltPaymentRequest, MeltQuote, MintQuote, Operation,
};
use cdk_common::payment::PaymentIdentifier;
use cdk_common::quote_id::QuoteId;
use cdk_common::state::check_melt_quote_state_transition;
use cdk_common::util::unix_time;
use cdk_common::{
    Amount, BlindedMessage, CurrencyUnit, Id, MeltQuoteState, PaymentMethod, PublicKey,
};
#[cfg(feature = "prometheus")]
use cdk_prometheus::METRICS;
use lightning_invoice::Bolt11Invoice;
use tracing::instrument;

use super::{SQLMintDatabase, SQLTransaction};
use crate::database::DatabaseExecutor;
use crate::pool::DatabasePool;
use crate::stmt::{query, Column};
use crate::{
    column_as_nullable_number, column_as_nullable_string, column_as_number, column_as_string,
    unpack_into,
};

async fn get_mint_quote_payments<C>(
    conn: &C,
    quote_id: &QuoteId,
) -> Result<Vec<IncomingPayment>, Error>
where
    C: DatabaseExecutor + Send + Sync,
{
    // Get payment IDs and timestamps from the mint_quote_payments table
    query(
        r#"
        SELECT
            payment_id,
            timestamp,
            amount
        FROM
            mint_quote_payments
        WHERE
            quote_id=:quote_id
        "#,
    )?
    .bind("quote_id", quote_id.to_string())
    .fetch_all(conn)
    .await?
    .into_iter()
    .map(|row| {
        let amount: u64 = column_as_number!(row[2].clone());
        let time: u64 = column_as_number!(row[1].clone());
        Ok(IncomingPayment::new(
            amount.into(),
            column_as_string!(&row[0]),
            time,
        ))
    })
    .collect()
}

async fn get_mint_quote_issuance<C>(conn: &C, quote_id: &QuoteId) -> Result<Vec<Issuance>, Error>
where
    C: DatabaseExecutor + Send + Sync,
{
    // Get payment IDs and timestamps from the mint_quote_payments table
    query(
        r#"
SELECT amount, timestamp
FROM mint_quote_issued
WHERE quote_id=:quote_id
            "#,
    )?
    .bind("quote_id", quote_id.to_string())
    .fetch_all(conn)
    .await?
    .into_iter()
    .map(|row| {
        let time: u64 = column_as_number!(row[1].clone());
        Ok(Issuance::new(
            Amount::from_i64(column_as_number!(row[0].clone()))
                .expect("Is amount when put into db"),
            time,
        ))
    })
    .collect()
}

// Inline helper functions that work with both connections and transactions
pub(super) async fn get_mint_quote_inner<T>(
    executor: &T,
    quote_id: &QuoteId,
    for_update: bool,
) -> Result<Option<MintQuote>, Error>
where
    T: DatabaseExecutor,
{
    let payments = get_mint_quote_payments(executor, quote_id).await?;
    let issuance = get_mint_quote_issuance(executor, quote_id).await?;

    let for_update_clause = if for_update { "FOR UPDATE" } else { "" };
    let query_str = format!(
        r#"
        SELECT
            id,
            amount,
            unit,
            request,
            expiry,
            request_lookup_id,
            pubkey,
            created_time,
            amount_paid,
            amount_issued,
            payment_method,
            request_lookup_id_kind
        FROM
            mint_quote
        WHERE id = :id
        {for_update_clause}
        "#
    );

    query(&query_str)?
        .bind("id", quote_id.to_string())
        .fetch_one(executor)
        .await?
        .map(|row| sql_row_to_mint_quote(row, payments, issuance))
        .transpose()
}

pub(super) async fn get_mint_quote_by_request_inner<T>(
    executor: &T,
    request: &str,
    for_update: bool,
) -> Result<Option<MintQuote>, Error>
where
    T: DatabaseExecutor,
{
    let for_update_clause = if for_update { "FOR UPDATE" } else { "" };
    let query_str = format!(
        r#"
        SELECT
            id,
            amount,
            unit,
            request,
            expiry,
            request_lookup_id,
            pubkey,
            created_time,
            amount_paid,
            amount_issued,
            payment_method,
            request_lookup_id_kind
        FROM
            mint_quote
        WHERE request = :request
        {for_update_clause}
        "#
    );

    let mut mint_quote = query(&query_str)?
        .bind("request", request.to_string())
        .fetch_one(executor)
        .await?
        .map(|row| sql_row_to_mint_quote(row, vec![], vec![]))
        .transpose()?;

    if let Some(quote) = mint_quote.as_mut() {
        let payments = get_mint_quote_payments(executor, &quote.id).await?;
        let issuance = get_mint_quote_issuance(executor, &quote.id).await?;
        quote.issuance = issuance;
        quote.payments = payments;
    }

    Ok(mint_quote)
}

pub(super) async fn get_mint_quote_by_request_lookup_id_inner<T>(
    executor: &T,
    request_lookup_id: &PaymentIdentifier,
    for_update: bool,
) -> Result<Option<MintQuote>, Error>
where
    T: DatabaseExecutor,
{
    let for_update_clause = if for_update { "FOR UPDATE" } else { "" };
    let query_str = format!(
        r#"
        SELECT
            id,
            amount,
            unit,
            request,
            expiry,
            request_lookup_id,
            pubkey,
            created_time,
            amount_paid,
            amount_issued,
            payment_method,
            request_lookup_id_kind
        FROM
            mint_quote
        WHERE request_lookup_id = :request_lookup_id
        AND request_lookup_id_kind = :request_lookup_id_kind
        {for_update_clause}
        "#
    );

    let mut mint_quote = query(&query_str)?
        .bind("request_lookup_id", request_lookup_id.to_string())
        .bind("request_lookup_id_kind", request_lookup_id.kind())
        .fetch_one(executor)
        .await?
        .map(|row| sql_row_to_mint_quote(row, vec![], vec![]))
        .transpose()?;

    if let Some(quote) = mint_quote.as_mut() {
        let payments = get_mint_quote_payments(executor, &quote.id).await?;
        let issuance = get_mint_quote_issuance(executor, &quote.id).await?;
        quote.issuance = issuance;
        quote.payments = payments;
    }

    Ok(mint_quote)
}

pub(super) async fn get_melt_quote_inner<T>(
    executor: &T,
    quote_id: &QuoteId,
    for_update: bool,
) -> Result<Option<mint::MeltQuote>, Error>
where
    T: DatabaseExecutor,
{
    let for_update_clause = if for_update { "FOR UPDATE" } else { "" };
    let query_str = format!(
        r#"
        SELECT
            id,
            unit,
            amount,
            request,
            fee_reserve,
            expiry,
            state,
            payment_preimage,
            request_lookup_id,
            created_time,
            paid_time,
            payment_method,
            options,
            request_lookup_id_kind
        FROM
            melt_quote
        WHERE
            id=:id
        {for_update_clause}
        "#
    );

    query(&query_str)?
        .bind("id", quote_id.to_string())
        .fetch_one(executor)
        .await?
        .map(sql_row_to_melt_quote)
        .transpose()
}

pub(super) async fn get_melt_quotes_by_request_lookup_id_inner<T>(
    executor: &T,
    request_lookup_id: &PaymentIdentifier,
    for_update: bool,
) -> Result<Vec<mint::MeltQuote>, Error>
where
    T: DatabaseExecutor,
{
    let for_update_clause = if for_update { "FOR UPDATE" } else { "" };
    let query_str = format!(
        r#"
        SELECT
            id,
            unit,
            amount,
            request,
            fee_reserve,
            expiry,
            state,
            payment_preimage,
            request_lookup_id,
            created_time,
            paid_time,
            payment_method,
            options,
            request_lookup_id_kind
        FROM
            melt_quote
        WHERE
            request_lookup_id = :request_lookup_id
            AND request_lookup_id_kind = :request_lookup_id_kind
        {for_update_clause}
        "#
    );

    query(&query_str)?
        .bind("request_lookup_id", request_lookup_id.to_string())
        .bind("request_lookup_id_kind", request_lookup_id.kind())
        .fetch_all(executor)
        .await?
        .into_iter()
        .map(sql_row_to_melt_quote)
        .collect::<Result<Vec<_>, _>>()
}

/// Locks a melt quote and all related quotes atomically to prevent deadlocks.
///
/// This function acquires all locks in a single query with consistent ordering (by ID),
/// preventing the circular wait condition that can occur when locks are acquired in
/// separate queries.
async fn lock_melt_quote_and_related_inner<T>(
    executor: &T,
    quote_id: &QuoteId,
) -> Result<LockedMeltQuotes, Error>
where
    T: DatabaseExecutor,
{
    // Use a single query with subquery to atomically lock:
    // 1. All quotes with the same request_lookup_id as the target quote, OR
    // 2. Just the target quote if it has no request_lookup_id
    //
    // The ORDER BY ensures consistent lock acquisition order across transactions,
    // preventing deadlocks.
    let query_str = r#"
        SELECT
            id,
            unit,
            amount,
            request,
            fee_reserve,
            expiry,
            state,
            payment_preimage,
            request_lookup_id,
            created_time,
            paid_time,
            payment_method,
            options,
            request_lookup_id_kind
        FROM
            melt_quote
        WHERE
            (
                request_lookup_id IS NOT NULL
                AND request_lookup_id = (SELECT request_lookup_id FROM melt_quote WHERE id = :quote_id)
                AND request_lookup_id_kind = (SELECT request_lookup_id_kind FROM melt_quote WHERE id = :quote_id)
            )
            OR
            (
                id = :quote_id
                AND (SELECT request_lookup_id FROM melt_quote WHERE id = :quote_id) IS NULL
            )
        ORDER BY id
        FOR UPDATE
        "#;

    let all_quotes: Vec<mint::MeltQuote> = query(query_str)?
        .bind("quote_id", quote_id.to_string())
        .fetch_all(executor)
        .await?
        .into_iter()
        .map(sql_row_to_melt_quote)
        .collect::<Result<Vec<_>, _>>()?;

    // Find the target quote from the locked set
    let target_quote = all_quotes.iter().find(|q| &q.id == quote_id).cloned();

    Ok(LockedMeltQuotes {
        target: target_quote.map(|q| q.into()),
        all_related: all_quotes.into_iter().map(|q| q.into()).collect(),
    })
}

#[instrument(skip_all)]
fn sql_row_to_mint_quote(
    row: Vec<Column>,
    payments: Vec<IncomingPayment>,
    issueances: Vec<Issuance>,
) -> Result<MintQuote, Error> {
    unpack_into!(
        let (
            id, amount, unit, request, expiry, request_lookup_id,
            pubkey, created_time, amount_paid, amount_issued, payment_method, request_lookup_id_kind
        ) = row
    );

    let request_str = column_as_string!(&request);
    let request_lookup_id = column_as_nullable_string!(&request_lookup_id).unwrap_or_else(|| {
        Bolt11Invoice::from_str(&request_str)
            .map(|invoice| invoice.payment_hash().to_string())
            .unwrap_or_else(|_| request_str.clone())
    });
    let request_lookup_id_kind = column_as_string!(request_lookup_id_kind);

    let pubkey = column_as_nullable_string!(&pubkey)
        .map(|pk| PublicKey::from_hex(&pk))
        .transpose()?;

    let id = column_as_string!(id);
    let amount: Option<u64> = column_as_nullable_number!(amount);
    let amount_paid: u64 = column_as_number!(amount_paid);
    let amount_issued: u64 = column_as_number!(amount_issued);
    let payment_method = column_as_string!(payment_method, PaymentMethod::from_str);

    Ok(MintQuote::new(
        Some(QuoteId::from_str(&id)?),
        request_str,
        column_as_string!(unit, CurrencyUnit::from_str),
        amount.map(Amount::from),
        column_as_number!(expiry),
        PaymentIdentifier::new(&request_lookup_id_kind, &request_lookup_id)
            .map_err(|_| ConversionError::MissingParameter("Payment id".to_string()))?,
        pubkey,
        amount_paid.into(),
        amount_issued.into(),
        payment_method,
        column_as_number!(created_time),
        payments,
        issueances,
    ))
}

// FIXME: Replace unwrap with proper error handling
fn sql_row_to_melt_quote(row: Vec<Column>) -> Result<mint::MeltQuote, Error> {
    unpack_into!(
        let (
                id,
                unit,
                amount,
                request,
                fee_reserve,
                expiry,
                state,
                payment_preimage,
                request_lookup_id,
                created_time,
                paid_time,
                payment_method,
                options,
                request_lookup_id_kind
        ) = row
    );

    let id = column_as_string!(id);
    let amount: u64 = column_as_number!(amount);
    let fee_reserve: u64 = column_as_number!(fee_reserve);

    let expiry = column_as_number!(expiry);
    let payment_preimage = column_as_nullable_string!(payment_preimage);
    let options = column_as_nullable_string!(options);
    let options = options.and_then(|o| serde_json::from_str(&o).ok());
    let created_time: i64 = column_as_number!(created_time);
    let paid_time = column_as_nullable_number!(paid_time);
    let payment_method = PaymentMethod::from_str(&column_as_string!(payment_method))?;

    let state =
        MeltQuoteState::from_str(&column_as_string!(&state)).map_err(ConversionError::from)?;

    let unit = column_as_string!(unit);
    let request = column_as_string!(request);

    let request_lookup_id_kind = column_as_nullable_string!(request_lookup_id_kind);

    let request_lookup_id = column_as_nullable_string!(&request_lookup_id).or_else(|| {
        Bolt11Invoice::from_str(&request)
            .ok()
            .map(|invoice| invoice.payment_hash().to_string())
    });

    let request_lookup_id = if let (Some(id_kind), Some(request_lookup_id)) =
        (request_lookup_id_kind, request_lookup_id)
    {
        Some(
            PaymentIdentifier::new(&id_kind, &request_lookup_id)
                .map_err(|_| ConversionError::MissingParameter("Payment id".to_string()))?,
        )
    } else {
        None
    };

    let request = match serde_json::from_str(&request) {
        Ok(req) => req,
        Err(err) => {
            tracing::debug!(
                "Melt quote from pre migrations defaulting to bolt11 {}.",
                err
            );
            let bolt11 = Bolt11Invoice::from_str(&request)
                .map_err(|e| Error::Internal(format!("Could not parse invoice: {e}")))?;
            MeltPaymentRequest::Bolt11 { bolt11 }
        }
    };

    Ok(MeltQuote {
        id: QuoteId::from_str(&id)?,
        unit: CurrencyUnit::from_str(&unit)?,
        amount: Amount::from(amount),
        request,
        fee_reserve: Amount::from(fee_reserve),
        state,
        expiry,
        payment_preimage,
        request_lookup_id,
        options,
        created_time: created_time as u64,
        paid_time,
        payment_method,
    })
}

#[async_trait]
impl<RM> MintQuotesTransaction for SQLTransaction<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn add_melt_request(
        &mut self,
        quote_id: &QuoteId,
        inputs_amount: Amount,
        inputs_fee: Amount,
    ) -> Result<(), Self::Err> {
        // Insert melt_request
        query(
            r#"
            INSERT INTO melt_request
            (quote_id, inputs_amount, inputs_fee)
            VALUES
            (:quote_id, :inputs_amount, :inputs_fee)
            "#,
        )?
        .bind("quote_id", quote_id.to_string())
        .bind("inputs_amount", inputs_amount.to_i64())
        .bind("inputs_fee", inputs_fee.to_i64())
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn add_blinded_messages(
        &mut self,
        quote_id: Option<&QuoteId>,
        blinded_messages: &[BlindedMessage],
        operation: &Operation,
    ) -> Result<(), Self::Err> {
        let current_time = unix_time();

        // Insert blinded_messages directly into blind_signature with c = NULL
        // Let the database constraint handle duplicate detection
        for message in blinded_messages {
            match query(
                r#"
                INSERT INTO blind_signature
                (blinded_message, amount, keyset_id, c, quote_id, created_time, operation_kind, operation_id)
                VALUES
                (:blinded_message, :amount, :keyset_id, NULL, :quote_id, :created_time, :operation_kind, :operation_id)
                "#,
            )?
            .bind(
                "blinded_message",
                message.blinded_secret.to_bytes().to_vec(),
            )
            .bind("amount", message.amount.to_i64())
            .bind("keyset_id", message.keyset_id.to_string())
            .bind("quote_id", quote_id.map(|q| q.to_string()))
            .bind("created_time", current_time as i64)
            .bind("operation_kind", operation.kind().to_string())
            .bind("operation_id", operation.id().to_string())
            .execute(&self.inner)
            .await
            {
                Ok(_) => continue,
                Err(database::Error::Duplicate) => {
                    // Primary key constraint violation - blinded message already exists
                    // This could be either:
                    // 1. Already signed (c IS NOT NULL) - definitely an error
                    // 2. Already pending (c IS NULL) - also an error
                    return Err(database::Error::Duplicate);
                }
                Err(err) => return Err(err),
            }
        }

        Ok(())
    }

    async fn delete_blinded_messages(
        &mut self,
        blinded_secrets: &[PublicKey],
    ) -> Result<(), Self::Err> {
        if blinded_secrets.is_empty() {
            return Ok(());
        }

        // Delete blinded messages from blind_signature table where c IS NULL
        // (only delete unsigned blinded messages)
        query(
            r#"
            DELETE FROM blind_signature
            WHERE blinded_message IN (:blinded_secrets) AND c IS NULL
            "#,
        )?
        .bind_vec(
            "blinded_secrets",
            blinded_secrets
                .iter()
                .map(|secret| secret.to_bytes().to_vec())
                .collect(),
        )
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn get_melt_request_and_blinded_messages(
        &mut self,
        quote_id: &QuoteId,
    ) -> Result<Option<database::mint::MeltRequestInfo>, Self::Err> {
        let melt_request_row = query(
            r#"
            SELECT inputs_amount, inputs_fee
            FROM melt_request
            WHERE quote_id = :quote_id
            FOR UPDATE
            "#,
        )?
        .bind("quote_id", quote_id.to_string())
        .fetch_one(&self.inner)
        .await?;

        if let Some(row) = melt_request_row {
            let inputs_amount: u64 = column_as_number!(row[0].clone());
            let inputs_fee: u64 = column_as_number!(row[1].clone());

            // Get blinded messages from blind_signature table where c IS NULL
            let blinded_messages_rows = query(
                r#"
                SELECT blinded_message, keyset_id, amount
                FROM blind_signature
                WHERE quote_id = :quote_id AND c IS NULL
                "#,
            )?
            .bind("quote_id", quote_id.to_string())
            .fetch_all(&self.inner)
            .await?;

            let blinded_messages: Result<Vec<BlindedMessage>, Error> = blinded_messages_rows
                .into_iter()
                .map(|row| -> Result<BlindedMessage, Error> {
                    let blinded_message_key =
                        column_as_string!(&row[0], PublicKey::from_hex, PublicKey::from_slice);
                    let keyset_id = column_as_string!(&row[1], Id::from_str, Id::from_bytes);
                    let amount: u64 = column_as_number!(row[2].clone());

                    Ok(BlindedMessage {
                        blinded_secret: blinded_message_key,
                        keyset_id,
                        amount: Amount::from(amount),
                        witness: None, // Not storing witness in database currently
                    })
                })
                .collect();
            let blinded_messages = blinded_messages?;

            Ok(Some(database::mint::MeltRequestInfo {
                inputs_amount: Amount::from(inputs_amount),
                inputs_fee: Amount::from(inputs_fee),
                change_outputs: blinded_messages,
            }))
        } else {
            Ok(None)
        }
    }

    async fn delete_melt_request(&mut self, quote_id: &QuoteId) -> Result<(), Self::Err> {
        // Delete from melt_request table
        query(
            r#"
            DELETE FROM melt_request
            WHERE quote_id = :quote_id
            "#,
        )?
        .bind("quote_id", quote_id.to_string())
        .execute(&self.inner)
        .await?;

        // Also delete blinded messages (where c IS NULL) from blind_signature table
        query(
            r#"
            DELETE FROM blind_signature
            WHERE quote_id = :quote_id AND c IS NULL
            "#,
        )?
        .bind("quote_id", quote_id.to_string())
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn update_mint_quote(
        &mut self,
        quote: &mut Acquired<mint::MintQuote>,
    ) -> Result<(), Self::Err> {
        let mut changes = if let Some(changes) = quote.take_changes() {
            changes
        } else {
            return Ok(());
        };

        if changes.issuances.is_none() && changes.payments.is_none() {
            return Ok(());
        }

        for payment in changes.payments.take().unwrap_or_default() {
            query(
                r#"
                INSERT INTO mint_quote_payments
                (quote_id, payment_id, amount, timestamp)
                VALUES (:quote_id, :payment_id, :amount, :timestamp)
                "#,
            )?
            .bind("quote_id", quote.id.to_string())
            .bind("payment_id", payment.payment_id)
            .bind("amount", payment.amount.to_i64())
            .bind("timestamp", payment.time as i64)
            .execute(&self.inner)
            .await
            .map_err(|err| {
                tracing::error!("SQLite could not insert payment ID: {}", err);
                err
            })?;
        }

        let current_time = unix_time();

        for amount_issued in changes.issuances.take().unwrap_or_default() {
            query(
                r#"
                INSERT INTO mint_quote_issued
                (quote_id, amount, timestamp)
                VALUES (:quote_id, :amount, :timestamp);
                "#,
            )?
            .bind("quote_id", quote.id.to_string())
            .bind("amount", amount_issued.to_i64())
            .bind("timestamp", current_time as i64)
            .execute(&self.inner)
            .await?;
        }

        query(
            r#"
            UPDATE
                mint_quote
            SET
                amount_issued = :amount_issued,
                amount_paid = :amount_paid
            WHERE
                id = :quote_id
            "#,
        )?
        .bind("quote_id", quote.id.to_string())
        .bind("amount_issued", quote.amount_issued().to_i64())
        .bind("amount_paid", quote.amount_paid().to_i64())
        .execute(&self.inner)
        .await
        .inspect_err(|err| {
            tracing::error!("SQLite could not update mint quote amount_paid: {}", err);
        })?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn add_mint_quote(&mut self, quote: MintQuote) -> Result<Acquired<MintQuote>, Self::Err> {
        query(
            r#"
                INSERT INTO mint_quote (
                id, amount, unit, request, expiry, request_lookup_id, pubkey, created_time, payment_method, request_lookup_id_kind
                )
                VALUES (
                :id, :amount, :unit, :request, :expiry, :request_lookup_id, :pubkey, :created_time, :payment_method, :request_lookup_id_kind
                )
            "#,
        )?
        .bind("id", quote.id.to_string())
        .bind("amount", quote.amount.map(|a| a.to_i64()))
        .bind("unit", quote.unit.to_string())
        .bind("request", quote.request.clone())
        .bind("expiry", quote.expiry as i64)
        .bind(
            "request_lookup_id",
            quote.request_lookup_id.to_string(),
        )
        .bind("pubkey", quote.pubkey.map(|p| p.to_string()))
        .bind("created_time", quote.created_time as i64)
        .bind("payment_method", quote.payment_method.to_string())
        .bind("request_lookup_id_kind", quote.request_lookup_id.kind())
        .execute(&self.inner)
        .await?;

        Ok(quote.into())
    }

    async fn add_melt_quote(&mut self, quote: mint::MeltQuote) -> Result<(), Self::Err> {
        // Now insert the new quote
        query(
            r#"
            INSERT INTO melt_quote
            (
                id, unit, amount, request, fee_reserve, state,
                expiry, payment_preimage, request_lookup_id,
                created_time, paid_time, options, request_lookup_id_kind, payment_method
            )
            VALUES
            (
                :id, :unit, :amount, :request, :fee_reserve, :state,
                :expiry, :payment_preimage, :request_lookup_id,
                :created_time, :paid_time, :options, :request_lookup_id_kind, :payment_method
            )
        "#,
        )?
        .bind("id", quote.id.to_string())
        .bind("unit", quote.unit.to_string())
        .bind("amount", quote.amount.to_i64())
        .bind("request", serde_json::to_string(&quote.request)?)
        .bind("fee_reserve", quote.fee_reserve.to_i64())
        .bind("state", quote.state.to_string())
        .bind("expiry", quote.expiry as i64)
        .bind("payment_preimage", quote.payment_preimage)
        .bind(
            "request_lookup_id",
            quote.request_lookup_id.as_ref().map(|id| id.to_string()),
        )
        .bind("created_time", quote.created_time as i64)
        .bind("paid_time", quote.paid_time.map(|t| t as i64))
        .bind(
            "options",
            quote.options.map(|o| serde_json::to_string(&o).ok()),
        )
        .bind(
            "request_lookup_id_kind",
            quote.request_lookup_id.map(|id| id.kind()),
        )
        .bind("payment_method", quote.payment_method.to_string())
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn update_melt_quote_request_lookup_id(
        &mut self,
        quote: &mut Acquired<mint::MeltQuote>,
        new_request_lookup_id: &PaymentIdentifier,
    ) -> Result<(), Self::Err> {
        query(r#"UPDATE melt_quote SET request_lookup_id = :new_req_id, request_lookup_id_kind = :new_kind WHERE id = :id"#)?
            .bind("new_req_id", new_request_lookup_id.to_string())
            .bind("new_kind", new_request_lookup_id.kind())
            .bind("id", quote.id.to_string())
            .execute(&self.inner)
            .await?;
        quote.request_lookup_id = Some(new_request_lookup_id.clone());
        Ok(())
    }

    async fn update_melt_quote_state(
        &mut self,
        quote: &mut Acquired<mint::MeltQuote>,
        state: MeltQuoteState,
        payment_proof: Option<String>,
    ) -> Result<MeltQuoteState, Self::Err> {
        let old_state = quote.state;

        check_melt_quote_state_transition(old_state, state)?;

        let rec = if state == MeltQuoteState::Paid {
            let current_time = unix_time();
            quote.paid_time = Some(current_time);
            quote.payment_preimage = payment_proof.clone();
            query(r#"UPDATE melt_quote SET state = :state, paid_time = :paid_time, payment_preimage = :payment_preimage WHERE id = :id"#)?
                .bind("state", state.to_string())
                .bind("paid_time", current_time as i64)
                .bind("payment_preimage", payment_proof)
                .bind("id", quote.id.to_string())
                .execute(&self.inner)
                .await
        } else {
            query(r#"UPDATE melt_quote SET state = :state WHERE id = :id"#)?
                .bind("state", state.to_string())
                .bind("id", quote.id.to_string())
                .execute(&self.inner)
                .await
        };

        match rec {
            Ok(_) => {}
            Err(err) => {
                tracing::error!("SQLite Could not update melt quote");
                return Err(err);
            }
        };

        quote.state = state;

        if state == MeltQuoteState::Unpaid || state == MeltQuoteState::Failed {
            self.delete_melt_request(&quote.id).await?;
        }

        Ok(old_state)
    }

    async fn get_mint_quote(
        &mut self,
        quote_id: &QuoteId,
    ) -> Result<Option<Acquired<MintQuote>>, Self::Err> {
        get_mint_quote_inner(&self.inner, quote_id, true)
            .await
            .map(|quote| quote.map(|inner| inner.into()))
    }

    async fn get_melt_quote(
        &mut self,
        quote_id: &QuoteId,
    ) -> Result<Option<Acquired<mint::MeltQuote>>, Self::Err> {
        get_melt_quote_inner(&self.inner, quote_id, true)
            .await
            .map(|quote| quote.map(|inner| inner.into()))
    }

    async fn get_melt_quotes_by_request_lookup_id(
        &mut self,
        request_lookup_id: &PaymentIdentifier,
    ) -> Result<Vec<Acquired<mint::MeltQuote>>, Self::Err> {
        get_melt_quotes_by_request_lookup_id_inner(&self.inner, request_lookup_id, true)
            .await
            .map(|quote| quote.into_iter().map(|inner| inner.into()).collect())
    }

    async fn lock_melt_quote_and_related(
        &mut self,
        quote_id: &QuoteId,
    ) -> Result<LockedMeltQuotes, Self::Err> {
        lock_melt_quote_and_related_inner(&self.inner, quote_id).await
    }

    async fn get_mint_quote_by_request(
        &mut self,
        request: &str,
    ) -> Result<Option<Acquired<MintQuote>>, Self::Err> {
        get_mint_quote_by_request_inner(&self.inner, request, true)
            .await
            .map(|quote| quote.map(|inner| inner.into()))
    }

    async fn get_mint_quote_by_request_lookup_id(
        &mut self,
        request_lookup_id: &PaymentIdentifier,
    ) -> Result<Option<Acquired<MintQuote>>, Self::Err> {
        get_mint_quote_by_request_lookup_id_inner(&self.inner, request_lookup_id, true)
            .await
            .map(|quote| quote.map(|inner| inner.into()))
    }
}

#[async_trait]
impl<RM> MintQuotesDatabase for SQLMintDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn get_mint_quote(&self, quote_id: &QuoteId) -> Result<Option<MintQuote>, Self::Err> {
        #[cfg(feature = "prometheus")]
        METRICS.inc_in_flight_requests("get_mint_quote");

        #[cfg(feature = "prometheus")]
        let start_time = std::time::Instant::now();
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        let result = get_mint_quote_inner(&*conn, quote_id, false).await;

        #[cfg(feature = "prometheus")]
        {
            let success = result.is_ok();

            METRICS.record_mint_operation("get_mint_quote", success);
            METRICS.record_mint_operation_histogram(
                "get_mint_quote",
                success,
                start_time.elapsed().as_secs_f64(),
            );
            METRICS.dec_in_flight_requests("get_mint_quote");
        }

        result
    }

    async fn get_mint_quote_by_request(
        &self,
        request: &str,
    ) -> Result<Option<MintQuote>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        get_mint_quote_by_request_inner(&*conn, request, false).await
    }

    async fn get_mint_quote_by_request_lookup_id(
        &self,
        request_lookup_id: &PaymentIdentifier,
    ) -> Result<Option<MintQuote>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        get_mint_quote_by_request_lookup_id_inner(&*conn, request_lookup_id, false).await
    }

    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        let mut mint_quotes = query(
            r#"
            SELECT
                id,
                amount,
                unit,
                request,
                expiry,
                request_lookup_id,
                pubkey,
                created_time,
                amount_paid,
                amount_issued,
                payment_method,
                request_lookup_id_kind
            FROM
                mint_quote
            "#,
        )?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(|row| sql_row_to_mint_quote(row, vec![], vec![]))
        .collect::<Result<Vec<_>, _>>()?;

        for quote in mint_quotes.as_mut_slice() {
            let payments = get_mint_quote_payments(&*conn, &quote.id).await?;
            let issuance = get_mint_quote_issuance(&*conn, &quote.id).await?;
            quote.issuance = issuance;
            quote.payments = payments;
        }

        Ok(mint_quotes)
    }

    async fn get_melt_quote(
        &self,
        quote_id: &QuoteId,
    ) -> Result<Option<mint::MeltQuote>, Self::Err> {
        #[cfg(feature = "prometheus")]
        METRICS.inc_in_flight_requests("get_melt_quote");

        #[cfg(feature = "prometheus")]
        let start_time = std::time::Instant::now();
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        let result = get_melt_quote_inner(&*conn, quote_id, false).await;

        #[cfg(feature = "prometheus")]
        {
            let success = result.is_ok();

            METRICS.record_mint_operation("get_melt_quote", success);
            METRICS.record_mint_operation_histogram(
                "get_melt_quote",
                success,
                start_time.elapsed().as_secs_f64(),
            );
            METRICS.dec_in_flight_requests("get_melt_quote");
        }

        result
    }

    async fn get_melt_quotes(&self) -> Result<Vec<mint::MeltQuote>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        Ok(query(
            r#"
            SELECT
                id,
                unit,
                amount,
                request,
                fee_reserve,
                expiry,
                state,
                payment_preimage,
                request_lookup_id,
                created_time,
                paid_time,
                payment_method,
                options,
                request_lookup_id_kind
            FROM
                melt_quote
            "#,
        )?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_melt_quote)
        .collect::<Result<Vec<_>, _>>()?)
    }
}
