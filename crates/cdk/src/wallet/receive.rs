use std::collections::HashMap;
use std::str::FromStr;

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::XOnlyPublicKey;
use tracing::instrument;

use crate::amount::SplitTarget;
use crate::dhke::construct_proofs;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::nut10::Kind;
use crate::nuts::{Conditions, Proofs, PublicKey, SecretKey, SigFlag, State, Token};
use crate::types::ProofInfo;
use crate::util::hex;
use crate::{Amount, Error, Wallet, SECP256K1};

impl Wallet {
    /// Receive proofs
    #[instrument(skip_all)]
    pub async fn receive_proofs(
        &self,
        proofs: Proofs,
        amount_split_target: SplitTarget,
        p2pk_signing_keys: &[SecretKey],
        preimages: &[String],
    ) -> Result<Amount, Error> {
        let mint_url = &self.mint_url;
        // Add mint if it does not exist in the store
        if self
            .localstore
            .get_mint(self.mint_url.clone())
            .await?
            .is_none()
        {
            tracing::debug!("Mint not in localstore fetching info for: {mint_url}");
            self.get_mint_info().await?;
        }

        let _ = self.get_active_mint_keyset().await?;

        let active_keyset_id = self.get_active_mint_keyset().await?.id;

        let keys = self.get_keyset_keys(active_keyset_id).await?;

        let mut proofs = proofs;

        let mut sig_flag = SigFlag::SigInputs;

        // Map hash of preimage to preimage
        let hashed_to_preimage: HashMap<String, &String> = preimages
            .iter()
            .map(|p| {
                let hex_bytes = hex::decode(p)?;
                Ok::<(String, &String), Error>((Sha256Hash::hash(&hex_bytes).to_string(), p))
            })
            .collect::<Result<HashMap<String, &String>, _>>()?;

        let p2pk_signing_keys: HashMap<XOnlyPublicKey, &SecretKey> = p2pk_signing_keys
            .iter()
            .map(|s| (s.x_only_public_key(&SECP256K1).0, s))
            .collect();

        for proof in &mut proofs {
            // Verify that proof DLEQ is valid
            if proof.dleq.is_some() {
                let keys = self.get_keyset_keys(proof.keyset_id).await?;
                let key = keys.amount_key(proof.amount).ok_or(Error::AmountKey)?;
                proof.verify_dleq(key)?;
            }

            if let Ok(secret) =
                <crate::secret::Secret as TryInto<crate::nuts::nut10::Secret>>::try_into(
                    proof.secret.clone(),
                )
            {
                let conditions: Result<Conditions, _> =
                    secret.secret_data.tags.unwrap_or_default().try_into();
                if let Ok(conditions) = conditions {
                    let mut pubkeys = conditions.pubkeys.unwrap_or_default();

                    match secret.kind {
                        Kind::P2PK => {
                            let data_key = PublicKey::from_str(&secret.secret_data.data)?;

                            pubkeys.push(data_key);
                        }
                        Kind::HTLC => {
                            let hashed_preimage = &secret.secret_data.data;
                            let preimage = hashed_to_preimage
                                .get(hashed_preimage)
                                .ok_or(Error::PreimageNotProvided)?;
                            proof.add_preimage(preimage.to_string());
                        }
                    }
                    for pubkey in pubkeys {
                        if let Some(signing) = p2pk_signing_keys.get(&pubkey.x_only_public_key()) {
                            proof.sign_p2pk(signing.to_owned().clone())?;
                        }
                    }

                    if conditions.sig_flag.eq(&SigFlag::SigAll) {
                        sig_flag = SigFlag::SigAll;
                    }
                }
            }
        }

        // Since the proofs are unknown they need to be added to the database
        let proofs_info = proofs
            .clone()
            .into_iter()
            .map(|p| ProofInfo::new(p, self.mint_url.clone(), State::Pending, self.unit.clone()))
            .collect::<Result<Vec<ProofInfo>, _>>()?;
        self.localstore
            .update_proofs(proofs_info.clone(), vec![])
            .await?;

        let mut pre_swap = self
            .create_swap(None, amount_split_target, proofs, None, false)
            .await?;

        if sig_flag.eq(&SigFlag::SigAll) {
            for blinded_message in &mut pre_swap.swap_request.outputs {
                for signing_key in p2pk_signing_keys.values() {
                    blinded_message.sign_p2pk(signing_key.to_owned().clone())?
                }
            }
        }

        let swap_response = self.client.post_swap(pre_swap.swap_request).await?;

        // Proof to keep
        let recv_proofs = construct_proofs(
            swap_response.signatures,
            pre_swap.pre_mint_secrets.rs(),
            pre_swap.pre_mint_secrets.secrets(),
            &keys,
        )?;

        self.localstore
            .increment_keyset_counter(&active_keyset_id, recv_proofs.len() as u32)
            .await?;

        let total_amount = recv_proofs.total_amount()?;

        let recv_proof_infos = recv_proofs
            .into_iter()
            .map(|proof| ProofInfo::new(proof, mint_url.clone(), State::Unspent, self.unit.clone()))
            .collect::<Result<Vec<ProofInfo>, _>>()?;
        self.localstore
            .update_proofs(
                recv_proof_infos,
                proofs_info.into_iter().map(|p| p.y).collect(),
            )
            .await?;

        Ok(total_amount)
    }

    /// Receive
    /// # Synopsis
    /// ```rust, no_run
    ///  use std::sync::Arc;
    ///
    ///  use cdk::amount::SplitTarget;
    ///  use cdk::cdk_database::WalletMemoryDatabase;
    ///  use cdk::nuts::CurrencyUnit;
    ///  use cdk::wallet::Wallet;
    ///  use rand::Rng;
    ///
    /// #[tokio::main]
    /// async fn main() -> anyhow::Result<()> {
    ///  let seed = rand::thread_rng().gen::<[u8; 32]>();
    ///  let mint_url = "https://testnut.cashu.space";
    ///  let unit = CurrencyUnit::Sat;
    ///
    ///  let localstore = WalletMemoryDatabase::default();
    ///  let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), &seed, None).unwrap();
    ///  let token = "cashuAeyJ0b2tlbiI6W3sicHJvb2ZzIjpbeyJhbW91bnQiOjEsInNlY3JldCI6ImI0ZjVlNDAxMDJhMzhiYjg3NDNiOTkwMzU5MTU1MGYyZGEzZTQxNWEzMzU0OTUyN2M2MmM5ZDc5MGVmYjM3MDUiLCJDIjoiMDIzYmU1M2U4YzYwNTMwZWVhOWIzOTQzZmRhMWEyY2U3MWM3YjNmMGNmMGRjNmQ4NDZmYTc2NWFhZjc3OWZhODFkIiwiaWQiOiIwMDlhMWYyOTMyNTNlNDFlIn1dLCJtaW50IjoiaHR0cHM6Ly90ZXN0bnV0LmNhc2h1LnNwYWNlIn1dLCJ1bml0Ijoic2F0In0=";
    ///  let amount_receive = wallet.receive(token, SplitTarget::default(), &[], &[]).await?;
    ///  Ok(())
    /// }
    /// ```
    #[instrument(skip_all)]
    pub async fn receive(
        &self,
        encoded_token: &str,
        amount_split_target: SplitTarget,
        p2pk_signing_keys: &[SecretKey],
        preimages: &[String],
    ) -> Result<Amount, Error> {
        let token_data = Token::from_str(encoded_token)?;

        let unit = token_data.unit().unwrap_or_default();

        if unit != self.unit {
            return Err(Error::UnitUnsupported);
        }

        let proofs = token_data.proofs();
        if proofs.len() != 1 {
            return Err(Error::MultiMintTokenNotSupported);
        }

        if self.mint_url != token_data.mint_url()? {
            return Err(Error::IncorrectMint);
        }

        let amount = self
            .receive_proofs(proofs, amount_split_target, p2pk_signing_keys, preimages)
            .await?;

        Ok(amount)
    }
}
