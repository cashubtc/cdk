//! Nutroot transaction signing and transfer-key handling.
use bitcoin::hashes::{sha256, Hash};
use bitcoin::secp256k1::{PublicKey as SecpPublicKey, SecretKey as SecpSecretKey};
use cdk_common::nuts::nut10::nutroot::{self, Condition, KeyPurpose, Transaction};
use cdk_common::nuts::{KeySetVersion, Proofs, ProofsMethods, SecretKey, SwapRequest, Witness};

use super::Wallet;
use crate::Error;

fn error(error: nutroot::Error) -> Error {
    cdk_common::nuts::nut10::Error::from(error).into()
}

impl Wallet {
    /// Include the transfer scalar for a seed-derived bare proof when sending it.
    pub(crate) async fn attach_nutroot_transfer_keys(
        &self,
        proofs: &mut Proofs,
    ) -> Result<(), Error> {
        let stored = self.localstore.get_proofs_by_ys(proofs.ys()?).await?;
        for proof in proofs {
            if proof.keyset_id.get_version() != KeySetVersion::Version02 {
                continue;
            }
            if let Some(info) = &proof.spend_info {
                // A locked output retains its original transfer policy.
                if info.ephemeral_key.is_none() {
                    info.verify(&proof.secret.to_string(), None)
                        .map_err(error)?;
                }
                continue;
            }
            let counter = stored
                .iter()
                .find(|info| {
                    info.proof.secret == proof.secret && info.proof.keyset_id == proof.keyset_id
                })
                .and_then(|info| info.derivation_index)
                .ok_or(Error::NUT10(
                    cdk_common::nuts::nut10::Error::SpendConditionsNotMet,
                ))?;
            let key = nutroot::derive_key(
                &self.seed,
                proof.keyset_id,
                counter.into(),
                KeyPurpose::Internal,
            )
            .map_err(error)?;
            let info = nutroot::SpendInfo {
                bearer_key: Some(SecretKey::from_slice(&key.secret_bytes())?),
                ..Default::default()
            };
            info.verify(&proof.secret.to_string(), None)
                .map_err(error)?;
            proof.spend_info = Some(info);
        }
        Ok(())
    }
    pub(crate) async fn sign_nutroot_swap(
        &self,
        request: &mut SwapRequest,
        keys: &[SecretKey],
        preimages: &[String],
    ) -> Result<(), Error> {
        if !request
            .inputs()
            .iter()
            .any(|p| p.keyset_id.get_version() == KeySetVersion::Version02)
        {
            return Ok(());
        }
        let transaction =
            Transaction::new(request.inputs(), &[], request.outputs(), &[]).map_err(error)?;
        self.sign_nutroot_inputs(request.inputs_mut(), &transaction, keys, preimages)
            .await
    }

    pub(crate) async fn sign_nutroot_inputs(
        &self,
        inputs: &mut Proofs,
        transaction: &Transaction,
        explicit_keys: &[SecretKey],
        preimages: &[String],
    ) -> Result<(), Error> {
        let mut known_keys = explicit_keys
            .iter()
            .map(|key| key.as_secp256k1().copied())
            .collect::<Result<Vec<_>, _>>()?;
        for stored in self.localstore.list_p2pk_keys().await? {
            if let Some(key) = self.get_signing_key(&stored.pubkey).await? {
                known_keys.push(*key.as_secp256k1()?);
            }
        }
        let stored = self.localstore.get_proofs_by_ys(inputs.ys()?).await?;
        let now = cdk_common::util::unix_time();
        let mut updates = vec![];
        for (index, proof) in inputs.iter_mut().enumerate() {
            if proof.keyset_id.get_version() != KeySetVersion::Version02 {
                continue;
            }
            let digest = transaction.input_digest(index).map_err(error)?;
            let secret = proof.secret.to_string();
            let public = nutroot::parse_secret(&secret).map_err(error)?;
            let stored = stored.iter().find(|info| {
                info.proof.keyset_id == proof.keyset_id && info.proof.secret == proof.secret
            });
            if proof.spend_info.is_none() {
                proof.spend_info = stored.and_then(|info| info.proof.spend_info.clone());
            }
            let mut keys = known_keys.clone();
            if let Some(counter) = stored.and_then(|info| info.derivation_index) {
                keys.push(
                    nutroot::derive_key(
                        &self.seed,
                        proof.keyset_id,
                        counter.into(),
                        KeyPurpose::Internal,
                    )
                    .map_err(error)?,
                );
                if let Some(tree) = proof
                    .spend_info
                    .as_ref()
                    .and_then(|info| info.parsed_tree().ok().flatten())
                {
                    for i in 0..tree
                        .leaves()
                        .iter()
                        .map(|leaf| leaf.keys().len())
                        .sum::<usize>()
                    {
                        keys.push(
                            nutroot::derive_key(
                                &self.seed,
                                proof.keyset_id,
                                counter.into(),
                                KeyPurpose::Leaf(i as u32),
                            )
                            .map_err(error)?,
                        );
                    }
                }
            }
            let mut signed = None;
            if let Some(info) = &proof.spend_info {
                let candidates: Vec<Option<&SecpSecretKey>> =
                    std::iter::once(None).chain(keys.iter().map(Some)).collect();
                // Reconstruction is mandatory even when an old witness is present.
                let internal = candidates
                    .iter()
                    .find_map(|key| info.verify(&secret, *key).ok())
                    .ok_or(Error::NUT10(
                        cdk_common::nuts::nut10::Error::SpendConditionsNotMet,
                    ))?;
                for candidate in &candidates {
                    if let Ok(key) = info.key_path_key(&secret, *candidate) {
                        signed = Some(nutroot::Witness::key_path(&key, digest));
                        break;
                    }
                }
                if signed.is_none() {
                    if let Some(tree) = info.parsed_tree().map_err(error)? {
                        let slots = tree
                            .leaves()
                            .iter()
                            .map(|leaf| leaf.keys().len())
                            .sum::<usize>();
                        let mut leaf_keys = keys.clone();
                        if let Some(ephemeral) = info.ephemeral_key {
                            for key in &keys {
                                for slot in 1..=slots {
                                    if let Ok(derived) =
                                        nutroot::receiver_key(key, &ephemeral, slot as u8)
                                    {
                                        leaf_keys.push(derived);
                                    }
                                }
                            }
                        }
                        for (leaf_index, leaf) in tree.leaves().iter().enumerate() {
                            if matches!(leaf.condition(), Condition::Commit(_)) {
                                continue;
                            }
                            let mut witness =
                                nutroot::Witness::script_path(&tree, leaf_index, internal)
                                    .map_err(error)?;
                            if let Condition::Hashlock(hash) = leaf.condition() {
                                witness.preimage = preimages
                                    .iter()
                                    .find(|value| {
                                        cdk_common::util::hex::decode(value).ok().is_some_and(
                                            |bytes| {
                                                bytes.len() <= 32
                                                    && sha256::Hash::hash(&bytes).to_byte_array()
                                                        == *hash
                                            },
                                        )
                                    })
                                    .cloned();
                            }
                            for public in leaf.keys() {
                                if let Some(key) = leaf_keys.iter().find(|key| {
                                    SecpPublicKey::from_secret_key(&cdk_common::SECP256K1, key)
                                        .x_only_public_key()
                                        .0
                                        == public.x_only_public_key().0
                                }) {
                                    witness
                                        .signatures
                                        .extend(nutroot::Witness::key_path(key, digest).signatures);
                                }
                            }
                            if witness.verify(&secret, digest, now).is_ok() {
                                signed = Some(witness);
                                break;
                            }
                        }
                    }
                }
            } else if let Some(key) = keys
                .iter()
                .find(|key| SecpPublicKey::from_secret_key(&cdk_common::SECP256K1, key) == public)
            {
                signed = Some(nutroot::Witness::key_path(key, digest));
            }
            // A persisted request may already carry the signature needed to replay it.
            if signed.is_none() {
                if let Some(Witness::NutrootWitness(raw)) = &proof.witness {
                    if let Ok(witness) = serde_json::from_str::<nutroot::Witness>(raw) {
                        if witness.verify(&secret, digest, now).is_ok() {
                            signed = Some(witness);
                        }
                    }
                }
            }
            let witness = signed.ok_or(Error::NUT10(
                cdk_common::nuts::nut10::Error::SpendConditionsNotMet,
            ))?;
            witness.verify(&secret, digest, now).map_err(error)?;
            let existing = proof.witness.as_ref().and_then(|value| match value {
                Witness::NutrootWitness(raw) => serde_json::from_str::<nutroot::Witness>(raw)
                    .ok()
                    .filter(|w| w.verify(&secret, digest, now).is_ok())
                    .map(|_| raw.clone()),
                _ => None,
            });
            proof.witness = Some(Witness::NutrootWitness(match existing {
                Some(raw) => raw,
                None => serde_json::to_string(&witness)?,
            }));
            if let Some(stored) = stored {
                let mut update = stored.clone();
                update.proof.witness = proof.witness.clone();
                updates.push(update);
            }
            // Transfer keys and unexercised tree leaves are wallet data, not mint inputs.
            proof.spend_info = None;
        }
        if !updates.is_empty() {
            self.localstore.update_proofs(updates, vec![]).await?;
        }
        Ok(())
    }
}

pub(crate) fn is_nutroot_outputs(outputs: &[crate::nuts::BlindedMessage]) -> bool {
    outputs
        .iter()
        .any(|output| output.keyset_id.get_version() == KeySetVersion::Version02)
}

pub(crate) fn sign_quote(
    outputs: &[crate::nuts::BlindedMessage],
    quotes: &[cdk_common::wallet::MintQuote],
    id: &str,
    key: &SecretKey,
) -> Result<String, Error> {
    let keyset = outputs
        .first()
        .ok_or(Error::SignatureMissingOrInvalid)?
        .keyset_id;
    if outputs.iter().any(|output| output.keyset_id != keyset) {
        return Err(Error::SignatureMissingOrInvalid);
    }
    let index = quotes
        .iter()
        .position(|quote| quote.id == id)
        .ok_or(Error::UnknownQuote)?;
    let quotes: Vec<_> = quotes
        .iter()
        .map(|quote| nutroot::Quote {
            id: quote.id.clone(),
            amount: quote.amount.unwrap_or(crate::Amount::ZERO),
        })
        .collect();
    let transaction = Transaction::new(&[], &quotes, outputs, &[]).map_err(error)?;
    nutroot::Witness::key_path(
        key.as_secp256k1()?,
        transaction.input_digest(index).map_err(error)?,
    )
    .signatures
    .into_iter()
    .next()
    .ok_or(Error::SignatureMissingOrInvalid)
}

pub(crate) fn sign_mint_request(
    request: &mut crate::nuts::MintRequest<String>,
    quote: &cdk_common::wallet::MintQuote,
    key: &SecretKey,
) -> Result<(), Error> {
    if is_nutroot_outputs(&request.outputs) {
        request.signature = Some(sign_quote(
            &request.outputs,
            std::slice::from_ref(quote),
            &quote.id,
            key,
        )?);
    } else {
        request.sign(key)?;
    }
    Ok(())
}
