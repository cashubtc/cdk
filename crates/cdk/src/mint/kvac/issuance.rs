//! KVAC MAC issuance
use cashu_kvac::kvac::IssuanceProof;
use cashu_kvac::models::{MAC, ZKP};
use cdk_common::kvac::KvacCoinMessage;
use tracing::instrument;

use crate::{Error, Mint};

impl Mint {
    /// Issue a MAC
    #[instrument(skip_all)]
    pub async fn issue_mac(
        &self,
        input: &KvacCoinMessage,
    ) -> Result<(MAC, ZKP), Error> {
        let KvacCoinMessage {
            commitments,
            keyset_id,
            t_tag,
        } = input;
        self.ensure_kvac_keyset_loaded(keyset_id).await?;

        let keyset_info = self
            .localstore
            .get_kvac_keyset_info(keyset_id)
            .await?
            .ok_or(Error::UnknownKeySet)?;

        let active = self
            .localstore
            .get_active_kvac_keyset_id(&keyset_info.unit)
            .await?
            .ok_or(Error::InactiveKeyset)?;

        // Check that the keyset is active and should be used to sign
        if keyset_info.id.ne(&active) {
            return Err(Error::InactiveKeyset);
        }

        let keysets = self.kvac_keysets.read().await;
        let keyset = keysets.get(keyset_id).ok_or(Error::UnknownKeySet)?;

        let key_pair = &keyset.kvac_keys;

        let c = MAC::generate(
            &key_pair.private_key,
            &commitments.0,
            Some(&commitments.1),
            Some(t_tag),
        )
        .expect("MAC generate");
        let iparams_proof = IssuanceProof::create(
            &key_pair.private_key,
            &c,
            &commitments.0,
            Some(&commitments.1)
        );

        Ok((c, iparams_proof))
    }
}
