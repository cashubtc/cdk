use super::*;

fn setup_conflict(
    artifact: &'static str,
    setup_id: FederationSetupId,
    coordinate: impl Into<String>,
) -> Error {
    Error::Federation(
        cdk_common::federation::FederationError::SetupPersistenceConflict {
            artifact,
            setup_id,
            coordinate: coordinate.into(),
        },
    )
}

fn setup_json<T>(value: &T) -> Result<String, Error>
where
    T: Serialize,
{
    serde_json::to_string(value).map_err(Error::from)
}

fn setup_record<T>(row: Vec<Column>, artifact: &'static str) -> Result<T, Error>
where
    T: DeserializeOwned,
{
    unpack_into!(let (payload_json) = row);
    serde_json::from_str(&column_as_string!(&payload_json)).map_err(|error| {
        Error::Internal(format!("Invalid federation setup {artifact} JSON: {error}"))
    })
}

fn setup_records<T>(rows: Vec<Vec<Column>>, artifact: &'static str) -> Result<Vec<T>, Error>
where
    T: DeserializeOwned,
{
    rows.into_iter()
        .map(|row| setup_record(row, artifact))
        .collect()
}

fn sql_i64(label: &'static str, value: u64) -> Result<i64, Error> {
    i64::try_from(value)
        .map_err(|_| Error::Internal(format!("Federation setup {label} exceeds SQL i64")))
}

fn local_role_to_str(role: FederationSetupLocalRole) -> &'static str {
    match role {
        FederationSetupLocalRole::Coordinator { .. } => "coordinator",
        FederationSetupLocalRole::Member { .. } => "member",
    }
}

fn coordinator_phase_to_str(phase: FederationSetupCoordinatorPhase) -> &'static str {
    match phase {
        FederationSetupCoordinatorPhase::CollectingEnrollments => "collecting_enrollments",
        FederationSetupCoordinatorPhase::ProposalPublished => "proposal_published",
        FederationSetupCoordinatorPhase::Certified => "certified",
        FederationSetupCoordinatorPhase::Closed => "closed",
        FederationSetupCoordinatorPhase::Aborted => "aborted",
    }
}

fn participant_phase_to_str(phase: FederationSetupParticipantPhase) -> &'static str {
    match phase {
        FederationSetupParticipantPhase::IdentityCreated => "identity_created",
        FederationSetupParticipantPhase::Enrolled => "enrolled",
        FederationSetupParticipantPhase::ProposalReviewed => "proposal_reviewed",
        FederationSetupParticipantPhase::Approved => "approved",
        FederationSetupParticipantPhase::Certified => "certified",
        FederationSetupParticipantPhase::DkgRunning => "dkg_running",
        FederationSetupParticipantPhase::ReadyToActivate => "ready_to_activate",
        FederationSetupParticipantPhase::Activated => "activated",
        FederationSetupParticipantPhase::Aborted => "aborted",
    }
}

fn invitation_state_to_str(state: FederationSetupInvitationState) -> &'static str {
    match state {
        FederationSetupInvitationState::Available => "available",
        FederationSetupInvitationState::Consumed => "consumed",
        FederationSetupInvitationState::Expired => "expired",
    }
}

#[async_trait]
impl<RM> FederationSetupTransaction for SQLTransaction<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn add_federation_setup_session(
        &mut self,
        session: &FederationSetupSession,
    ) -> Result<(), Self::Err> {
        session.validate()?;
        let rows_affected = query(
            r#"
            INSERT INTO federation_setup_sessions
            (setup_id, local_role, local_member_id, coordinator_phase,
             participant_phase, created_at_unix, updated_at_unix, payload_json)
            VALUES
            (:setup_id, :local_role, :local_member_id, :coordinator_phase,
             :participant_phase, :created_at_unix, :updated_at_unix, :payload_json)
            ON CONFLICT (setup_id)
            DO UPDATE SET payload_json = EXCLUDED.payload_json
            WHERE federation_setup_sessions.payload_json = EXCLUDED.payload_json
            "#,
        )?
        .bind("setup_id", session.setup_id.to_string())
        .bind("local_role", local_role_to_str(session.local_role))
        .bind(
            "local_member_id",
            i64::from(session.local_role.member_id().get()),
        )
        .bind(
            "coordinator_phase",
            session.coordinator_phase.map(coordinator_phase_to_str),
        )
        .bind(
            "participant_phase",
            participant_phase_to_str(session.participant_phase),
        )
        .bind(
            "created_at_unix",
            sql_i64("creation time", session.created_at_unix)?,
        )
        .bind(
            "updated_at_unix",
            sql_i64("update time", session.updated_at_unix)?,
        )
        .bind("payload_json", setup_json(session)?)
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            return Err(setup_conflict("session", session.setup_id, "local"));
        }
        Ok(())
    }

    async fn transition_federation_setup_session(
        &mut self,
        expected: &FederationSetupSession,
        updated: &FederationSetupSession,
    ) -> Result<(), Self::Err> {
        expected.validate_transition_to(updated)?;
        let rows_affected = query(
            r#"
            UPDATE federation_setup_sessions
            SET coordinator_phase = :coordinator_phase,
                participant_phase = :participant_phase,
                updated_at_unix = :updated_at_unix,
                payload_json = :updated_payload_json
            WHERE setup_id = :setup_id
              AND (payload_json = :expected_payload_json OR payload_json = :updated_payload_json)
            "#,
        )?
        .bind("setup_id", expected.setup_id.to_string())
        .bind(
            "coordinator_phase",
            updated.coordinator_phase.map(coordinator_phase_to_str),
        )
        .bind(
            "participant_phase",
            participant_phase_to_str(updated.participant_phase),
        )
        .bind(
            "updated_at_unix",
            sql_i64("update time", updated.updated_at_unix)?,
        )
        .bind("expected_payload_json", setup_json(expected)?)
        .bind("updated_payload_json", setup_json(updated)?)
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            return Err(setup_conflict("session", expected.setup_id, "local"));
        }
        Ok(())
    }

    async fn add_federation_setup_invitation(
        &mut self,
        invitation: &FederationSetupInvitationRecord,
    ) -> Result<(), Self::Err> {
        invitation.validate()?;
        let payload_json = setup_json(invitation)?;
        let rows_affected = query(
            r#"
            INSERT INTO federation_setup_invitations
            (setup_id, invitation_id, member_id, expires_at_unix,
             capability_hash, invitation_state, payload_json)
            VALUES
            (:setup_id, :invitation_id, :member_id, :expires_at_unix,
             :capability_hash, :invitation_state, :payload_json)
            ON CONFLICT DO NOTHING
            "#,
        )?
        .bind("setup_id", invitation.setup_id.to_string())
        .bind("invitation_id", invitation.invitation_id.to_string())
        .bind("member_id", i64::from(invitation.member_id.get()))
        .bind(
            "expires_at_unix",
            sql_i64("invitation expiry", invitation.expires_at_unix)?,
        )
        .bind("capability_hash", invitation.capability_hash.to_string())
        .bind(
            "invitation_state",
            invitation_state_to_str(invitation.state),
        )
        .bind("payload_json", payload_json.clone())
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            let existing = query(
                r#"
                SELECT payload_json
                FROM federation_setup_invitations
                WHERE setup_id = :setup_id
                  AND (invitation_id = :invitation_id OR capability_hash = :capability_hash)
                "#,
            )?
            .bind("setup_id", invitation.setup_id.to_string())
            .bind("invitation_id", invitation.invitation_id.to_string())
            .bind("capability_hash", invitation.capability_hash.to_string())
            .pluck(&self.inner)
            .await?;
            if !matches!(existing, Some(Column::Text(existing)) if existing == payload_json) {
                return Err(setup_conflict(
                    "invitation",
                    invitation.setup_id,
                    invitation.invitation_id.to_string(),
                ));
            }
        }
        Ok(())
    }

    async fn transition_federation_setup_invitation(
        &mut self,
        expected: &FederationSetupInvitationRecord,
        updated: &FederationSetupInvitationRecord,
    ) -> Result<(), Self::Err> {
        expected.validate_transition_to(updated)?;
        let rows_affected = query(
            r#"
            UPDATE federation_setup_invitations
            SET invitation_state = :invitation_state,
                payload_json = :updated_payload_json
            WHERE setup_id = :setup_id
              AND invitation_id = :invitation_id
              AND (payload_json = :expected_payload_json OR payload_json = :updated_payload_json)
            "#,
        )?
        .bind("setup_id", expected.setup_id.to_string())
        .bind("invitation_id", expected.invitation_id.to_string())
        .bind("invitation_state", invitation_state_to_str(updated.state))
        .bind("expected_payload_json", setup_json(expected)?)
        .bind("updated_payload_json", setup_json(updated)?)
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            return Err(setup_conflict(
                "invitation",
                expected.setup_id,
                expected.invitation_id.to_string(),
            ));
        }
        Ok(())
    }

    async fn add_federation_setup_enrollment(
        &mut self,
        enrollment: &FederationSetupEnrollment,
    ) -> Result<(), Self::Err> {
        enrollment
            .validate(cdk_common::federation::FederationSetupUrlPolicy::AllowInsecureLocalhost)?;
        let payload_json = setup_json(enrollment)?;
        let rows_affected = query(
            r#"
            INSERT INTO federation_setup_enrollments
            (setup_id, member_id, invitation_id, payload_json)
            VALUES (:setup_id, :member_id, :invitation_id, :payload_json)
            ON CONFLICT DO NOTHING
            "#,
        )?
        .bind("setup_id", enrollment.setup_id.to_string())
        .bind("member_id", i64::from(enrollment.member_id.get()))
        .bind("invitation_id", enrollment.invitation_id.to_string())
        .bind("payload_json", payload_json.clone())
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            let existing = query(
                r#"
                SELECT payload_json
                FROM federation_setup_enrollments
                WHERE setup_id = :setup_id
                  AND (member_id = :member_id OR invitation_id = :invitation_id)
                "#,
            )?
            .bind("setup_id", enrollment.setup_id.to_string())
            .bind("member_id", i64::from(enrollment.member_id.get()))
            .bind("invitation_id", enrollment.invitation_id.to_string())
            .pluck(&self.inner)
            .await?;
            if !matches!(existing, Some(Column::Text(existing)) if existing == payload_json) {
                return Err(setup_conflict(
                    "enrollment",
                    enrollment.setup_id,
                    enrollment.member_id.to_string(),
                ));
            }
        }
        Ok(())
    }

    async fn add_federation_setup_proposal(
        &mut self,
        proposal: &FederationSetupProposal,
    ) -> Result<(), Self::Err> {
        proposal
            .validate(cdk_common::federation::FederationSetupUrlPolicy::AllowInsecureLocalhost)?;
        let rows_affected = query(
            r#"
            INSERT INTO federation_setup_proposals
            (setup_id, proposal_hash, payload_json)
            VALUES (:setup_id, :proposal_hash, :payload_json)
            ON CONFLICT (setup_id)
            DO UPDATE SET payload_json = EXCLUDED.payload_json
            WHERE federation_setup_proposals.payload_json = EXCLUDED.payload_json
            "#,
        )?
        .bind("setup_id", proposal.body.setup_id.to_string())
        .bind("proposal_hash", proposal.proposal_hash.to_string())
        .bind("payload_json", setup_json(proposal)?)
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            return Err(setup_conflict(
                "proposal",
                proposal.body.setup_id,
                proposal.proposal_hash.to_string(),
            ));
        }
        Ok(())
    }

    async fn add_federation_setup_approval_fence(
        &mut self,
        approval: &FederationSetupApproval,
    ) -> Result<(), Self::Err> {
        insert_approval(&self.inner, "federation_setup_approval_fences", approval).await
    }

    async fn add_federation_setup_approval(
        &mut self,
        approval: &FederationSetupApproval,
    ) -> Result<(), Self::Err> {
        insert_approval(&self.inner, "federation_setup_approvals", approval).await
    }

    async fn add_federation_setup_manifest(
        &mut self,
        manifest: &FederationSetupManifest,
    ) -> Result<(), Self::Err> {
        manifest
            .validate(cdk_common::federation::FederationSetupUrlPolicy::AllowInsecureLocalhost)?;
        let setup_id = manifest.proposal.body.setup_id;
        let rows_affected = query(
            r#"
            INSERT INTO federation_setup_manifests
            (setup_id, proposal_hash, manifest_hash, payload_json)
            VALUES (:setup_id, :proposal_hash, :manifest_hash, :payload_json)
            ON CONFLICT (setup_id)
            DO UPDATE SET payload_json = EXCLUDED.payload_json
            WHERE federation_setup_manifests.payload_json = EXCLUDED.payload_json
            "#,
        )?
        .bind("setup_id", setup_id.to_string())
        .bind("proposal_hash", manifest.proposal.proposal_hash.to_string())
        .bind("manifest_hash", manifest.manifest_hash.to_string())
        .bind("payload_json", setup_json(manifest)?)
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            return Err(setup_conflict(
                "manifest",
                setup_id,
                manifest.manifest_hash.to_string(),
            ));
        }
        Ok(())
    }

    async fn add_federation_setup_activation(
        &mut self,
        activation: &FederationSetupActivation,
    ) -> Result<(), Self::Err> {
        activation.validate()?;
        let rows_affected = query(
            r#"
            INSERT INTO federation_setup_activations
            (setup_id, member_id, manifest_hash, setup_authorization,
             federation_id, final_config_digest, activated_at_unix, payload_json)
            VALUES
            (:setup_id, :member_id, :manifest_hash, :setup_authorization,
             :federation_id, :final_config_digest, :activated_at_unix, :payload_json)
            ON CONFLICT (setup_id, member_id)
            DO UPDATE SET payload_json = EXCLUDED.payload_json
            WHERE federation_setup_activations.payload_json = EXCLUDED.payload_json
            "#,
        )?
        .bind("setup_id", activation.setup_id.to_string())
        .bind("member_id", i64::from(activation.member_id.get()))
        .bind("manifest_hash", activation.manifest_hash.to_string())
        .bind(
            "setup_authorization",
            activation.setup_authorization.to_string(),
        )
        .bind("federation_id", activation.federation_id.to_string())
        .bind(
            "final_config_digest",
            activation.final_config_digest.to_string(),
        )
        .bind(
            "activated_at_unix",
            sql_i64("activation time", activation.activated_at_unix)?,
        )
        .bind("payload_json", setup_json(activation)?)
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            return Err(setup_conflict(
                "activation",
                activation.setup_id,
                activation.member_id.to_string(),
            ));
        }
        Ok(())
    }
}

async fn insert_approval<C>(
    connection: &C,
    table: &'static str,
    approval: &FederationSetupApproval,
) -> Result<(), Error>
where
    C: DatabaseExecutor,
{
    let sql = match table {
        "federation_setup_approval_fences" => {
            r#"
            INSERT INTO federation_setup_approval_fences
            (setup_id, member_id, proposal_hash, payload_json)
            VALUES (:setup_id, :member_id, :proposal_hash, :payload_json)
            ON CONFLICT (setup_id, member_id)
            DO UPDATE SET payload_json = EXCLUDED.payload_json
            WHERE federation_setup_approval_fences.payload_json = EXCLUDED.payload_json
            "#
        }
        "federation_setup_approvals" => {
            r#"
            INSERT INTO federation_setup_approvals
            (setup_id, member_id, proposal_hash, payload_json)
            VALUES (:setup_id, :member_id, :proposal_hash, :payload_json)
            ON CONFLICT (setup_id, member_id)
            DO UPDATE SET payload_json = EXCLUDED.payload_json
            WHERE federation_setup_approvals.payload_json = EXCLUDED.payload_json
            "#
        }
        _ => return Err(Error::Internal("Invalid setup approval table".to_string())),
    };
    let rows_affected = query(sql)?
        .bind("setup_id", approval.setup_id.to_string())
        .bind("member_id", i64::from(approval.member_id.get()))
        .bind("proposal_hash", approval.proposal_hash.to_string())
        .bind("payload_json", setup_json(approval)?)
        .execute(connection)
        .await?;
    if rows_affected == 0 {
        return Err(setup_conflict(
            if table == "federation_setup_approval_fences" {
                "approval fence"
            } else {
                "approval"
            },
            approval.setup_id,
            approval.member_id.to_string(),
        ));
    }
    Ok(())
}

#[async_trait]
impl<RM> FederationSetupDatabase for SQLMintDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn get_federation_setup_session(
        &self,
        setup_id: FederationSetupId,
    ) -> Result<Option<FederationSetupSession>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|error| Error::Database(Box::new(error)))?;
        query("SELECT payload_json FROM federation_setup_sessions WHERE setup_id = :setup_id")?
            .bind("setup_id", setup_id.to_string())
            .fetch_one(&*conn)
            .await?
            .map(|row| setup_record(row, "session"))
            .transpose()
    }

    async fn get_federation_setup_invitations(
        &self,
        setup_id: FederationSetupId,
    ) -> Result<Vec<FederationSetupInvitationRecord>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|error| Error::Database(Box::new(error)))?;
        let rows = query(
            r#"
            SELECT payload_json
            FROM federation_setup_invitations
            WHERE setup_id = :setup_id
            ORDER BY member_id ASC, invitation_id ASC
            "#,
        )?
        .bind("setup_id", setup_id.to_string())
        .fetch_all(&*conn)
        .await?;
        setup_records(rows, "invitation")
    }

    async fn get_federation_setup_invitation(
        &self,
        setup_id: FederationSetupId,
        invitation_id: FederationSetupInvitationId,
    ) -> Result<Option<FederationSetupInvitationRecord>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|error| Error::Database(Box::new(error)))?;
        query(
            r#"
            SELECT payload_json
            FROM federation_setup_invitations
            WHERE setup_id = :setup_id AND invitation_id = :invitation_id
            "#,
        )?
        .bind("setup_id", setup_id.to_string())
        .bind("invitation_id", invitation_id.to_string())
        .fetch_one(&*conn)
        .await?
        .map(|row| setup_record(row, "invitation"))
        .transpose()
    }

    async fn get_federation_setup_enrollments(
        &self,
        setup_id: FederationSetupId,
    ) -> Result<Vec<FederationSetupEnrollment>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|error| Error::Database(Box::new(error)))?;
        let rows = query(
            r#"
            SELECT payload_json
            FROM federation_setup_enrollments
            WHERE setup_id = :setup_id
            ORDER BY member_id ASC
            "#,
        )?
        .bind("setup_id", setup_id.to_string())
        .fetch_all(&*conn)
        .await?;
        setup_records(rows, "enrollment")
    }

    async fn get_federation_setup_proposal(
        &self,
        setup_id: FederationSetupId,
    ) -> Result<Option<FederationSetupProposal>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|error| Error::Database(Box::new(error)))?;
        query("SELECT payload_json FROM federation_setup_proposals WHERE setup_id = :setup_id")?
            .bind("setup_id", setup_id.to_string())
            .fetch_one(&*conn)
            .await?
            .map(|row| setup_record(row, "proposal"))
            .transpose()
    }

    async fn get_federation_setup_approval_fence(
        &self,
        setup_id: FederationSetupId,
        member_id: MemberId,
    ) -> Result<Option<FederationSetupApproval>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|error| Error::Database(Box::new(error)))?;
        query(
            r#"
            SELECT payload_json
            FROM federation_setup_approval_fences
            WHERE setup_id = :setup_id AND member_id = :member_id
            "#,
        )?
        .bind("setup_id", setup_id.to_string())
        .bind("member_id", i64::from(member_id.get()))
        .fetch_one(&*conn)
        .await?
        .map(|row| setup_record(row, "approval fence"))
        .transpose()
    }

    async fn get_federation_setup_approvals(
        &self,
        setup_id: FederationSetupId,
    ) -> Result<Vec<FederationSetupApproval>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|error| Error::Database(Box::new(error)))?;
        let rows = query(
            r#"
            SELECT payload_json
            FROM federation_setup_approvals
            WHERE setup_id = :setup_id
            ORDER BY member_id ASC
            "#,
        )?
        .bind("setup_id", setup_id.to_string())
        .fetch_all(&*conn)
        .await?;
        setup_records(rows, "approval")
    }

    async fn get_federation_setup_manifest(
        &self,
        setup_id: FederationSetupId,
    ) -> Result<Option<FederationSetupManifest>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|error| Error::Database(Box::new(error)))?;
        query("SELECT payload_json FROM federation_setup_manifests WHERE setup_id = :setup_id")?
            .bind("setup_id", setup_id.to_string())
            .fetch_one(&*conn)
            .await?
            .map(|row| setup_record(row, "manifest"))
            .transpose()
    }

    async fn get_federation_setup_activations(
        &self,
        setup_id: FederationSetupId,
    ) -> Result<Vec<FederationSetupActivation>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|error| Error::Database(Box::new(error)))?;
        let rows = query(
            r#"
            SELECT payload_json
            FROM federation_setup_activations
            WHERE setup_id = :setup_id
            ORDER BY member_id ASC
            "#,
        )?
        .bind("setup_id", setup_id.to_string())
        .fetch_all(&*conn)
        .await?;
        setup_records(rows, "activation")
    }
}
