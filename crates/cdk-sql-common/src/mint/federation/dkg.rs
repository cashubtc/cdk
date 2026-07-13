use super::*;

async fn insert_frost_record<C, T>(
    connection: &C,
    ceremony_id: FederationDkgCeremonyId,
    record_kind: &str,
    sender: MemberId,
    receiver: Option<MemberId>,
    payload: &T,
) -> Result<(), Error>
where
    C: DatabaseExecutor,
    T: Serialize,
{
    let rows_affected = query(
        r#"
        INSERT INTO federation_frost_dkg_records
        (ceremony_id, record_kind, sender_id, receiver_id, payload_json)
        VALUES
        (:ceremony_id, :record_kind, :sender_id, :receiver_id, :payload_json)
        ON CONFLICT (ceremony_id, record_kind, sender_id, receiver_id)
        DO UPDATE SET payload_json = EXCLUDED.payload_json
        WHERE federation_frost_dkg_records.payload_json = EXCLUDED.payload_json
        "#,
    )?
    .bind("ceremony_id", ceremony_id.to_string())
    .bind("record_kind", record_kind.to_string())
    .bind("sender_id", i64::from(sender.get()))
    .bind(
        "receiver_id",
        receiver.map_or(0_i64, |member| i64::from(member.get())),
    )
    .bind("payload_json", serde_json::to_string(payload)?)
    .execute(connection)
    .await?;
    if rows_affected == 0 {
        return Err(Error::Duplicate);
    }
    Ok(())
}

async fn fetch_frost_records<C, T>(connection: &C, record_kind: &str) -> Result<Vec<T>, Error>
where
    C: DatabaseExecutor,
    T: DeserializeOwned,
{
    query(
        r#"
        SELECT payload_json
        FROM federation_frost_dkg_records
        WHERE record_kind = :record_kind
        ORDER BY ceremony_id ASC, sender_id ASC, receiver_id ASC
        "#,
    )?
    .bind("record_kind", record_kind.to_string())
    .fetch_all(connection)
    .await?
    .into_iter()
    .map(|row| {
        unpack_into!(let (payload_json) = row);
        serde_json::from_str(&column_as_string!(&payload_json)).map_err(|error| {
            Error::Internal(format!("Invalid federation FROST DKG record JSON: {error}"))
        })
    })
    .collect()
}

#[async_trait]
impl<RM> FederationDkgTransaction for SQLTransaction<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn add_federation_dkg_ceremony(
        &mut self,
        ceremony: &FederationDkgCeremonyRecord,
    ) -> Result<(), Self::Err> {
        let rows_affected = query(
            r#"
            INSERT INTO federation_dkg_ceremonies
            (ceremony_id, payload_json)
            VALUES
            (:ceremony_id, :payload_json)
            ON CONFLICT (ceremony_id)
            DO UPDATE SET payload_json = EXCLUDED.payload_json
            WHERE federation_dkg_ceremonies.payload_json = EXCLUDED.payload_json
            "#,
        )?
        .bind("ceremony_id", ceremony.ceremony_id.to_string())
        .bind("payload_json", serde_json::to_string(ceremony)?)
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            return Err(federation_dkg_ceremony_conflict_error(ceremony.ceremony_id));
        }
        Ok(())
    }

    async fn transition_federation_dkg_ceremony(
        &mut self,
        expected: &FederationDkgCeremonyRecord,
        updated: &FederationDkgCeremonyRecord,
    ) -> Result<(), Self::Err> {
        if expected.ceremony_id != updated.ceremony_id {
            return Err(Error::Internal(
                "Federation DKG ceremony transition cannot change ceremony ID".to_string(),
            ));
        }
        let rows_affected = query(
            r#"
            UPDATE federation_dkg_ceremonies
            SET payload_json = :updated_payload_json
            WHERE ceremony_id = :ceremony_id
              AND (
                payload_json = :expected_payload_json
                OR payload_json = :updated_payload_json
              )
            "#,
        )?
        .bind("ceremony_id", expected.ceremony_id.to_string())
        .bind("expected_payload_json", serde_json::to_string(expected)?)
        .bind("updated_payload_json", serde_json::to_string(updated)?)
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            return Err(federation_dkg_ceremony_conflict_error(expected.ceremony_id));
        }
        Ok(())
    }

    async fn add_federation_dkg_commitment(
        &mut self,
        commitment: &FederationDkgParticipantCommitment,
    ) -> Result<(), Self::Err> {
        let rows_affected = query(
            r#"
            INSERT INTO federation_dkg_setup_messages
            (ceremony_id, phase, member_id, payload_json)
            VALUES
            (:ceremony_id, :phase, :member_id, :payload_json)
            ON CONFLICT (ceremony_id, phase, member_id)
            DO UPDATE SET payload_json = EXCLUDED.payload_json
            WHERE federation_dkg_setup_messages.payload_json = EXCLUDED.payload_json
            "#,
        )?
        .bind("ceremony_id", commitment.ceremony_id.to_string())
        .bind("phase", FEDERATION_DKG_COMMITMENT_PHASE)
        .bind("member_id", i64::from(commitment.member_id.get()))
        .bind("payload_json", serde_json::to_string(commitment)?)
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            return Err(federation_dkg_conflict_error(
                FEDERATION_DKG_COMMITMENT_PHASE,
                commitment.member_id,
            ));
        }

        Ok(())
    }

    async fn add_federation_dkg_reveal(
        &mut self,
        reveal: &FederationDkgParticipantCommitmentReveal,
    ) -> Result<(), Self::Err> {
        let rows_affected = query(
            r#"
            INSERT INTO federation_dkg_setup_messages
            (ceremony_id, phase, member_id, payload_json)
            VALUES
            (:ceremony_id, :phase, :member_id, :payload_json)
            ON CONFLICT (ceremony_id, phase, member_id)
            DO UPDATE SET payload_json = EXCLUDED.payload_json
            WHERE federation_dkg_setup_messages.payload_json = EXCLUDED.payload_json
            "#,
        )?
        .bind("ceremony_id", reveal.ceremony_id.to_string())
        .bind("phase", FEDERATION_DKG_REVEAL_PHASE)
        .bind("member_id", i64::from(reveal.member_id.get()))
        .bind("payload_json", serde_json::to_string(reveal)?)
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            return Err(federation_dkg_conflict_error(
                FEDERATION_DKG_REVEAL_PHASE,
                reveal.member_id,
            ));
        }

        Ok(())
    }

    async fn add_federation_dkg_secret_share_contribution(
        &mut self,
        contribution: &FederationDkgSecretShareRequest,
    ) -> Result<(), Self::Err> {
        let rows_affected = query(
            r#"
            INSERT INTO federation_dkg_secret_share_contributions
            (ceremony_id, sender_id, receiver_id, payload_json)
            VALUES
            (:ceremony_id, :sender_id, :receiver_id, :payload_json)
            ON CONFLICT (ceremony_id, sender_id, receiver_id)
            DO UPDATE SET payload_json = EXCLUDED.payload_json
            WHERE federation_dkg_secret_share_contributions.payload_json = EXCLUDED.payload_json
            "#,
        )?
        .bind("ceremony_id", contribution.ceremony_id.to_string())
        .bind("sender_id", i64::from(contribution.sender.get()))
        .bind("receiver_id", i64::from(contribution.receiver.get()))
        .bind("payload_json", serde_json::to_string(contribution)?)
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            return Err(federation_dkg_secret_share_conflict_error(contribution));
        }

        Ok(())
    }

    async fn add_federation_dkg_participant_state(
        &mut self,
        state: &FederationDkgParticipantState,
    ) -> Result<(), Self::Err> {
        let rows_affected = query(
            r#"
            INSERT INTO federation_dkg_participant_states
            (ceremony_id, member_id, payload_json)
            VALUES
            (:ceremony_id, :member_id, :payload_json)
            ON CONFLICT (ceremony_id)
            DO UPDATE SET
                member_id = EXCLUDED.member_id,
                payload_json = EXCLUDED.payload_json
            WHERE federation_dkg_participant_states.member_id = EXCLUDED.member_id
              AND federation_dkg_participant_states.payload_json = EXCLUDED.payload_json
            "#,
        )?
        .bind("ceremony_id", state.ceremony_id().to_string())
        .bind("member_id", i64::from(state.member_id().get()))
        .bind("payload_json", serde_json::to_string(state)?)
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            return Err(federation_dkg_participant_state_conflict_error(state));
        }

        Ok(())
    }

    async fn add_federation_dkg_member_keyset_share(
        &mut self,
        share: &FederationDkgMemberKeysetShare,
    ) -> Result<(), Self::Err> {
        let rows_affected = query(
            r#"
            INSERT INTO federation_dkg_member_keyset_shares
            (keyset_id, member_id, payload_json)
            VALUES
            (:keyset_id, :member_id, :payload_json)
            ON CONFLICT (keyset_id)
            DO UPDATE SET
                member_id = EXCLUDED.member_id,
                payload_json = EXCLUDED.payload_json
            WHERE federation_dkg_member_keyset_shares.member_id = EXCLUDED.member_id
              AND federation_dkg_member_keyset_shares.payload_json = EXCLUDED.payload_json
            "#,
        )?
        .bind("keyset_id", share.keyset.id.to_string())
        .bind("member_id", i64::from(share.member_id.get()))
        .bind("payload_json", serde_json::to_string(share)?)
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            return Err(federation_dkg_member_keyset_share_conflict_error(share));
        }

        Ok(())
    }

    async fn add_federation_dkg_transcript_signature(
        &mut self,
        signature: &FederationDkgTranscriptSignatureRequest,
    ) -> Result<(), Self::Err> {
        let rows_affected = query(
            r#"
            INSERT INTO federation_dkg_transcript_signatures
            (transcript_hash, member_id, payload_json)
            VALUES
            (:transcript_hash, :member_id, :payload_json)
            ON CONFLICT (transcript_hash, member_id)
            DO UPDATE SET payload_json = EXCLUDED.payload_json
            WHERE federation_dkg_transcript_signatures.payload_json = EXCLUDED.payload_json
            "#,
        )?
        .bind("transcript_hash", signature.transcript_hash.to_string())
        .bind("member_id", i64::from(signature.signer.get()))
        .bind("payload_json", serde_json::to_string(signature)?)
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            return Err(federation_dkg_transcript_signature_conflict_error(
                signature,
            ));
        }

        Ok(())
    }

    async fn add_federation_dkg_activation(
        &mut self,
        activation: &FederationDkgActivationRequest,
    ) -> Result<(), Self::Err> {
        let rows_affected = query(
            r#"
            INSERT INTO federation_dkg_activations
            (ceremony_id, member_id, payload_json)
            VALUES
            (:ceremony_id, :member_id, :payload_json)
            ON CONFLICT (ceremony_id, member_id)
            DO UPDATE SET payload_json = EXCLUDED.payload_json
            WHERE federation_dkg_activations.payload_json = EXCLUDED.payload_json
            "#,
        )?
        .bind("ceremony_id", activation.ceremony_id.to_string())
        .bind("member_id", i64::from(activation.signer.get()))
        .bind("payload_json", serde_json::to_string(activation)?)
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            return Err(federation_dkg_activation_conflict_error(activation));
        }

        Ok(())
    }

    async fn add_federation_frost_dkg_round1(
        &mut self,
        request: &FederationFrostDkgRound1Request,
    ) -> Result<(), Self::Err> {
        insert_frost_record(
            &self.inner,
            request.context.ceremony_id,
            FEDERATION_FROST_DKG_ROUND1_RECORD,
            request.sender,
            None,
            request,
        )
        .await
    }

    async fn add_federation_frost_dkg_round2(
        &mut self,
        record: &FederationFrostDkgRound2Record,
    ) -> Result<(), Self::Err> {
        insert_frost_record(
            &self.inner,
            record.context.ceremony_id,
            FEDERATION_FROST_DKG_ROUND2_RECORD,
            record.sender,
            Some(record.receiver),
            record,
        )
        .await
    }

    async fn set_federation_frost_dkg_local_state(
        &mut self,
        state: &FederationFrostDkgLocalState,
    ) -> Result<(), Self::Err> {
        let record_kind = match state.phase {
            cdk_common::federation::FederationFrostDkgLocalPhase::Round1 => {
                FEDERATION_FROST_DKG_LOCAL_ROUND1_RECORD
            }
            cdk_common::federation::FederationFrostDkgLocalPhase::Round2 => {
                FEDERATION_FROST_DKG_LOCAL_ROUND2_RECORD
            }
        };
        insert_frost_record(
            &self.inner,
            state.context.ceremony_id,
            record_kind,
            state.member_id,
            None,
            state,
        )
        .await
    }

    async fn add_federation_frost_root_record(
        &mut self,
        record: &FederationFrostRootRecord,
    ) -> Result<(), Self::Err> {
        insert_frost_record(
            &self.inner,
            record.context.ceremony_id,
            FEDERATION_FROST_DKG_ROOT_RECORD,
            record.member_id,
            None,
            record,
        )
        .await
    }

    async fn add_federation_frost_dkg_confirmation(
        &mut self,
        confirmation: &FederationFrostDkgConfirmationRequest,
    ) -> Result<(), Self::Err> {
        insert_frost_record(
            &self.inner,
            confirmation.context.ceremony_id,
            FEDERATION_FROST_DKG_CONFIRMATION_RECORD,
            confirmation.member_id,
            None,
            confirmation,
        )
        .await
    }
}

#[async_trait]
impl<RM> FederationDkgDatabase for SQLMintDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn get_federation_dkg_ceremonies(
        &self,
    ) -> Result<Vec<FederationDkgCeremonyRecord>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT ceremony_id, payload_json
            FROM federation_dkg_ceremonies
            ORDER BY ceremony_id ASC
            "#,
        )?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_dkg_ceremony)
        .collect()
    }

    async fn get_federation_dkg_commitments(
        &self,
    ) -> Result<Vec<FederationDkgParticipantCommitment>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT member_id, payload_json
            FROM federation_dkg_setup_messages
            WHERE phase = :phase
            ORDER BY ceremony_id ASC, member_id ASC
            "#,
        )?
        .bind("phase", FEDERATION_DKG_COMMITMENT_PHASE)
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_dkg_commitment)
        .collect()
    }

    async fn get_federation_dkg_reveals(
        &self,
    ) -> Result<Vec<FederationDkgParticipantCommitmentReveal>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT member_id, payload_json
            FROM federation_dkg_setup_messages
            WHERE phase = :phase
            ORDER BY ceremony_id ASC, member_id ASC
            "#,
        )?
        .bind("phase", FEDERATION_DKG_REVEAL_PHASE)
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_dkg_reveal)
        .collect()
    }

    async fn get_federation_dkg_secret_share_contributions(
        &self,
    ) -> Result<Vec<FederationDkgSecretShareRequest>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT sender_id, receiver_id, payload_json
            FROM federation_dkg_secret_share_contributions
            ORDER BY ceremony_id ASC, sender_id ASC, receiver_id ASC
            "#,
        )?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_dkg_secret_share)
        .collect()
    }

    async fn get_federation_dkg_participant_states(
        &self,
    ) -> Result<Vec<FederationDkgParticipantState>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT ceremony_id, member_id, payload_json
            FROM federation_dkg_participant_states
            ORDER BY ceremony_id ASC
            "#,
        )?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_dkg_participant_state)
        .collect()
    }

    async fn get_federation_dkg_member_keyset_shares(
        &self,
    ) -> Result<Vec<FederationDkgMemberKeysetShare>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT keyset_id, member_id, payload_json
            FROM federation_dkg_member_keyset_shares
            ORDER BY member_id ASC, keyset_id ASC
            "#,
        )?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_dkg_member_keyset_share)
        .collect()
    }

    async fn get_federation_dkg_transcript_signatures(
        &self,
    ) -> Result<Vec<FederationDkgTranscriptSignatureRequest>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT transcript_hash, member_id, payload_json
            FROM federation_dkg_transcript_signatures
            ORDER BY transcript_hash ASC, member_id ASC
            "#,
        )?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_dkg_transcript_signature)
        .collect()
    }

    async fn get_federation_dkg_activations(
        &self,
    ) -> Result<Vec<FederationDkgActivationRequest>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT ceremony_id, member_id, payload_json
            FROM federation_dkg_activations
            ORDER BY ceremony_id ASC, member_id ASC
            "#,
        )?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_dkg_activation)
        .collect()
    }

    async fn get_federation_frost_dkg_round1(
        &self,
    ) -> Result<Vec<FederationFrostDkgRound1Request>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|error| Error::Database(Box::new(error)))?;
        fetch_frost_records(&*conn, FEDERATION_FROST_DKG_ROUND1_RECORD).await
    }

    async fn get_federation_frost_dkg_round2(
        &self,
    ) -> Result<Vec<FederationFrostDkgRound2Record>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|error| Error::Database(Box::new(error)))?;
        fetch_frost_records(&*conn, FEDERATION_FROST_DKG_ROUND2_RECORD).await
    }

    async fn get_federation_frost_dkg_local_states(
        &self,
    ) -> Result<Vec<FederationFrostDkgLocalState>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|error| Error::Database(Box::new(error)))?;
        let mut states =
            fetch_frost_records(&*conn, FEDERATION_FROST_DKG_LOCAL_ROUND1_RECORD).await?;
        states.extend(fetch_frost_records(&*conn, FEDERATION_FROST_DKG_LOCAL_ROUND2_RECORD).await?);
        Ok(states)
    }

    async fn get_federation_frost_root_records(
        &self,
    ) -> Result<Vec<FederationFrostRootRecord>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|error| Error::Database(Box::new(error)))?;
        fetch_frost_records(&*conn, FEDERATION_FROST_DKG_ROOT_RECORD).await
    }

    async fn get_federation_frost_dkg_confirmations(
        &self,
    ) -> Result<Vec<FederationFrostDkgConfirmationRequest>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|error| Error::Database(Box::new(error)))?;
        fetch_frost_records(&*conn, FEDERATION_FROST_DKG_CONFIRMATION_RECORD).await
    }
}
