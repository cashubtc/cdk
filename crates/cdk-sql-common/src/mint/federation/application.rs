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
    ) -> Result<(), Self::Err> {
        query(
            r#"
            INSERT INTO federation_payment_observations
            (operation_id, operation_index, context, quote, observer_member_id, state, observation_json)
            VALUES
            (:operation_id, :operation_index, :context, :quote, :observer_member_id, :state, :observation_json)
            ON CONFLICT (operation_id)
            DO UPDATE SET
                operation_index = EXCLUDED.operation_index,
                context = EXCLUDED.context,
                quote = EXCLUDED.quote,
                observer_member_id = EXCLUDED.observer_member_id,
                state = EXCLUDED.state,
                observation_json = EXCLUDED.observation_json
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
        .bind("observation_json", serde_json::to_string(&observation.observation)?)
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn add_federation_signature_share(
        &mut self,
        share: &FederationSignatureShareRecord,
    ) -> Result<(), Self::Err> {
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
                signature_share = EXCLUDED.signature_share,
                share_json = EXCLUDED.share_json
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
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            return Err(federation_signature_share_conflict_error(share));
        }

        Ok(())
    }

    async fn get_federation_payment_observations(
        &mut self,
        context: FederationPaymentObservationContext,
        quote: &str,
    ) -> Result<Vec<FederationPaymentObservationRecord>, Self::Err> {
        query(
            r#"
            SELECT operation_id, operation_index, observation_json
            FROM federation_payment_observations
            WHERE context = :context
              AND quote = :quote
            ORDER BY operation_index ASC, observer_member_id ASC
            "#,
        )?
        .bind("context", payment_observation_context_to_str(context))
        .bind("quote", quote)
        .fetch_all(&self.inner)
        .await?
        .into_iter()
        .map(sql_row_to_federation_payment_observation_record)
        .collect()
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
                    reason: "federation application support is disabled".to_owned(),
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
        context: FederationPaymentObservationContext,
        quote: &str,
    ) -> Result<Vec<FederationPaymentObservationRecord>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT operation_id, operation_index, observation_json
            FROM federation_payment_observations
            WHERE context = :context
              AND quote = :quote
            ORDER BY operation_index ASC, observer_member_id ASC
            "#,
        )?
        .bind("context", payment_observation_context_to_str(context))
        .bind("quote", quote)
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_payment_observation_record)
        .collect()
    }

    async fn get_all_federation_payment_observations(
        &self,
    ) -> Result<Vec<FederationPaymentObservationRecord>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT operation_id, operation_index, observation_json
            FROM federation_payment_observations
            ORDER BY operation_index ASC, observer_member_id ASC
            "#,
        )?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_payment_observation_record)
        .collect()
    }

    async fn get_federation_materialized_state_keys(
        &self,
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
            ORDER BY state_kind ASC, state_key ASC
            "#,
        )?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_materialized_state_key)
        .collect()
    }

    async fn get_federation_application_projection_snapshot(
        &self,
        application_id: cdk_common::federation::FederationFrostApplicationId,
    ) -> Result<serde_json::Value, Self::Err> {
        #[cfg(feature = "federation-bark")]
        {
            if application_id != cdk_federation_bark::federation_bark_application_id()? {
                return Ok(serde_json::Value::Null);
            }
            let conn = self
                .pool
                .get()
                .await
                .map_err(|e| Error::Database(Box::new(e)))?;
            let snapshot = super::bark_projection::get_bark_projection_snapshot(&*conn).await?;
            return serde_json::to_value(snapshot).map_err(Error::from);
        }
        #[cfg(not(feature = "federation-bark"))]
        {
            let _ = application_id;
            Ok(serde_json::Value::Null)
        }
    }

    async fn get_federation_application_action(
        &self,
        application_id: cdk_common::federation::FederationFrostApplicationId,
        action_id: [u8; 32],
    ) -> Result<Option<serde_json::Value>, Self::Err> {
        #[cfg(feature = "federation-bark")]
        {
            if application_id != cdk_federation_bark::federation_bark_application_id()? {
                return Ok(None);
            }
            let action_id = cdk_federation_bark::FederationBarkActionId::from_bytes(action_id)?;
            let conn = self
                .pool
                .get()
                .await
                .map_err(|e| Error::Database(Box::new(e)))?;
            super::bark_projection::get_action(&*conn, action_id)
                .await?
                .map(serde_json::to_value)
                .transpose()
                .map_err(Error::from)
        }
        #[cfg(not(feature = "federation-bark"))]
        {
            let _ = (application_id, action_id);
            Ok(None)
        }
    }

    async fn get_federation_mint_quote_states(
        &self,
        quote_ids: &[QuoteId],
    ) -> Result<Vec<Option<FederationMintQuoteState>>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        get_federation_mint_quote_states_inner(&*conn, quote_ids).await
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

    async fn get_federation_completed_operations(
        &self,
        operation_ids: &[uuid::Uuid],
    ) -> Result<Vec<cdk_common::mint::Operation>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        get_completed_operations_by_ids_inner(&*conn, operation_ids).await
    }

    async fn get_federation_signature_shares_for_output(
        &self,
        operation_id: FederationOperationId,
        output_index: u32,
    ) -> Result<Vec<FederationSignatureShareRecord>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
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
        .into_iter()
        .map(sql_row_to_federation_signature_share_record)
        .collect()
    }

    async fn get_federation_signature_shares_for_blinded_message(
        &self,
        blinded_message: PublicKey,
    ) -> Result<Vec<FederationSignatureShareRecord>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
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
        .into_iter()
        .map(sql_row_to_federation_signature_share_record)
        .collect()
    }
}
