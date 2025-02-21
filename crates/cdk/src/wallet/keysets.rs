use tracing::instrument;

use crate::nuts::{Id, KeySetInfo, Keys};
use crate::{Error, Wallet};

impl Wallet {
    /// Get keys for mint keyset
    ///
    /// Selected keys from localstore if they are already known
    /// If they are not known queries mint for keyset id and stores the [`Keys`]
    #[instrument(skip(self))]
    pub async fn get_keyset_keys(&self, keyset_id: Id) -> Result<Keys, Error> {
        let keys = if let Some(keys) = self.localstore.get_keys(&keyset_id).await? {
            keys
        } else {
            let keys = self.client.get_mint_keyset(keyset_id).await?;

            keys.verify_id()?;

            self.localstore.add_keys(keys.keys.clone()).await?;

            keys.keys
        };

        Ok(keys)
    }

    /// Add a keyset to the local database and update keyset info
    pub async fn add_keyset(
        &self,
        keys: Keys,
        active: bool,
        input_fee_ppk: u64,
    ) -> Result<(), Error> {
        self.localstore.add_keys(keys.clone()).await?;

        let keyset_info = KeySetInfo {
            id: Id::from(&keys),
            active,
            unit: self.unit.clone(),
            input_fee_ppk,
        };

        self.localstore
            .add_mint_keysets(self.mint_url.clone(), vec![keyset_info])
            .await?;

        Ok(())
    }

    /// Get keysets for mint
    ///
    /// Queries mint for all keysets
    #[instrument(skip(self))]
    pub async fn get_mint_keysets(&self) -> Result<Vec<KeySetInfo>, Error> {
        let keysets = self.client.get_mint_keysets().await?;

        self.localstore
            .add_mint_keysets(self.mint_url.clone(), keysets.keysets.clone())
            .await?;

        Ok(keysets.keysets)
    }

    /// Get active keyset for mint
    ///
    /// Queries mint for current keysets then gets [`Keys`] for any unknown
    /// keysets
    #[instrument(skip(self))]
    pub async fn get_active_mint_keysets(&self) -> Result<Vec<KeySetInfo>, Error> {
        let keysets = self.client.get_mint_keysets().await?;
        let keysets = keysets.keysets;

        self.localstore
            .add_mint_keysets(self.mint_url.clone(), keysets.clone())
            .await?;

        let active_keysets = keysets
            .clone()
            .into_iter()
            .filter(|k| k.active && k.unit == self.unit)
            .collect::<Vec<KeySetInfo>>();

        match self
            .localstore
            .get_mint_keysets(self.mint_url.clone())
            .await?
        {
            Some(known_keysets) => {
                let unknown_keysets: Vec<&KeySetInfo> = keysets
                    .iter()
                    .filter(|k| known_keysets.contains(k))
                    .collect();

                for keyset in unknown_keysets {
                    self.get_keyset_keys(keyset.id).await?;
                }
            }
            None => {
                for keyset in keysets {
                    self.get_keyset_keys(keyset.id).await?;
                }
            }
        }

        Ok(active_keysets)
    }

    /// Get active keyset for mint with the lowest fees
    ///
    /// Queries mint for current keysets then gets [`Keys`] for any unknown
    /// keysets
    #[instrument(skip(self))]
    pub async fn get_active_mint_keyset(&self) -> Result<KeySetInfo, Error> {
        let active_keysets = self.get_active_mint_keysets().await?;

        let keyset_with_lowest_fee = active_keysets
            .into_iter()
            .min_by_key(|key| key.input_fee_ppk)
            .ok_or(Error::NoActiveKeyset)?;
        Ok(keyset_with_lowest_fee)
    }

    /// Get active keyset for mint from local without querying the mint
    #[instrument(skip(self))]
    pub async fn get_active_mint_keyset_local(&self) -> Result<KeySetInfo, Error> {
        let active_keysets = match self
            .localstore
            .get_mint_keysets(self.mint_url.clone())
            .await?
        {
            Some(keysets) => keysets
                .into_iter()
                .filter(|k| k.active && k.unit == self.unit)
                .collect::<Vec<KeySetInfo>>(),
            None => {
                vec![]
            }
        };

        let keyset_with_lowest_fee = active_keysets
            .into_iter()
            .min_by_key(|key| key.input_fee_ppk)
            .ok_or(Error::NoActiveKeyset)?;

        Ok(keyset_with_lowest_fee)
    }
}

#[cfg(test)]
mod test {
    use crate::cdk_database::WalletMemoryDatabase;
    use crate::nuts;
    use crate::Wallet;
    use bitcoin::bip32::DerivationPath;
    use bitcoin::bip32::Xpriv;
    use bitcoin::key::Secp256k1;
    use cdk_common::KeySet;
    use cdk_common::KeySetInfo;
    use cdk_common::MintKeySet;
    use nuts::CurrencyUnit;
    use std::sync::Arc;

    fn create_new_keyset() -> (KeySet, KeySetInfo) {
        let secp = Secp256k1::new();
        let seed = [0u8; 32]; // Default seed for testing
        let xpriv = Xpriv::new_master(bitcoin::Network::Bitcoin, &seed).expect("RNG busted");

        let derivation_path = DerivationPath::default();
        let unit = CurrencyUnit::Custom("HASH".to_string());
        let max_order = 64;

        let keyset: KeySet = MintKeySet::generate(
            &secp,
            xpriv
                .derive_priv(&secp, &derivation_path)
                .expect("RNG busted"),
            unit.clone(),
            max_order,
        )
        .into();

        let keyset_info = KeySetInfo {
            id: keyset.id,
            unit: keyset.unit.clone(),
            active: true,
            input_fee_ppk: 0,
        };

        (keyset, keyset_info)
    }

    fn create_wallet() -> Wallet {
        use rand::Rng;

        let seed = rand::thread_rng().gen::<[u8; 32]>();
        let mint_url = "https://testnut.cashu.space";

        let localstore = WalletMemoryDatabase::default();
        Wallet::new(
            mint_url,
            CurrencyUnit::Custom("HASH".to_string()),
            Arc::new(localstore),
            &seed,
            None,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn test_add_and_get_active_mint_keysets_local() {
        let (keyset, keyset_info) = create_new_keyset();

        let wallet = create_wallet();

        // Add the keyset
        wallet.add_keyset(keyset.keys, true, 0).await.unwrap();

        // Retrieve the keysets locally
        let active_keyset = wallet.get_active_mint_keyset_local().await.unwrap();

        // Validate the retrieved keyset
        assert_eq!(active_keyset.id, keyset_info.id);
        assert_eq!(active_keyset.active, keyset_info.active);
        assert_eq!(active_keyset.unit, keyset_info.unit);
        assert_eq!(active_keyset.input_fee_ppk, keyset_info.input_fee_ppk);
    }
}
