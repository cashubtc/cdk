//! Receive module for the wallet.
//!
//! This module provides functionality for receiving ecash tokens and proofs.

use std::str::FromStr;

use tracing::instrument;

use crate::nuts::{Proofs, Token};
use crate::{ensure_cdk, Amount, Error, Wallet};

pub(crate) mod saga;

pub use cdk_common::wallet::ReceiveOptions;
use saga::ReceiveSaga;

impl Wallet {
    /// Receive proofs using the saga pattern
    ///
    /// This is the internal implementation that uses the saga pattern
    /// for proper crash recovery and compensation.
    #[instrument(skip_all)]
    pub async fn receive_proofs(
        &self,
        proofs: Proofs,
        opts: ReceiveOptions,
        memo: Option<String>,
        token: Option<String>,
    ) -> Result<Amount, Error> {
        let saga = ReceiveSaga::new(self);
        let saga = saga.prepare(proofs, opts, memo, token).await?;
        let saga = saga.execute().await?;
        Ok(saga.into_amount())
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
            .receive_proofs(
                proofs,
                opts,
                token.memo().clone(),
                Some(encoded_token.to_string()),
            )
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

    /// Receive an encoded token offline without contacting the mint
    #[instrument(skip_all)]
    pub async fn receive_offline(
        &self,
        encoded_token: &str,
        opts: cdk_common::wallet::OfflineReceiveOptions,
    ) -> Result<Amount, Error> {
        let token = Token::from_str(encoded_token)?;

        let unit = token.unit().unwrap_or_default();
        ensure_cdk!(unit == self.unit, Error::UnsupportedUnit);

        let mint_url = token.mint_url()?;
        ensure_cdk!(self.mint_url == mint_url, Error::IncorrectMint);

        if !opts.trusted_mints.is_empty() {
            ensure_cdk!(opts.trusted_mints.contains(&mint_url), Error::IncorrectMint);
        }

        if let Token::TokenV3(token) = &token {
            ensure_cdk!(!token.is_multi_mint(), Error::MultiMintTokenNotSupported);
        }

        let keysets_info = self.load_mint_keysets().await?;
        use cdk_common::ProofsMethods;
        let mut proofs = token.proofs(&keysets_info)?;
        let proofs_ys = proofs.ys()?;

        let mut total_amount = Amount::ZERO;

        for proof in &mut proofs {
            if opts.require_dleq {
                ensure_cdk!(proof.dleq.is_some(), Error::DleqProofNotProvided);
            }

            if proof.dleq.is_some() {
                let keys = self.load_keyset_keys(proof.keyset_id).await?;
                let key = keys.amount_key(proof.amount).ok_or(Error::AmountKey)?;
                proof.verify_dleq(key)?;
            }

            if opts.require_locked {
                use crate::nuts::nut10::Kind;
                let secret_res: Result<crate::nuts::nut10::Secret, _> = proof.secret.clone().try_into();
                if let Ok(secret) = secret_res {
                    let is_p2pk = match secret.kind() {
                        Kind::P2PK => true,
                        _ => false,
                    };
                    ensure_cdk!(
                        is_p2pk,
                        Error::InvalidSpendConditions("Token must be P2PK locked".to_string())
                    );
                } else {
                    return Err(Error::InvalidSpendConditions(
                        "Token must be P2PK locked".to_string(),
                    ));
                }
            }

            if let Some(min_locktime) = opts.minimum_locktime {
                let secret_res: Result<crate::nuts::nut10::Secret, _> = proof.secret.clone().try_into();
                if let Ok(secret) = secret_res {
                    let conditions: Result<crate::nuts::Conditions, _> = secret
                        .secret_data()
                        .tags()
                        .cloned()
                        .unwrap_or_default()
                        .try_into();
                    if let Ok(conditions) = conditions {
                        if let Some(locktime) = conditions.locktime {
                            ensure_cdk!(
                                locktime >= min_locktime,
                                Error::InvalidSpendConditions(format!(
                                    "Locktime {} is less than required {}",
                                    locktime, min_locktime
                                ))
                            );
                        } else {
                            return Err(Error::LocktimeNotProvided);
                        }
                    } else {
                        return Err(Error::LocktimeNotProvided);
                    }
                } else {
                    return Err(Error::LocktimeNotProvided);
                }
            }

            total_amount += proof.amount;
        }

        use crate::nuts::State;
        use crate::wallet::ProofInfo;
        use cdk_common::util::unix_time;
        use cdk_common::wallet::{Transaction, TransactionDirection};

        let proofs_info = proofs
            .clone()
            .into_iter()
            .map(|p| {
                ProofInfo::new(
                    p,
                    self.mint_url.clone(),
                    State::PendingReceive,
                    self.unit.clone(),
                )
            })
            .collect::<Result<Vec<ProofInfo>, _>>()?;

        self.localstore.update_proofs(proofs_info, vec![]).await?;

        let memo = token.memo().clone();

        self.localstore
            .add_transaction(Transaction {
                mint_url: self.mint_url.clone(),
                direction: TransactionDirection::Incoming,
                amount: total_amount,
                fee: Amount::ZERO,
                unit: self.unit.clone(),
                ys: proofs_ys,
                timestamp: unix_time(),
                memo,
                metadata: std::collections::HashMap::new(),
                quote_id: None,
                payment_request: None,
                payment_proof: None,
                payment_method: None,
                saga_id: None,
            })
            .await?;

        Ok(total_amount)
    }

    /// Finalize pending offline receives by attempting to swap them
    #[instrument(skip_all)]
    pub async fn finalize_pending_receives(&self) -> Result<Amount, Error> {
        use crate::nuts::State;

        let proofs_info = self
            .localstore
            .get_proofs(
                Some(self.mint_url.clone()),
                Some(self.unit.clone()),
                Some(vec![State::PendingReceive]),
                None,
            )
            .await?;

        if proofs_info.is_empty() {
            return Ok(Amount::ZERO);
        }

        let proofs: Proofs = proofs_info.into_iter().map(|p| p.proof).collect();

        self.receive_proofs(proofs, ReceiveOptions::default(), None, None)
            .await
    }
}
