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

fn decode_frost_record<T>(payload_json: &str) -> Result<T, Error>
where
    T: DeserializeOwned,
{
    serde_json::from_str(payload_json).map_err(|error| {
        Error::Internal(format!("Invalid federation FROST DKG record JSON: {error}"))
    })
}

/// Read one DKG ceremony record, locking its row for the rest of the transaction.
async fn locked_federation_dkg_ceremony<RM>(
    transaction: &SQLTransaction<RM>,
    ceremony_id: FederationDkgCeremonyId,
) -> Result<Option<FederationDkgCeremonyRecord>, Error>
where
    RM: DatabasePool + 'static,
{
    query(
        r#"
        SELECT payload_json
        FROM federation_dkg_ceremonies
        WHERE ceremony_id = :ceremony_id
        FOR UPDATE
        "#,
    )?
    .bind("ceremony_id", ceremony_id.to_string())
    .fetch_one(&transaction.inner)
    .await?
    .map(|row| {
        unpack_into!(let (payload_json) = row);
        serde_json::from_str(&column_as_string!(&payload_json)).map_err(|error| {
            Error::Internal(format!("Invalid federation DKG ceremony JSON: {error}"))
        })
    })
    .transpose()
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
            ON CONFLICT (ceremony_id) DO NOTHING
            "#,
        )?
        .bind("ceremony_id", ceremony.ceremony_id.to_string())
        .bind("payload_json", serde_json::to_string(ceremony)?)
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            // Idempotent retry: the identical record is already stored.
            let existing = locked_federation_dkg_ceremony(self, ceremony.ceremony_id).await?;
            if existing.as_ref() != Some(ceremony) {
                return Err(federation_dkg_ceremony_conflict_error(ceremony.ceremony_id));
            }
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
        if !expected.state.can_transition_to(updated.state) {
            return Err(Error::Internal(format!(
                "Illegal federation DKG ceremony transition from {} to {}",
                expected.state.as_str(),
                updated.state.as_str()
            )));
        }

        let current = locked_federation_dkg_ceremony(self, expected.ceremony_id).await?;
        if current.as_ref() == Some(updated) {
            // Idempotent retry of a transition that already committed.
            return Ok(());
        }
        if current.as_ref() != Some(expected) {
            return Err(federation_dkg_ceremony_conflict_error(expected.ceremony_id));
        }

        query(
            r#"
            UPDATE federation_dkg_ceremonies
            SET payload_json = :updated_payload_json
            WHERE ceremony_id = :ceremony_id
            "#,
        )?
        .bind("ceremony_id", expected.ceremony_id.to_string())
        .bind("updated_payload_json", serde_json::to_string(updated)?)
        .execute(&self.inner)
        .await?;
        Ok(())
    }

    async fn add_federation_dkg_record(
        &mut self,
        record: FederationDkgRecord<'_>,
    ) -> Result<(), Self::Err> {
        match record {
            FederationDkgRecord::Commitment(commitment) => {
                self.insert_dkg_commitment(commitment).await
            }
            FederationDkgRecord::Reveal(reveal) => self.insert_dkg_reveal(reveal).await,
            FederationDkgRecord::SecretShareContribution(contribution) => {
                self.insert_dkg_secret_share_contribution(contribution)
                    .await
            }
            FederationDkgRecord::ParticipantState(state) => {
                self.insert_dkg_participant_state(state).await
            }
            FederationDkgRecord::MemberKeysetShare(share) => {
                self.insert_dkg_member_keyset_share(share).await
            }
            FederationDkgRecord::TranscriptSignature(signature) => {
                self.insert_dkg_transcript_signature(signature).await
            }
            FederationDkgRecord::Activation(activation) => {
                self.insert_dkg_activation(activation).await
            }
            FederationDkgRecord::Frost(FederationFrostDkgRecord::Round1(request)) => {
                self.insert_frost_dkg_round1(request).await
            }
            FederationDkgRecord::Frost(FederationFrostDkgRecord::Round2(record)) => {
                self.insert_frost_dkg_round2(record).await
            }
            FederationDkgRecord::Frost(FederationFrostDkgRecord::LocalState(state)) => {
                self.insert_frost_dkg_local_state(state).await
            }
            FederationDkgRecord::Frost(FederationFrostDkgRecord::Root(record)) => {
                self.insert_frost_root_record(record).await
            }
            FederationDkgRecord::Frost(FederationFrostDkgRecord::Confirmation(confirmation)) => {
                self.insert_frost_dkg_confirmation(confirmation).await
            }
        }
    }
}

/// Immutable DKG records, stored through [`FederationDkgTransaction::add_federation_dkg_record`].
impl<RM> SQLTransaction<RM>
where
    RM: DatabasePool + 'static,
{
    async fn insert_dkg_commitment(
        &mut self,
        commitment: &FederationDkgParticipantCommitment,
    ) -> Result<(), Error> {
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

    async fn insert_dkg_reveal(
        &mut self,
        reveal: &FederationDkgParticipantCommitmentReveal,
    ) -> Result<(), Error> {
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

    async fn insert_dkg_secret_share_contribution(
        &mut self,
        contribution: &FederationDkgSecretShareRequest,
    ) -> Result<(), Error> {
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

    async fn insert_dkg_participant_state(
        &mut self,
        state: &FederationDkgParticipantState,
    ) -> Result<(), Error> {
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

    async fn insert_dkg_member_keyset_share(
        &mut self,
        share: &FederationDkgMemberKeysetShare,
    ) -> Result<(), Error> {
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

    async fn insert_dkg_transcript_signature(
        &mut self,
        signature: &FederationDkgTranscriptSignatureRequest,
    ) -> Result<(), Error> {
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

    async fn insert_dkg_activation(
        &mut self,
        activation: &FederationDkgActivationRequest,
    ) -> Result<(), Error> {
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

    async fn insert_frost_dkg_round1(
        &mut self,
        request: &FederationFrostDkgRound1Request,
    ) -> Result<(), Error> {
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

    async fn insert_frost_dkg_round2(
        &mut self,
        record: &FederationFrostDkgRound2Record,
    ) -> Result<(), Error> {
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

    async fn insert_frost_dkg_local_state(
        &mut self,
        state: &FederationFrostDkgLocalState,
    ) -> Result<(), Error> {
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

    async fn insert_frost_root_record(
        &mut self,
        record: &FederationFrostRootRecord,
    ) -> Result<(), Error> {
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

    async fn insert_frost_dkg_confirmation(
        &mut self,
        confirmation: &FederationFrostDkgConfirmationRequest,
    ) -> Result<(), Error> {
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

    async fn get_federation_frost_dkg_records(
        &self,
    ) -> Result<FederationFrostDkgRecords, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|error| Error::Database(Box::new(error)))?;
        let rows = query(
            r#"
            SELECT record_kind, payload_json
            FROM federation_frost_dkg_records
            ORDER BY ceremony_id ASC, sender_id ASC, receiver_id ASC
            "#,
        )?
        .fetch_all(&*conn)
        .await?;
        let mut records = FederationFrostDkgRecords::default();
        let mut local_round2 = Vec::new();
        for row in rows {
            unpack_into!(let (record_kind, payload_json) = row);
            let record_kind = column_as_string!(&record_kind);
            let payload_json = column_as_string!(&payload_json);
            match record_kind.as_str() {
                FEDERATION_FROST_DKG_ROUND1_RECORD => {
                    records.round1.push(decode_frost_record(&payload_json)?);
                }
                FEDERATION_FROST_DKG_ROUND2_RECORD => {
                    records.round2.push(decode_frost_record(&payload_json)?);
                }
                FEDERATION_FROST_DKG_LOCAL_ROUND1_RECORD => {
                    records
                        .local_states
                        .push(decode_frost_record(&payload_json)?);
                }
                FEDERATION_FROST_DKG_LOCAL_ROUND2_RECORD => {
                    local_round2.push(decode_frost_record(&payload_json)?);
                }
                FEDERATION_FROST_DKG_ROOT_RECORD => {
                    records.roots.push(decode_frost_record(&payload_json)?);
                }
                FEDERATION_FROST_DKG_CONFIRMATION_RECORD => {
                    records
                        .confirmations
                        .push(decode_frost_record(&payload_json)?);
                }
                _ => {
                    return Err(Error::Internal(
                        "Unknown federation FROST DKG record kind".to_string(),
                    ))
                }
            }
        }
        records.local_states.extend(local_round2);
        Ok(records)
    }
}
