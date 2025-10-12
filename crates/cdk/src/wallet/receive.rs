use std::collections::HashMap;
use std::str::FromStr;

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::XOnlyPublicKey;
use cdk_common::util::unix_time;
use cdk_common::wallet::{Transaction, TransactionDirection};
use tracing::instrument;

use crate::amount::SplitTarget;
use crate::dhke::construct_proofs;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::nut10::Kind;
use crate::nuts::{Conditions, Proofs, PublicKey, SecretKey, SigFlag, State, Token};
use crate::types::ProofInfo;
use crate::util::hex;
use crate::{ensure_cdk, Amount, Error, Wallet, SECP256K1};

impl Wallet {
    /// Receive proofs
    #[instrument(skip_all)]
    pub async fn receive_proofs(
        &self,
        proofs: Proofs,
        opts: ReceiveOptions,
        memo: Option<String>,
    ) -> Result<Amount, Error> {
        let mint_url = &self.mint_url;

        self.refresh_keysets().await?;

        let active_keyset_id = self.fetch_active_keyset().await?.id;

        let keys = self.load_keyset_keys(active_keyset_id).await?;

        let mut proofs = proofs;

        let proofs_amount = proofs.total_amount()?;
        let proofs_ys = proofs.ys()?;

        let mut sig_flag = SigFlag::SigInputs;

        // Map hash of preimage to preimage
        let hashed_to_preimage: HashMap<String, &String> = opts
            .preimages
            .iter()
            .map(|p| {
                let hex_bytes = hex::decode(p)?;
                Ok::<(String, &String), Error>((Sha256Hash::hash(&hex_bytes).to_string(), p))
            })
            .collect::<Result<HashMap<String, &String>, _>>()?;

        let p2pk_signing_keys: HashMap<XOnlyPublicKey, &SecretKey> = opts
            .p2pk_signing_keys
            .iter()
            .map(|s| (s.x_only_public_key(&SECP256K1).0, s))
            .collect();

        for proof in &mut proofs {
            // Verify that proof DLEQ is valid
            if proof.dleq.is_some() {
                let keys = self.load_keyset_keys(proof.keyset_id).await?;
                let key = keys.amount_key(proof.amount).ok_or(Error::AmountKey)?;
                proof.verify_dleq(key)?;
            }

            if let Ok(secret) =
                <crate::secret::Secret as TryInto<crate::nuts::nut10::Secret>>::try_into(
                    proof.secret.clone(),
                )
            {
                let conditions: Result<Conditions, _> = secret
                    .secret_data()
                    .tags()
                    .cloned()
                    .unwrap_or_default()
                    .try_into();
                if let Ok(conditions) = conditions {
                    let mut pubkeys = conditions.pubkeys.unwrap_or_default();

                    match secret.kind() {
                        Kind::P2PK => {
                            let data_key = PublicKey::from_str(secret.secret_data().data())?;

                            pubkeys.push(data_key);
                        }
                        Kind::HTLC => {
                            let hashed_preimage = secret.secret_data().data();
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
            .create_swap(None, opts.amount_split_target, proofs, None, false)
            .await?;

        if sig_flag.eq(&SigFlag::SigAll) {
            for blinded_message in pre_swap.swap_request.outputs_mut() {
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

        // Add transaction to store
        self.localstore
            .add_transaction(Transaction {
                mint_url: self.mint_url.clone(),
                direction: TransactionDirection::Incoming,
                amount: total_amount,
                fee: proofs_amount - total_amount,
                unit: self.unit.clone(),
                ys: proofs_ys,
                timestamp: unix_time(),
                memo,
                metadata: opts.metadata,
                quote_id: None,
                payment_request: None,
                payment_proof: None,
            })
            .await?;

        Ok(total_amount)
    }

    /// Receive
    /// # Synopsis
    /// ```rust, no_run
    ///  use std::sync::Arc;
    ///
    ///  use cdk::amount::SplitTarget;
    ///  use cdk_sqlite::wallet::memory;
    ///  use cdk::nuts::CurrencyUnit;
    ///  use cdk::wallet::{ReceiveOptions, Wallet};
    ///  use rand::random;
    ///
    /// #[tokio::main]
    /// async fn main() -> anyhow::Result<()> {
    ///  let seed = random::<[u8; 64]>();
    ///  let mint_url = "https://fake.thesimplekid.dev";
    ///  let unit = CurrencyUnit::Sat;
    ///
    ///  let localstore = memory::empty().await?;
    ///  let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), seed, None).unwrap();
    ///  let token = "cashuAeyJ0b2tlbiI6W3sicHJvb2ZzIjpbeyJhbW91bnQiOjEsInNlY3JldCI6ImI0ZjVlNDAxMDJhMzhiYjg3NDNiOTkwMzU5MTU1MGYyZGEzZTQxNWEzMzU0OTUyN2M2MmM5ZDc5MGVmYjM3MDUiLCJDIjoiMDIzYmU1M2U4YzYwNTMwZWVhOWIzOTQzZmRhMWEyY2U3MWM3YjNmMGNmMGRjNmQ4NDZmYTc2NWFhZjc3OWZhODFkIiwiaWQiOiIwMDlhMWYyOTMyNTNlNDFlIn1dLCJtaW50IjoiaHR0cHM6Ly90ZXN0bnV0LmNhc2h1LnNwYWNlIn1dLCJ1bml0Ijoic2F0In0=";
    ///  let amount_receive = wallet.receive(token, ReceiveOptions::default()).await?;
    ///  Ok(())
    /// }
    /// ```
    #[instrument(skip_all)]
    pub async fn receive(
        &self,
        encoded_token: &str,
        opts: ReceiveOptions,
    ) -> Result<Amount, Error> {
        let token = Token::from_str(encoded_token)?;

        let unit = token.unit().unwrap_or_default();

        ensure_cdk!(unit == self.unit, Error::UnsupportedUnit);

        let keysets_info = self.load_mint_keysets().await?;
        let proofs = token.proofs(&keysets_info)?;

        if let Token::TokenV3(token) = &token {
            ensure_cdk!(!token.is_multi_mint(), Error::MultiMintTokenNotSupported);
        }

        ensure_cdk!(self.mint_url == token.mint_url()?, Error::IncorrectMint);

        let amount = self
            .receive_proofs(proofs, opts, token.memo().clone())
            .await?;

        Ok(amount)
    }

    /// Receive
    /// # Synopsis
    /// ```rust, no_run
    ///  use std::sync::Arc;
    ///
    ///  use cdk::amount::SplitTarget;
    ///  use cdk_sqlite::wallet::memory;
    ///  use cdk::nuts::CurrencyUnit;
    ///  use cdk::wallet::{ReceiveOptions, Wallet};
    ///  use cdk::util::hex;
    ///  use rand::random;
    ///
    /// #[tokio::main]
    /// async fn main() -> anyhow::Result<()> {
    ///  let seed = random::<[u8; 64]>();
    ///  let mint_url = "https://fake.thesimplekid.dev";
    ///  let unit = CurrencyUnit::Sat;
    ///
    ///  let localstore = memory::empty().await?;
    ///  let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), seed, None).unwrap();
    ///  let token_raw = hex::decode("6372617742a4617481a261694800ad268c4d1f5826617081a3616101617378403961366462623834376264323332626137366462306466313937323136623239643362386363313435353363643237383237666331636339343266656462346561635821038618543ffb6b8695df4ad4babcde92a34a96bdcd97dcee0d7ccf98d4721267926164695468616e6b20796f75616d75687474703a2f2f6c6f63616c686f73743a33333338617563736174").unwrap();
    ///  let amount_receive = wallet.receive_raw(&token_raw, ReceiveOptions::default()).await?;
    ///  Ok(())
    /// }
    /// ```
    #[instrument(skip_all)]
    pub async fn receive_raw(
        &self,
        binary_token: &Vec<u8>,
        opts: ReceiveOptions,
    ) -> Result<Amount, Error> {
        let token_str = Token::try_from(binary_token)?.to_string();
        self.receive(token_str.as_str(), opts).await
    }
}

/// Receive options
#[derive(Debug, Clone, Default)]
pub struct ReceiveOptions {
    /// Amount split target
    pub amount_split_target: SplitTarget,
    /// P2PK signing keys
    pub p2pk_signing_keys: Vec<SecretKey>,
    /// Preimages
    pub preimages: Vec<String>,
    /// Metadata
    pub metadata: HashMap<String, String>,
}
