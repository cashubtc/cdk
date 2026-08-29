use std::collections::hash_map::Entry;
use std::collections::HashMap;

use cdk_common::wallet::KeysetLoadPolicy;

use crate::dhke::construct_proofs;
use crate::nuts::{nut12, BlindSignature, BlindedMessage, Id, Keys, Proofs, SecretKey};
use crate::secret::Secret;
use crate::wallet::Wallet;
use crate::{Amount, Error};

/// How strictly returned signatures must match the outputs the wallet sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputSignaturePolicy {
    /// Amount and keyset must both match the requested output exactly.
    Exact,
    /// NUT-08/NUT-09 blank outputs: the mint fills in the amount, and may sign
    /// under a keyset the wallet did not request if the requested one rotated
    /// away before the change was issued.
    BlankOutput,
}

/// Validate mint-returned blind signatures against the wallet's requested outputs.
///
/// The mint controls the `amount` and `keyset_id` fields in each returned
/// [`BlindSignature`], so callers must verify those fields against the
/// corresponding premint blinded message before constructing wallet proofs.
///
/// Use [`OutputSignaturePolicy::Exact`] for mint/swap responses where the
/// wallet requested a specific denomination. Use
/// [`OutputSignaturePolicy::BlankOutput`] for NUT-08/NUT-09 style outputs where
/// the wallet sends amount `0` and the mint fills in the actual change or
/// restored amount. DLEQ proofs are optional for compatibility, but when
/// present they are verified after the signature metadata has been
/// cross-checked.
pub(crate) async fn validate_mint_response_signatures<'a>(
    wallet: &Wallet,
    signatures: &[BlindSignature],
    blinded_messages: impl IntoIterator<Item = &'a BlindedMessage>,
    policy: OutputSignaturePolicy,
) -> Result<(), Error> {
    let blinded_messages = blinded_messages.into_iter().collect::<Vec<_>>();

    if signatures.len() != blinded_messages.len() {
        return Err(Error::InvalidMintResponse(format!(
            "mint signatures ({}) does not match secrets sent ({})",
            signatures.len(),
            blinded_messages.len()
        )));
    }

    for (sig, blinded_message) in signatures.iter().zip(blinded_messages) {
        let amount_matches = match policy {
            OutputSignaturePolicy::Exact => sig.amount == blinded_message.amount,
            OutputSignaturePolicy::BlankOutput => {
                blinded_message.amount == Amount::ZERO || sig.amount == blinded_message.amount
            }
        };

        if !amount_matches {
            return Err(Error::InvalidMintResponse(format!(
                "mint signature amount ({}) does not match requested amount ({})",
                sig.amount, blinded_message.amount
            )));
        }

        let substituted = sig.keyset_id != blinded_message.keyset_id;
        if substituted && policy == OutputSignaturePolicy::Exact {
            return Err(Error::InvalidMintResponse(format!(
                "mint signature keyset ({}) does not match requested keyset ({})",
                sig.keyset_id, blinded_message.keyset_id
            )));
        }

        // Resolving the keyset already proves it belongs to this mint and unit:
        // `Wallet::keysets` only yields keysets matching the wallet's unit.
        let keyset = match wallet
            .keyset_or_refresh(sig.keyset_id, Default::default())
            .await
        {
            Ok(keyset) => keyset,
            Err(Error::UnknownKeySet) => {
                return Err(Error::InvalidMintResponse(format!(
                    "mint signed with keyset {}, which it does not publish for this unit",
                    sig.keyset_id
                )))
            }
            Err(err) => return Err(err),
        };
        let key = keyset.keys.amount_key(sig.amount).ok_or(Error::AmountKey)?;
        let dleq_verified = match sig.verify_dleq(key, blinded_message.blinded_secret) {
            Ok(_) => true,
            Err(nut12::Error::MissingDleqProof) => false,
            Err(_) => return Err(Error::CouldNotVerifyDleq),
        };

        // A substituted keyset is only acceptable when the mint is still
        // issuing under it, or when the DLEQ proves it signed this very output.
        // The DLEQ alternative covers a further rotation landing between the
        // mint signing the change and the wallet checking.
        if substituted && !keyset.active.unwrap_or(false) && !dleq_verified {
            return Err(Error::InvalidMintResponse(format!(
                "mint signature keyset ({}) does not match requested keyset ({}) and is neither active nor DLEQ-proven",
                sig.keyset_id, blinded_message.keyset_id
            )));
        }
    }

    Ok(())
}

/// Unblind mint-returned signatures, keying each one on the keyset that signed it.
///
/// [`construct_proofs`] takes a single key set, but a NUT-08 change batch can be
/// signed under a keyset the wallet did not request, and the wallet's own active
/// keyset may have moved on since the outputs were built. Unblinding with the
/// wrong `A` yields a structurally valid proof that only fails when spent, so
/// each signature is unblinded with its own keyset's key and the result is
/// checked against that key before it is handed back.
pub(crate) async fn construct_proofs_per_keyset(
    wallet: &Wallet,
    signatures: Vec<BlindSignature>,
    rs: Vec<SecretKey>,
    secrets: Vec<Secret>,
    policy: KeysetLoadPolicy,
) -> Result<Proofs, Error> {
    let mut keys_by_keyset: HashMap<Id, Keys> = HashMap::new();
    let mut proofs = Vec::with_capacity(signatures.len());

    for ((signature, r), secret) in signatures.into_iter().zip(rs).zip(secrets) {
        let keyset_id = signature.keyset_id;
        let keys = match keys_by_keyset.entry(keyset_id) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                entry.insert(wallet.keyset_or_refresh(keyset_id, policy).await?.keys)
            }
        };
        let key = keys.amount_key(signature.amount).ok_or(Error::AmountKey)?;

        let Some(proof) = construct_proofs(vec![signature], vec![r], vec![secret], keys)?.pop()
        else {
            return Err(Error::Custom("unblinding returned no proof".to_string()));
        };

        match proof.verify_dleq(key) {
            Ok(_) | Err(nut12::Error::MissingDleqProof) => (),
            Err(_) => return Err(Error::CouldNotVerifyDleq),
        }

        proofs.push(proof);
    }

    Ok(proofs)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cdk_common::wallet::KeysetLoadPolicy;

    use super::{validate_mint_response_signatures, OutputSignaturePolicy};
    use crate::dhke::sign_message;
    use crate::nuts::{BlindSignature, BlindedMessage, PreMintSecrets};
    use crate::wallet::test_utils::{
        create_test_db, create_test_wallet_with_mock, signing_keyset, test_keyset, test_keyset_id,
        MockMintConnector,
    };
    use crate::{Amount, Error};

    /// Builds a blank output plus a signature over it issued under `keyset`.
    fn blank_output_signed_by(
        keyset_id: crate::nuts::Id,
        signing_key: &crate::nuts::SecretKey,
        with_dleq: bool,
    ) -> (BlindedMessage, BlindSignature) {
        let premint = PreMintSecrets::blank(test_keyset_id(), Amount::from(8)).expect("blank");
        let blinded_message = premint.blinded_messages()[0].clone();
        let c = sign_message(signing_key, &blinded_message.blinded_secret).expect("signature");

        let signature = if with_dleq {
            BlindSignature::new(
                Amount::from(8),
                c,
                keyset_id,
                &blinded_message.blinded_secret,
                signing_key,
            )
            .expect("dleq")
        } else {
            BlindSignature {
                amount: Amount::from(8),
                keyset_id,
                c,
                dleq: None,
            }
        };

        (blinded_message, signature)
    }

    #[tokio::test]
    async fn keyset_or_refresh_finds_a_keyset_missing_from_the_cache() {
        let db = create_test_db().await;
        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.reset_default_mint_state();
        let wallet = create_test_wallet_with_mock(db, mock_client.clone()).await;

        // Populate the cache with the original keyset only, then rotate.
        wallet
            .keysets(KeysetLoadPolicy::CacheThenNetwork)
            .await
            .unwrap();
        let (new_keyset, _) = signing_keyset(true);
        mock_client.set_mint_keys_response(Ok(vec![test_keyset(), new_keyset.clone()]));

        assert!(matches!(
            wallet
                .keyset_with_policy(new_keyset.id, KeysetLoadPolicy::CacheOnly)
                .await,
            Err(Error::UnknownKeySet)
        ));
        assert_eq!(
            wallet
                .keyset_or_refresh(new_keyset.id, Default::default())
                .await
                .expect("refresh must find the rotated keyset")
                .id,
            new_keyset.id
        );
    }

    #[tokio::test]
    async fn keyset_or_refresh_honors_cache_only() {
        let db = create_test_db().await;
        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.reset_default_mint_state();
        let wallet = create_test_wallet_with_mock(db, mock_client.clone()).await;

        wallet
            .keysets(KeysetLoadPolicy::CacheThenNetwork)
            .await
            .unwrap();
        let (new_keyset, _) = signing_keyset(true);
        mock_client.set_mint_keys_response(Ok(vec![test_keyset(), new_keyset.clone()]));

        assert!(matches!(
            wallet
                .keyset_or_refresh(new_keyset.id, KeysetLoadPolicy::CacheOnly)
                .await,
            Err(Error::UnknownKeySet)
        ));
    }

    #[tokio::test]
    async fn exact_policy_rejects_a_substituted_keyset() {
        let db = create_test_db().await;
        let mock_client = Arc::new(MockMintConnector::new());
        let (new_keyset, signing_keys) = signing_keyset(true);
        mock_client.set_mint_keys_response(Ok(vec![test_keyset(), new_keyset.clone()]));
        let wallet = create_test_wallet_with_mock(db, mock_client).await;

        let (blinded_message, signature) = blank_output_signed_by(
            new_keyset.id,
            signing_keys.get(&Amount::from(8)).unwrap(),
            false,
        );

        assert!(matches!(
            validate_mint_response_signatures(
                &wallet,
                &[signature],
                [&blinded_message],
                OutputSignaturePolicy::Exact,
            )
            .await,
            Err(Error::InvalidMintResponse(_))
        ));
    }

    #[tokio::test]
    async fn blank_output_policy_rejects_an_inactive_keyset_without_dleq() {
        let db = create_test_db().await;
        let mock_client = Arc::new(MockMintConnector::new());
        let (retired_keyset, signing_keys) = signing_keyset(false);
        mock_client.set_mint_keys_response(Ok(vec![test_keyset(), retired_keyset.clone()]));
        let wallet = create_test_wallet_with_mock(db, mock_client).await;

        let (blinded_message, signature) = blank_output_signed_by(
            retired_keyset.id,
            signing_keys.get(&Amount::from(8)).unwrap(),
            false,
        );

        assert!(matches!(
            validate_mint_response_signatures(
                &wallet,
                &[signature],
                [&blinded_message],
                OutputSignaturePolicy::BlankOutput,
            )
            .await,
            Err(Error::InvalidMintResponse(_))
        ));
    }

    /// A further rotation can retire the substituted keyset before the wallet
    /// looks it up, so a DLEQ proof stands in for the active flag.
    #[tokio::test]
    async fn blank_output_policy_accepts_an_inactive_keyset_proven_by_dleq() {
        let db = create_test_db().await;
        let mock_client = Arc::new(MockMintConnector::new());
        let (retired_keyset, signing_keys) = signing_keyset(false);
        mock_client.set_mint_keys_response(Ok(vec![test_keyset(), retired_keyset.clone()]));
        let wallet = create_test_wallet_with_mock(db, mock_client).await;

        let (blinded_message, signature) = blank_output_signed_by(
            retired_keyset.id,
            signing_keys.get(&Amount::from(8)).unwrap(),
            true,
        );

        validate_mint_response_signatures(
            &wallet,
            &[signature],
            [&blinded_message],
            OutputSignaturePolicy::BlankOutput,
        )
        .await
        .expect("a DLEQ-proven signature must be accepted");
    }
}
