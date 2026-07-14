use super::*;

#[async_trait]
impl<RM> FederationJournalTransaction for SQLTransaction<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn set_federation_config(
        &mut self,
        config: &FederationConfigSnapshot,
    ) -> Result<(), Self::Err> {
        let config_json = serde_json::to_string(&config.config)?;
        let rows_affected = query(
            r#"
            INSERT INTO federation_config
            (federation_id, config_digest, config_json)
            VALUES
            (:federation_id, :config_digest, :config_json)
            ON CONFLICT (federation_id)
            DO UPDATE SET
                config_digest = EXCLUDED.config_digest,
                config_json = EXCLUDED.config_json
            WHERE federation_config.config_digest = EXCLUDED.config_digest
              AND federation_config.config_json = EXCLUDED.config_json
            "#,
        )?
        .bind("federation_id", config.federation_id.to_string())
        .bind("config_digest", config.config_digest.to_string())
        .bind("config_json", config_json.clone())
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            let mut rows = query(
                r#"
                SELECT
                    federation_id,
                    config_digest,
                    config_json
                FROM federation_config
                WHERE federation_id = :federation_id
                "#,
            )?
            .bind("federation_id", config.federation_id.to_string())
            .fetch_all(&self.inner)
            .await?
            .into_iter();

            if let Some(existing) = rows
                .next()
                .map(sql_row_to_federation_config_snapshot)
                .transpose()?
            {
                if is_dkg_finalization_config_update(&existing, config) {
                    query(
                        r#"
                        UPDATE federation_config
                        SET
                            config_digest = :config_digest,
                            config_json = :config_json
                        WHERE federation_id = :federation_id
                        "#,
                    )?
                    .bind("federation_id", config.federation_id.to_string())
                    .bind("config_digest", config.config_digest.to_string())
                    .bind("config_json", config_json)
                    .execute(&self.inner)
                    .await?;
                    return Ok(());
                }
            }

            return Err(federation_config_conflict_error(config));
        }

        Ok(())
    }

    async fn add_federation_journal_entry(
        &mut self,
        entry: &FederationJournalEntry,
    ) -> Result<(), Self::Err> {
        query(
            r#"
            INSERT INTO federation_journal_entries
            (operation_index, operation_id, envelope_json, conflict_keys_json, previous_order_digest, order_digest, consensus_evidence_json, rejection_json, status)
            VALUES
            (:operation_index, :operation_id, :envelope_json, :conflict_keys_json, :previous_order_digest, :order_digest, :consensus_evidence_json, :rejection_json, :status)
            "#,
        )?
        .bind("operation_index", entry.index as i64)
        .bind("operation_id", entry.operation_id.to_string())
        .bind("envelope_json", serde_json::to_string(&entry.envelope)?)
        .bind(
            "conflict_keys_json",
            serde_json::to_string(&entry.conflict_keys)?,
        )
        .bind(
            "previous_order_digest",
            entry.previous_order_digest.to_string(),
        )
        .bind("order_digest", entry.order_digest.to_string())
        .bind(
            "consensus_evidence_json",
            entry
                .consensus_evidence
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?,
        )
        .bind(
            "rejection_json",
            entry
                .rejection()
                .map(serde_json::to_string)
                .transpose()?,
        )
        .bind("status", status_to_str(entry.status()))
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn set_federation_operation_application_state(
        &mut self,
        operation_id: FederationOperationId,
        application: &FederationApplicationState,
    ) -> Result<(), Self::Err> {
        query(
            r#"
            UPDATE federation_journal_entries
            SET status = :status, rejection_json = :rejection_json
            WHERE operation_id = :operation_id
            "#,
        )?
        .bind("operation_id", operation_id.to_string())
        .bind("status", status_to_str(application.status()))
        .bind(
            "rejection_json",
            application
                .rejection()
                .map(serde_json::to_string)
                .transpose()?,
        )
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn set_federation_dkg_member_sequence(
        &mut self,
        member_id: MemberId,
        sequence: u64,
    ) -> Result<(), Self::Err> {
        query(
            r#"
            INSERT INTO federation_dkg_member_sequences
            (member_id, last_sequence)
            VALUES
            (:member_id, :last_sequence)
            ON CONFLICT (member_id)
            DO UPDATE SET last_sequence = EXCLUDED.last_sequence
            WHERE federation_dkg_member_sequences.last_sequence < EXCLUDED.last_sequence
            "#,
        )?
        .bind("member_id", i64::from(member_id.get()))
        .bind("last_sequence", sequence as i64)
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn add_federation_catch_up_certificate(
        &mut self,
        certificate: &FederationCatchUpCertificate,
    ) -> Result<(), Self::Err> {
        query(
            r#"
            INSERT INTO federation_catch_up_certificates
            (from_index, signer_member_id, certificate_json)
            VALUES
            (:from_index, :signer_member_id, :certificate_json)
            ON CONFLICT (from_index, signer_member_id)
            DO UPDATE SET
                certificate_json = EXCLUDED.certificate_json
            "#,
        )?
        .bind("from_index", certificate.from_index as i64)
        .bind("signer_member_id", i64::from(certificate.signer.get()))
        .bind("certificate_json", serde_json::to_string(certificate)?)
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn add_federation_aleph_bft_finalized_item(
        &mut self,
        item: &FederationAlephBftFinalizedItem,
    ) -> Result<(), Self::Err> {
        let rows_affected = query(
            r#"
            INSERT INTO federation_aleph_bft_finalized_items
            (
                accepted_item_index,
                session,
                unit_finalization_index,
                unit_submitting_member,
                unit_index,
                item_index_in_unit,
                operation_id,
                status,
                unit_body_hash,
                item_json
            )
            VALUES
            (
                :accepted_item_index,
                :session,
                :unit_finalization_index,
                :unit_submitting_member,
                :unit_index,
                :item_index_in_unit,
                :operation_id,
                :status,
                :unit_body_hash,
                :item_json
            )
            ON CONFLICT (accepted_item_index)
            DO UPDATE SET
                session = EXCLUDED.session,
                unit_finalization_index = EXCLUDED.unit_finalization_index,
                unit_submitting_member = EXCLUDED.unit_submitting_member,
                unit_index = EXCLUDED.unit_index,
                item_index_in_unit = EXCLUDED.item_index_in_unit,
                operation_id = EXCLUDED.operation_id,
                status = EXCLUDED.status,
                unit_body_hash = EXCLUDED.unit_body_hash,
                item_json = EXCLUDED.item_json
            WHERE federation_aleph_bft_finalized_items.session = EXCLUDED.session
              AND federation_aleph_bft_finalized_items.unit_finalization_index = EXCLUDED.unit_finalization_index
              AND federation_aleph_bft_finalized_items.unit_submitting_member = EXCLUDED.unit_submitting_member
              AND federation_aleph_bft_finalized_items.unit_index = EXCLUDED.unit_index
              AND federation_aleph_bft_finalized_items.item_index_in_unit = EXCLUDED.item_index_in_unit
              AND (
                    federation_aleph_bft_finalized_items.operation_id = EXCLUDED.operation_id
                    OR (
                        federation_aleph_bft_finalized_items.operation_id IS NULL
                        AND EXCLUDED.operation_id IS NULL
                    )
                  )
              AND federation_aleph_bft_finalized_items.status = EXCLUDED.status
              AND federation_aleph_bft_finalized_items.unit_body_hash = EXCLUDED.unit_body_hash
              AND federation_aleph_bft_finalized_items.item_json = EXCLUDED.item_json
            "#,
        )?
        .bind("accepted_item_index", item.accepted_item_index as i64)
        .bind("session", item.session as i64)
        .bind(
            "unit_finalization_index",
            item.unit_finalization_index as i64,
        )
        .bind(
            "unit_submitting_member",
            i64::from(item.unit_submitting_member.get()),
        )
        .bind("unit_index", item.unit_index as i64)
        .bind("item_index_in_unit", item.item_index_in_unit as i64)
        .bind(
            "operation_id",
            item.operation_id.map(|operation_id| operation_id.to_string()),
        )
        .bind("status", aleph_bft_finalized_item_status_to_str(item.status))
        .bind("unit_body_hash", item.unit_body_hash.to_hex())
        .bind("item_json", serde_json::to_string(item)?)
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            return Err(federation_aleph_bft_finalized_item_conflict_error(item));
        }

        Ok(())
    }

    async fn append_federation_aleph_bft_backup_bytes(
        &mut self,
        session: u64,
        bytes: &[u8],
    ) -> Result<(), Self::Err> {
        if bytes.is_empty() {
            return Ok(());
        }

        query(
            r#"
            INSERT INTO federation_aleph_bft_backups
            (session, chunk_index, backup_bytes)
            SELECT
                :session,
                COALESCE(MAX(chunk_index), -1) + 1,
                :backup_bytes
            FROM federation_aleph_bft_backups
            WHERE session = :session
            "#,
        )?
        .bind("session", session as i64)
        .bind("backup_bytes", bytes)
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn replace_federation_aleph_bft_backup_chunks(
        &mut self,
        session: u64,
        chunks: &[Vec<u8>],
    ) -> Result<(), Self::Err> {
        query(
            r#"
            DELETE FROM federation_aleph_bft_backups
            WHERE session = :session
            "#,
        )?
        .bind("session", session as i64)
        .execute(&self.inner)
        .await?;

        for chunk in chunks {
            self.append_federation_aleph_bft_backup_bytes(session, chunk)
                .await?;
        }

        Ok(())
    }

    async fn set_federation_aleph_bft_active_session(
        &mut self,
        session: u64,
    ) -> Result<(), Self::Err> {
        query(
            r#"
            INSERT INTO federation_aleph_bft_session_state
            (session_key, active_session)
            VALUES
            (:session_key, :active_session)
            ON CONFLICT (session_key)
            DO UPDATE SET active_session =
                CASE
                    WHEN federation_aleph_bft_session_state.active_session < EXCLUDED.active_session
                    THEN EXCLUDED.active_session
                    ELSE federation_aleph_bft_session_state.active_session
                END
            "#,
        )?
        .bind("session_key", FEDERATION_ALEPH_BFT_ACTIVE_SESSION_KEY)
        .bind("active_session", session as i64)
        .execute(&self.inner)
        .await?;

        let active_session_row = query(
            r#"
            SELECT active_session
            FROM federation_aleph_bft_session_state
            WHERE session_key = :session_key
            FOR UPDATE
            "#,
        )?
        .bind("session_key", FEDERATION_ALEPH_BFT_ACTIVE_SESSION_KEY)
        .fetch_one(&self.inner)
        .await?
        .ok_or_else(|| {
            Error::Internal(
                "Federation AlephBFT active session disappeared during update".to_string(),
            )
        })?;
        let active_session_column = active_session_row.into_iter().next().ok_or_else(|| {
            Error::Internal(
                "Federation AlephBFT active session row had no value during update".to_string(),
            )
        })?;
        let active_session: u64 = column_as_number!(active_session_column);

        let pending_operations = query(
            r#"
            SELECT
                operation_id,
                session,
                envelope_json
            FROM federation_aleph_bft_pending_operations
            ORDER BY operation_id ASC
            FOR UPDATE
            "#,
        )?
        .fetch_all(&self.inner)
        .await?
        .into_iter()
        .map(sql_row_to_federation_aleph_bft_pending_operation)
        .collect::<Result<Vec<_>, _>>()?;
        if let Some(future) = pending_operations
            .iter()
            .find(|operation| operation.session_id > active_session)
        {
            return Err(Error::Internal(format!(
                "Federation AlephBFT pending operation {} belongs to future session {} while active session is {}",
                future.operation_id, future.session_id, active_session
            )));
        }

        query(
            r#"
            UPDATE federation_aleph_bft_pending_operations
            SET session = :active_session
            WHERE session < :active_session
            "#,
        )?
        .bind("active_session", active_session as i64)
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn add_federation_aleph_bft_pending_operation(
        &mut self,
        operation: &FederationAlephBftPendingOperationRecord,
    ) -> Result<(), Self::Err> {
        let computed_operation_id = operation.envelope.operation_id().map_err(|err| {
            Error::Internal(format!(
                "Invalid federation AlephBFT pending operation envelope: {err}"
            ))
        })?;
        if computed_operation_id != operation.operation_id {
            return Err(Error::Internal(format!(
                "Federation AlephBFT pending operation ID mismatch: stored {}, envelope {}",
                operation.operation_id, computed_operation_id
            )));
        }

        let active_session_row = query(
            r#"
            SELECT active_session
            FROM federation_aleph_bft_session_state
            WHERE session_key = :session_key
            FOR UPDATE
            "#,
        )?
        .bind("session_key", FEDERATION_ALEPH_BFT_ACTIVE_SESSION_KEY)
        .fetch_one(&self.inner)
        .await?;
        let active_session = match active_session_row {
            Some(row) => {
                let column = row.into_iter().next().ok_or_else(|| {
                    Error::Internal(
                        "Federation AlephBFT active session row had no value during pending insert"
                            .to_string(),
                    )
                })?;
                Some(column_as_number!(column))
            }
            None => None,
        };
        if active_session.is_some_and(|active| operation.session_id < active) {
            return Err(Error::Internal(format!(
                "Federation AlephBFT pending operation {} belongs to stale session {}",
                operation.operation_id, operation.session_id
            )));
        }

        query(
            r#"
            INSERT INTO federation_aleph_bft_pending_operations
            (operation_id, session, envelope_json)
            VALUES
            (:operation_id, :session, :envelope_json)
            ON CONFLICT (operation_id)
            DO UPDATE SET
                session = EXCLUDED.session,
                envelope_json = EXCLUDED.envelope_json
            "#,
        )?
        .bind("operation_id", operation.operation_id.to_string())
        .bind("session", operation.session_id as i64)
        .bind("envelope_json", serde_json::to_string(&operation.envelope)?)
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn remove_federation_aleph_bft_pending_operation(
        &mut self,
        operation_id: FederationOperationId,
    ) -> Result<(), Self::Err> {
        query(
            r#"
            DELETE FROM federation_aleph_bft_pending_operations
            WHERE operation_id = :operation_id
            "#,
        )?
        .bind("operation_id", operation_id.to_string())
        .execute(&self.inner)
        .await?;

        Ok(())
    }
}

#[async_trait]
impl<RM> FederationJournalDatabase for SQLMintDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn get_federation_config(
        &self,
        federation_id: FederationId,
    ) -> Result<Option<FederationConfigSnapshot>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        let mut rows = query(
            r#"
            SELECT
                federation_id,
                config_digest,
                config_json
            FROM federation_config
            WHERE federation_id = :federation_id
            "#,
        )?
        .bind("federation_id", federation_id.to_string())
        .fetch_all(&*conn)
        .await?
        .into_iter();
        match rows.next() {
            Some(row) => sql_row_to_federation_config_snapshot(row).map(Some),
            None => Ok(None),
        }
    }

    async fn get_federation_journal_entries(
        &self,
    ) -> Result<Vec<FederationJournalEntry>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(federation_journal_entry_select!(
            "ORDER BY operation_index ASC"
        ))?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_journal_entry)
        .collect()
    }

    async fn get_federation_journal_tip(&self) -> Result<Option<FederationJournalTip>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT operation_index, order_digest, status
            FROM federation_journal_entries
            ORDER BY operation_index DESC
            LIMIT 1
            "#,
        )?
        .fetch_one(&*conn)
        .await?
        .map(sql_row_to_federation_journal_tip)
        .transpose()
    }

    async fn get_federation_journal_entry_by_operation_id(
        &self,
        operation_id: FederationOperationId,
    ) -> Result<Option<FederationJournalEntry>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(federation_journal_entry_select!(
            "WHERE operation_id = :operation_id"
        ))?
        .bind("operation_id", operation_id.to_string())
        .fetch_one(&*conn)
        .await?
        .map(sql_row_to_federation_journal_entry)
        .transpose()
    }

    async fn get_federation_journal_entries_from(
        &self,
        next_index: u64,
    ) -> Result<Vec<FederationJournalEntry>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(federation_journal_entry_select!(
            "WHERE operation_index >= :next_index ORDER BY operation_index ASC"
        ))?
        .bind("next_index", next_index as i64)
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_journal_entry)
        .collect()
    }

    async fn get_federation_dkg_member_sequences(
        &self,
    ) -> Result<std::collections::BTreeMap<MemberId, u64>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        let rows = query(
            r#"
            SELECT member_id, last_sequence
            FROM federation_dkg_member_sequences
            ORDER BY member_id ASC
            "#,
        )?
        .fetch_all(&*conn)
        .await?;

        let mut sequences = std::collections::BTreeMap::new();
        for row in rows {
            unpack_into!(let (member_id, last_sequence) = row);
            let member_id_raw: u64 = column_as_number!(member_id);
            let member_id = MemberId::new(member_id_raw as u16)
                .map_err(|e| Error::Internal(format!("Invalid federation member id: {e}")))?;
            let last_sequence: u64 = column_as_number!(last_sequence);
            sequences.insert(member_id, last_sequence);
        }

        Ok(sequences)
    }

    async fn get_federation_catch_up_certificates(
        &self,
    ) -> Result<Vec<FederationCatchUpCertificate>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT certificate_json
            FROM federation_catch_up_certificates
            ORDER BY from_index ASC, signer_member_id ASC
            "#,
        )?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_catch_up_certificate)
        .collect()
    }

    async fn get_federation_catch_up_certificates_from(
        &self,
        from_index: u64,
    ) -> Result<Vec<FederationCatchUpCertificate>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT certificate_json
            FROM federation_catch_up_certificates
            WHERE from_index >= :from_index
            ORDER BY from_index ASC, signer_member_id ASC
            "#,
        )?
        .bind("from_index", from_index as i64)
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_catch_up_certificate)
        .collect()
    }

    async fn get_federation_catch_up_certificates_at(
        &self,
        from_index: u64,
    ) -> Result<Vec<FederationCatchUpCertificate>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT certificate_json
            FROM federation_catch_up_certificates
            WHERE from_index = :from_index
            ORDER BY signer_member_id ASC
            "#,
        )?
        .bind("from_index", from_index as i64)
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_catch_up_certificate)
        .collect()
    }

    async fn get_federation_aleph_bft_finalized_items(
        &self,
    ) -> Result<Vec<FederationAlephBftFinalizedItem>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(federation_aleph_bft_finalized_item_select!(
            "ORDER BY accepted_item_index ASC"
        ))?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_aleph_bft_finalized_item)
        .collect()
    }

    async fn get_federation_aleph_bft_finalized_items_from(
        &self,
        accepted_item_index: u64,
    ) -> Result<Vec<FederationAlephBftFinalizedItem>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(federation_aleph_bft_finalized_item_select!(
            "WHERE accepted_item_index >= :accepted_item_index ORDER BY accepted_item_index ASC"
        ))?
        .bind("accepted_item_index", accepted_item_index as i64)
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_aleph_bft_finalized_item)
        .collect()
    }

    async fn get_federation_aleph_bft_backup_bytes(
        &self,
        session: u64,
    ) -> Result<Option<Vec<u8>>, Self::Err> {
        let chunks = self.get_federation_aleph_bft_backup_chunks(session).await?;
        if chunks.is_empty() {
            return Ok(None);
        }

        Ok(Some(chunks.concat()))
    }

    async fn get_federation_aleph_bft_backup_chunks(
        &self,
        session: u64,
    ) -> Result<Vec<Vec<u8>>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        let rows = query(
            r#"
            SELECT backup_bytes
            FROM federation_aleph_bft_backups
            WHERE session = :session
            ORDER BY chunk_index ASC
            "#,
        )?
        .bind("session", session as i64)
        .fetch_all(&*conn)
        .await?;

        let mut chunks = Vec::with_capacity(rows.len());
        for mut row in rows {
            let bytes = row.remove(0);
            chunks.push(column_as_binary!(&bytes).to_vec());
        }

        Ok(chunks)
    }

    async fn get_federation_aleph_bft_backup_chunks_from(
        &self,
        session: u64,
        from_chunk: u64,
        limit: usize,
    ) -> Result<Vec<Vec<u8>>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        let rows = query(
            r#"
            SELECT backup_bytes
            FROM federation_aleph_bft_backups
            WHERE session = :session AND chunk_index >= :from_chunk
            ORDER BY chunk_index ASC
            LIMIT :limit
            "#,
        )?
        .bind("session", session as i64)
        .bind("from_chunk", from_chunk as i64)
        .bind("limit", limit as i64)
        .fetch_all(&*conn)
        .await?;

        let mut chunks = Vec::with_capacity(rows.len());
        for mut row in rows {
            let bytes = row.remove(0);
            chunks.push(column_as_binary!(&bytes).to_vec());
        }

        Ok(chunks)
    }

    async fn get_federation_aleph_bft_active_session(&self) -> Result<Option<u64>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT active_session
            FROM federation_aleph_bft_session_state
            WHERE session_key = :session_key
            "#,
        )?
        .bind("session_key", FEDERATION_ALEPH_BFT_ACTIVE_SESSION_KEY)
        .fetch_one(&*conn)
        .await?
        .map(|mut row| {
            let active_session = row.remove(0);
            Ok(column_as_number!(active_session))
        })
        .transpose()
    }

    async fn get_federation_aleph_bft_pending_operations(
        &self,
        session: u64,
    ) -> Result<Vec<FederationAlephBftPendingOperationRecord>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT
                operation_id,
                session,
                envelope_json
            FROM federation_aleph_bft_pending_operations
            WHERE session = :session
            ORDER BY operation_id ASC
            "#,
        )?
        .bind("session", session as i64)
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_aleph_bft_pending_operation)
        .collect()
    }
}
