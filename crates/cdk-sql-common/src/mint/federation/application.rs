use super::*;

#[async_trait]
impl<RM> FederationApplicationTransaction for SQLTransaction<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn add_federation_payment_observation(
        &mut self,
        observation: &FederationPaymentObservationRecord,
        terminal_operation_index: Option<u64>,
    ) -> Result<(), Self::Err> {
        insert_payment_observation(&self.inner, observation, terminal_operation_index).await
    }

    async fn retire_federation_payment_observations(
        &mut self,
        context: FederationPaymentObservationContext,
        quote: &str,
        operation_index: u64,
    ) -> Result<u64, Self::Err> {
        let rows = query(
            r#"
            UPDATE federation_payment_observations
            SET terminal_operation_index = :operation_index
            WHERE context = :context
              AND quote = :quote
              AND terminal_operation_index IS NULL
            "#,
        )?
        .bind("operation_index", operation_index as i64)
        .bind("context", payment_observation_context_to_str(context))
        .bind("quote", quote)
        .execute(&self.inner)
        .await?;
        Ok(u64::try_from(rows).unwrap_or(u64::MAX))
    }

    async fn settle_federation_application_quote(
        &mut self,
        quote: &str,
        operation_index: u64,
    ) -> Result<(), Self::Err> {
        #[cfg(feature = "federation-bark")]
        {
            super::bark_projection::settle_quote_action(&self.inner, quote, operation_index).await
        }
        #[cfg(not(feature = "federation-bark"))]
        {
            let _ = (quote, operation_index);
            Ok(())
        }
    }

    async fn add_federation_signature_shares(
        &mut self,
        shares: &[FederationSignatureShareRecord],
    ) -> Result<(), Self::Err> {
        for share in shares {
            insert_signature_share(&self.inner, share).await?;
        }
        Ok(())
    }

    async fn get_federation_payment_observations(
        &mut self,
        selection: FederationPaymentObservationSelection<'_>,
    ) -> Result<Vec<FederationStoredPaymentObservation>, Self::Err> {
        payment_observations(&self.inner, selection).await
    }

    async fn get_federation_materialized_state_source(
        &mut self,
    ) -> Result<cdk_common::database::mint::FederationMaterializedStateSource, Self::Err> {
        super::state_transfer::get_materialized_state_source(&self.inner).await
    }

    async fn add_federation_materialized_state_keys(
        &mut self,
        records: &[FederationMaterializedStateKeyRecord],
    ) -> Result<(), Self::Err> {
        super::state_transfer::add_materialized_state_keys(&self.inner, records).await
    }

    async fn install_federation_application_rows(
        &mut self,
        rows: &[cdk_common::federation::FederationMaterializedStateRow],
    ) -> Result<(), Self::Err> {
        super::state_transfer::install_application_rows(&self.inner, rows).await
    }

    async fn apply_federation_application_operation(
        &mut self,
        entry: &FederationJournalEntry,
        operation: &FederationApplicationOperation,
    ) -> Result<(), Self::Err> {
        #[cfg(feature = "federation-bark")]
        {
            let bark =
                cdk_federation_bark::validate_federation_bark_application_operation(operation)?;
            return super::bark_projection::apply_bark_operation(&self.inner, entry, &bark).await;
        }
        #[cfg(not(feature = "federation-bark"))]
        {
            let _ = (entry, operation);
            Err(
                cdk_common::federation::FederationError::ApplicationOperationInvalid {
                    reason: "federation application support is disabled",
                }
                .into(),
            )
        }
    }
}

#[async_trait]
impl<RM> FederationApplicationDatabase for SQLMintDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn get_federation_payment_observations(
        &self,
        selection: FederationPaymentObservationSelection<'_>,
    ) -> Result<Vec<FederationStoredPaymentObservation>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        payment_observations(&*conn, selection).await
    }

    async fn get_federation_materialized_state_keys(
        &self,
        kind: cdk_common::federation::FederationMaterializedStateKind,
    ) -> Result<Vec<FederationMaterializedStateKeyRecord>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT state_kind, state_key, operation_index, operation_id, record_json
            FROM federation_materialized_state_keys
            WHERE state_kind = :state_kind
            ORDER BY state_key ASC
            "#,
        )?
        .bind("state_kind", kind.label())
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_materialized_state_key)
        .collect()
    }

    async fn get_federation_melt_quotes(
        &self,
        quote_ids: &[QuoteId],
    ) -> Result<Vec<cdk_common::mint::MeltQuote>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        get_federation_melt_quotes_inner(&*conn, quote_ids).await
    }

    async fn get_federation_materialized_state_source(
        &self,
    ) -> Result<cdk_common::database::mint::FederationMaterializedStateSource, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        super::state_transfer::get_materialized_state_source(&*conn).await
    }

    async fn get_federation_signature_shares(
        &self,
        key: FederationSignatureShareKey,
    ) -> Result<Vec<FederationSignatureShareRecord>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        let rows = match key {
            FederationSignatureShareKey::Output {
                operation_id,
                output_index,
            } => {
                query(
                    r#"
                    SELECT share_json
                    FROM federation_signature_shares
                    WHERE operation_id = :operation_id
                      AND output_index = :output_index
                    ORDER BY member_id ASC
                    "#,
                )?
                .bind("operation_id", operation_id.to_string())
                .bind("output_index", i64::from(output_index))
                .fetch_all(&*conn)
                .await?
            }
            FederationSignatureShareKey::BlindedMessage(blinded_message) => {
                query(
                    r#"
                    SELECT share_json
                    FROM federation_signature_shares
                    WHERE blinded_message = :blinded_message
                    ORDER BY operation_id ASC, output_index ASC, member_id ASC
                    "#,
                )?
                .bind("blinded_message", blinded_message.to_bytes())
                .fetch_all(&*conn)
                .await?
            }
        };
        rows.into_iter()
            .map(sql_row_to_federation_signature_share_record)
            .collect()
    }
}

async fn insert_payment_observation<C>(
    connection: &C,
    observation: &FederationPaymentObservationRecord,
    terminal_operation_index: Option<u64>,
) -> Result<(), Error>
where
    C: DatabaseExecutor,
{
    query(
        r#"
        INSERT INTO federation_payment_observations
        (operation_id, operation_index, context, quote, observer_member_id, state,
         observation_json, terminal_operation_index)
        VALUES
        (:operation_id, :operation_index, :context, :quote, :observer_member_id, :state,
         :observation_json, :terminal_operation_index)
        ON CONFLICT (operation_id) DO NOTHING
        "#,
    )?
    .bind("operation_id", observation.operation_id.to_string())
    .bind("operation_index", observation.operation_index as i64)
    .bind(
        "context",
        payment_observation_context_to_str(observation.observation.context()),
    )
    .bind("quote", observation.observation.quote())
    .bind(
        "observer_member_id",
        i64::from(observation.observation.observer().get()),
    )
    .bind("state", observation.observation.state_label())
    .bind(
        "observation_json",
        serde_json::to_string(&observation.observation)?,
    )
    .bind(
        "terminal_operation_index",
        terminal_operation_index.map(|index| index as i64),
    )
    .execute(connection)
    .await?;
    Ok(())
}

/// The payment observations `selection` names, in consensus order.
async fn payment_observations<C>(
    connection: &C,
    selection: FederationPaymentObservationSelection<'_>,
) -> Result<Vec<FederationStoredPaymentObservation>, Error>
where
    C: DatabaseExecutor,
{
    let rows = match selection {
        FederationPaymentObservationSelection::Live { context, quote } => {
            query(
                r#"
                SELECT operation_id, operation_index, observation_json, terminal_operation_index
                FROM federation_payment_observations
                WHERE context = :context
                  AND quote = :quote
                  AND terminal_operation_index IS NULL
                ORDER BY operation_index ASC, observer_member_id ASC
                "#,
            )?
            .bind("context", payment_observation_context_to_str(context))
            .bind("quote", quote)
            .fetch_all(connection)
            .await?
        }
        FederationPaymentObservationSelection::History { context, quote } => {
            query(
                r#"
                SELECT operation_id, operation_index, observation_json, terminal_operation_index
                FROM federation_payment_observations
                WHERE context = :context
                  AND quote = :quote
                ORDER BY operation_index ASC, observer_member_id ASC
                "#,
            )?
            .bind("context", payment_observation_context_to_str(context))
            .bind("quote", quote)
            .fetch_all(connection)
            .await?
        }
        FederationPaymentObservationSelection::AllLive => {
            query(
                r#"
                SELECT operation_id, operation_index, observation_json, terminal_operation_index
                FROM federation_payment_observations
                WHERE terminal_operation_index IS NULL
                ORDER BY operation_index ASC, observer_member_id ASC
                "#,
            )?
            .fetch_all(connection)
            .await?
        }
    };
    rows.into_iter()
        .map(sql_row_to_stored_payment_observation)
        .collect()
}

/// Store one signature share; an identical share keeps its first position.
async fn insert_signature_share<C>(
    connection: &C,
    share: &FederationSignatureShareRecord,
) -> Result<(), Error>
where
    C: DatabaseExecutor,
{
    let rows_affected = query(
        r#"
        INSERT INTO federation_signature_shares
        (operation_id, output_index, member_id, blinded_message, amount, keyset_id, signature_share, share_json)
        VALUES
        (:operation_id, :output_index, :member_id, :blinded_message, :amount, :keyset_id, :signature_share, :share_json)
        ON CONFLICT (operation_id, output_index, member_id)
        DO UPDATE SET
            blinded_message = EXCLUDED.blinded_message,
            amount = EXCLUDED.amount,
            keyset_id = EXCLUDED.keyset_id,
            signature_share = EXCLUDED.signature_share
        WHERE federation_signature_shares.blinded_message = EXCLUDED.blinded_message
          AND federation_signature_shares.amount = EXCLUDED.amount
          AND federation_signature_shares.keyset_id = EXCLUDED.keyset_id
          AND federation_signature_shares.signature_share = EXCLUDED.signature_share
        "#,
    )?
    .bind("operation_id", share.operation_id.to_string())
    .bind("output_index", i64::from(share.output_index))
    .bind("member_id", i64::from(share.member_id.get()))
    .bind("blinded_message", share.blinded_secret.to_bytes())
    .bind("amount", u64::from(share.amount) as i64)
    .bind("keyset_id", share.keyset_id.to_string())
    .bind("signature_share", share.share.to_bytes().to_vec())
    .bind("share_json", serde_json::to_string(share)?)
    .execute(connection)
    .await?;
    if rows_affected == 0 {
        return Err(federation_signature_share_conflict_error(share));
    }

    Ok(())
}
