use super::*;

const ROOT_PUBLIC: &str = "root_public";
const ROOT_PRIVATE: &str = "root_private";
const APPLICATION: &str = "application";
const CONSUMER: &str = "consumer";
const ACTIVE_EPOCHS: &str = "active_epochs";

async fn insert_immutable<C, T>(
    connection: &C,
    record_kind: &str,
    root_key_epoch: u64,
    application_id: String,
    application_key_epoch: u64,
    payload: &T,
) -> Result<(), Error>
where
    C: DatabaseExecutor,
    T: Serialize,
{
    let rows_affected = query(
        r#"
        INSERT INTO federation_frost_wallet_config
        (record_kind, root_key_epoch, application_id, application_key_epoch, payload_json)
        VALUES (:record_kind, :root_key_epoch, :application_id, :application_key_epoch, :payload_json)
        ON CONFLICT (record_kind, root_key_epoch, application_id, application_key_epoch)
        DO UPDATE SET payload_json = EXCLUDED.payload_json
        WHERE federation_frost_wallet_config.payload_json = EXCLUDED.payload_json
        "#,
    )?
    .bind("record_kind", record_kind.to_string())
    .bind("root_key_epoch", root_key_epoch as i64)
    .bind("application_id", application_id)
    .bind("application_key_epoch", application_key_epoch as i64)
    .bind("payload_json", serde_json::to_string(payload)?)
    .execute(connection)
    .await?;
    if rows_affected == 0 {
        return Err(Error::Duplicate);
    }
    Ok(())
}

async fn fetch_records<C, T>(connection: &C, record_kind: &str) -> Result<Vec<T>, Error>
where
    C: DatabaseExecutor,
    T: DeserializeOwned,
{
    query(
        r#"
        SELECT payload_json
        FROM federation_frost_wallet_config
        WHERE record_kind = :record_kind
        ORDER BY root_key_epoch ASC, application_id ASC, application_key_epoch ASC
        "#,
    )?
    .bind("record_kind", record_kind.to_string())
    .fetch_all(connection)
    .await?
    .into_iter()
    .map(|row| {
        unpack_into!(let (payload_json) = row);
        serde_json::from_str(&column_as_string!(&payload_json)).map_err(|error| {
            Error::Internal(format!(
                "Invalid federation FROST wallet config JSON: {error}"
            ))
        })
    })
    .collect()
}

#[async_trait]
impl<RM> FederationFrostWalletTransaction for SQLTransaction<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn add_federation_frost_root_public_config(
        &mut self,
        config: &FederationFrostRootPublicConfig,
    ) -> Result<(), Self::Err> {
        insert_immutable(
            &self.inner,
            ROOT_PUBLIC,
            config.root_key_epoch,
            String::new(),
            0,
            config,
        )
        .await
    }

    async fn add_federation_frost_root_private_config(
        &mut self,
        config: &FederationFrostRootPrivateConfig,
    ) -> Result<(), Self::Err> {
        insert_immutable(
            &self.inner,
            ROOT_PRIVATE,
            config.root_key_epoch,
            String::new(),
            0,
            config,
        )
        .await
    }

    async fn add_federation_frost_application_config(
        &mut self,
        config: &FederationFrostApplicationConfig,
    ) -> Result<(), Self::Err> {
        insert_immutable(
            &self.inner,
            APPLICATION,
            config.root_key_epoch,
            config.application_id.to_string(),
            config.application_key_epoch,
            config,
        )
        .await
    }

    async fn add_federation_frost_consumer_registration(
        &mut self,
        registration: &FederationFrostConsumerRegistration,
    ) -> Result<(), Self::Err> {
        insert_immutable(
            &self.inner,
            CONSUMER,
            registration.root_key_epoch,
            registration.application_id.to_string(),
            registration.application_key_epoch,
            registration,
        )
        .await
    }

    async fn set_federation_frost_active_epochs(
        &mut self,
        active_epochs: &FederationFrostActiveEpochs,
    ) -> Result<(), Self::Err> {
        query(
            r#"
            INSERT INTO federation_frost_wallet_config
            (record_kind, root_key_epoch, application_id, application_key_epoch, payload_json)
            VALUES (:record_kind, 0, '', 0, :payload_json)
            ON CONFLICT (record_kind, root_key_epoch, application_id, application_key_epoch)
            DO UPDATE SET payload_json = EXCLUDED.payload_json
            "#,
        )?
        .bind("record_kind", ACTIVE_EPOCHS.to_string())
        .bind("payload_json", serde_json::to_string(active_epochs)?)
        .execute(&self.inner)
        .await?;
        Ok(())
    }
}

#[async_trait]
impl<RM> FederationFrostWalletDatabase for SQLMintDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn get_federation_frost_root_public_configs(
        &self,
    ) -> Result<Vec<FederationFrostRootPublicConfig>, Self::Err> {
        let connection = self
            .pool
            .get()
            .await
            .map_err(|error| Error::Database(Box::new(error)))?;
        fetch_records(&*connection, ROOT_PUBLIC).await
    }

    async fn get_federation_frost_root_private_configs(
        &self,
    ) -> Result<Vec<FederationFrostRootPrivateConfig>, Self::Err> {
        let connection = self
            .pool
            .get()
            .await
            .map_err(|error| Error::Database(Box::new(error)))?;
        fetch_records(&*connection, ROOT_PRIVATE).await
    }

    async fn get_federation_frost_application_configs(
        &self,
    ) -> Result<Vec<FederationFrostApplicationConfig>, Self::Err> {
        let connection = self
            .pool
            .get()
            .await
            .map_err(|error| Error::Database(Box::new(error)))?;
        fetch_records(&*connection, APPLICATION).await
    }

    async fn get_federation_frost_consumer_registrations(
        &self,
    ) -> Result<Vec<FederationFrostConsumerRegistration>, Self::Err> {
        let connection = self
            .pool
            .get()
            .await
            .map_err(|error| Error::Database(Box::new(error)))?;
        fetch_records(&*connection, CONSUMER).await
    }

    async fn get_federation_frost_active_epochs(
        &self,
    ) -> Result<Option<FederationFrostActiveEpochs>, Self::Err> {
        let connection = self
            .pool
            .get()
            .await
            .map_err(|error| Error::Database(Box::new(error)))?;
        let records: Vec<FederationFrostActiveEpochs> =
            fetch_records(&*connection, ACTIVE_EPOCHS).await?;
        match records.as_slice() {
            [] => Ok(None),
            [active] => Ok(Some(active.clone())),
            _ => Err(Error::Internal(
                "Multiple federation FROST active epoch records".to_string(),
            )),
        }
    }
}
