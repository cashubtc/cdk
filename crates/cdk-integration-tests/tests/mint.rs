//! Mint tests

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use bip39::Mnemonic;
use cdk::amount::{Amount, SplitTarget};
use cdk::cdk_database::mint_memory::MintMemoryDatabase;
use cdk::dhke::construct_proofs;
use cdk::mint::MintQuote;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::nut17::Params;
use cdk::nuts::{
    CurrencyUnit, Id, MintBolt11Request, MintInfo, NotificationPayload, Nuts, PreMintSecrets,
    ProofState, Proofs, SecretKey, SpendingConditions, State, SwapRequest,
};
use cdk::types::QuoteTTL;
use cdk::util::unix_time;
use cdk::Mint;
use tokio::sync::OnceCell;
use tokio::time::sleep;

pub const MINT_URL: &str = "http://127.0.0.1:8088";

static INSTANCE: OnceCell<Mint> = OnceCell::const_new();

async fn new_mint(fee: u64) -> Mint {
    let mut supported_units = HashMap::new();
    supported_units.insert(CurrencyUnit::Sat, (fee, 32));

    let nuts = Nuts::new()
        .nut07(true)
        .nut08(true)
        .nut09(true)
        .nut10(true)
        .nut11(true)
        .nut12(true)
        .nut14(true);

    let mint_info = MintInfo::new().nuts(nuts);

    let mnemonic = Mnemonic::generate(12).unwrap();

    let quote_ttl = QuoteTTL::new(10000, 10000);

    Mint::new(
        MINT_URL,
        &mnemonic.to_seed_normalized(""),
        mint_info,
        quote_ttl,
        Arc::new(MintMemoryDatabase::default()),
        HashMap::new(),
        supported_units,
        HashMap::new(),
    )
    .await
    .unwrap()
}

async fn initialize() -> &'static Mint {
    INSTANCE.get_or_init(|| new_mint(0)).await
}

async fn mint_proofs(
    mint: &Mint,
    amount: Amount,
    split_target: &SplitTarget,
    keys: cdk::nuts::Keys,
) -> Result<Proofs> {
    let request_lookup = uuid::Uuid::new_v4().to_string();

    let quote = MintQuote::new(
        mint.mint_url.clone(),
        "".to_string(),
        CurrencyUnit::Sat,
        amount,
        unix_time() + 36000,
        request_lookup.to_string(),
    );

    mint.localstore.add_mint_quote(quote.clone()).await?;

    mint.pay_mint_quote_for_request_id(&request_lookup).await?;
    let keyset_id = Id::from(&keys);

    let premint = PreMintSecrets::random(keyset_id, amount, split_target)?;

    let mint_request = MintBolt11Request {
        quote: quote.id,
        outputs: premint.blinded_messages(),
    };

    let after_mint = mint.process_mint_request(mint_request).await?;

    let proofs = construct_proofs(
        after_mint.signatures,
        premint.rs(),
        premint.secrets(),
        &keys,
    )?;

    Ok(proofs)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_mint_double_spend() -> Result<()> {
    let mint = initialize().await;

    let keys = mint.pubkeys().await?.keysets.first().unwrap().clone().keys;
    let keyset_id = Id::from(&keys);

    let proofs = mint_proofs(mint, 100.into(), &SplitTarget::default(), keys).await?;

    let preswap = PreMintSecrets::random(keyset_id, 100.into(), &SplitTarget::default())?;

    let swap_request = SwapRequest::new(proofs.clone(), preswap.blinded_messages());

    let swap = mint.process_swap_request(swap_request).await;

    assert!(swap.is_ok());

    let preswap_two = PreMintSecrets::random(keyset_id, 100.into(), &SplitTarget::default())?;

    let swap_two_request = SwapRequest::new(proofs, preswap_two.blinded_messages());

    match mint.process_swap_request(swap_two_request).await {
        Ok(_) => bail!("Proofs double spent"),
        Err(err) => match err {
            cdk::Error::TokenAlreadySpent => (),
            _ => bail!("Wrong error returned"),
        },
    }

    Ok(())
}

/// This attempts to swap for more outputs then inputs.
/// This will work if the mint does not check for outputs amounts overflowing
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_attempt_to_swap_by_overflowing() -> Result<()> {
    let mint = initialize().await;

    let keys = mint.pubkeys().await?.keysets.first().unwrap().clone().keys;
    let keyset_id = Id::from(&keys);

    let proofs = mint_proofs(mint, 100.into(), &SplitTarget::default(), keys).await?;

    let amount = 2_u64.pow(63);

    let pre_mint_amount =
        PreMintSecrets::random(keyset_id, amount.into(), &SplitTarget::default())?;
    let pre_mint_amount_two =
        PreMintSecrets::random(keyset_id, amount.into(), &SplitTarget::default())?;

    let mut pre_mint = PreMintSecrets::random(keyset_id, 1.into(), &SplitTarget::default())?;

    pre_mint.combine(pre_mint_amount);
    pre_mint.combine(pre_mint_amount_two);

    let swap_request = SwapRequest::new(proofs.clone(), pre_mint.blinded_messages());

    match mint.process_swap_request(swap_request).await {
        Ok(_) => bail!("Swap occurred with overflow"),
        Err(err) => match err {
            cdk::Error::NUT03(cdk::nuts::nut03::Error::Amount(_)) => (),
            _ => {
                println!("{:?}", err);
                bail!("Wrong error returned in swap overflow")
            }
        },
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
pub async fn test_p2pk_swap() -> Result<()> {
    let mint = initialize().await;

    let keys = mint.pubkeys().await?.keysets.first().unwrap().clone().keys;
    let keyset_id = Id::from(&keys);

    let proofs = mint_proofs(mint, 100.into(), &SplitTarget::default(), keys).await?;

    let secret = SecretKey::generate();

    let spending_conditions = SpendingConditions::new_p2pk(secret.public_key(), None);

    let pre_swap = PreMintSecrets::with_conditions(
        keyset_id,
        100.into(),
        &SplitTarget::default(),
        &spending_conditions,
    )?;

    let swap_request = SwapRequest::new(proofs.clone(), pre_swap.blinded_messages());

    let keys = mint.pubkeys().await?.keysets.first().cloned().unwrap().keys;

    let post_swap = mint.process_swap_request(swap_request).await?;

    let mut proofs = construct_proofs(
        post_swap.signatures,
        pre_swap.rs(),
        pre_swap.secrets(),
        &keys,
    )?;

    let pre_swap = PreMintSecrets::random(keyset_id, 100.into(), &SplitTarget::default())?;

    let swap_request = SwapRequest::new(proofs.clone(), pre_swap.blinded_messages());

    let public_keys_to_listen: Vec<_> = swap_request
        .inputs
        .ys()
        .expect("key")
        .into_iter()
        .enumerate()
        .filter_map(|(key, pk)| {
            if key % 2 == 0 {
                // Only expect messages from every other key
                Some(pk.to_string())
            } else {
                None
            }
        })
        .collect();

    let mut listener = mint
        .pubsub_manager
        .try_subscribe(Params {
            kind: cdk::nuts::nut17::Kind::ProofState,
            filters: public_keys_to_listen.clone(),
            id: "test".into(),
        })
        .await
        .expect("valid subscription");

    match mint.process_swap_request(swap_request).await {
        Ok(_) => bail!("Proofs spent without sig"),
        Err(err) => match err {
            cdk::Error::NUT11(cdk::nuts::nut11::Error::SignaturesNotProvided) => (),
            _ => {
                println!("{:?}", err);
                bail!("Wrong error returned")
            }
        },
    }

    for proof in &mut proofs {
        proof.sign_p2pk(secret.clone())?;
    }

    let swap_request = SwapRequest::new(proofs.clone(), pre_swap.blinded_messages());

    let attempt_swap = mint.process_swap_request(swap_request).await;

    assert!(attempt_swap.is_ok());

    sleep(Duration::from_millis(10)).await;

    let mut msgs = HashMap::new();
    while let Ok((sub_id, msg)) = listener.try_recv() {
        assert_eq!(sub_id, "test".into());
        match msg {
            NotificationPayload::ProofState(ProofState { y, state, .. }) => {
                let pk = y.to_string();
                msgs.get_mut(&pk)
                    .map(|x: &mut Vec<State>| {
                        x.push(state);
                    })
                    .unwrap_or_else(|| {
                        msgs.insert(pk, vec![state]);
                    });
            }
            _ => bail!("Wrong message received"),
        }
    }

    for keys in public_keys_to_listen {
        let statuses = msgs.remove(&keys).expect("some events");
        assert_eq!(statuses, vec![State::Pending, State::Pending, State::Spent]);
    }

    assert!(listener.try_recv().is_err(), "no other event is happening");
    assert!(msgs.is_empty(), "Only expected key events are received");

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_swap_unbalanced() -> Result<()> {
    let mint = initialize().await;

    let keys = mint.pubkeys().await?.keysets.first().unwrap().clone().keys;
    let keyset_id = Id::from(&keys);

    let proofs = mint_proofs(mint, 100.into(), &SplitTarget::default(), keys).await?;

    let preswap = PreMintSecrets::random(keyset_id, 95.into(), &SplitTarget::default())?;

    let swap_request = SwapRequest::new(proofs.clone(), preswap.blinded_messages());

    match mint.process_swap_request(swap_request).await {
        Ok(_) => bail!("Swap was allowed unbalanced"),
        Err(err) => match err {
            cdk::Error::TransactionUnbalanced(_, _, _) => (),
            _ => bail!("Wrong error returned"),
        },
    }

    let preswap = PreMintSecrets::random(keyset_id, 101.into(), &SplitTarget::default())?;

    let swap_request = SwapRequest::new(proofs.clone(), preswap.blinded_messages());

    match mint.process_swap_request(swap_request).await {
        Ok(_) => bail!("Swap was allowed unbalanced"),
        Err(err) => match err {
            cdk::Error::TransactionUnbalanced(_, _, _) => (),
            _ => bail!("Wrong error returned"),
        },
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_swap_overpay_underpay_fee() -> Result<()> {
    let mint = new_mint(1).await;

    mint.rotate_keyset(CurrencyUnit::Sat, 1, 32, 1, HashMap::new())
        .await?;

    let keys = mint.pubkeys().await?.keysets.first().unwrap().clone().keys;
    let keyset_id = Id::from(&keys);

    let proofs = mint_proofs(&mint, 1000.into(), &SplitTarget::default(), keys).await?;

    let preswap = PreMintSecrets::random(keyset_id, 9998.into(), &SplitTarget::default())?;

    let swap_request = SwapRequest::new(proofs.clone(), preswap.blinded_messages());

    // Attempt to swap overpaying fee
    match mint.process_swap_request(swap_request).await {
        Ok(_) => bail!("Swap was allowed unbalanced"),
        Err(err) => match err {
            cdk::Error::TransactionUnbalanced(_, _, _) => (),
            _ => {
                println!("{:?}", err);
                bail!("Wrong error returned")
            }
        },
    }

    let preswap = PreMintSecrets::random(keyset_id, 1000.into(), &SplitTarget::default())?;

    let swap_request = SwapRequest::new(proofs.clone(), preswap.blinded_messages());

    // Attempt to swap underpaying fee
    match mint.process_swap_request(swap_request).await {
        Ok(_) => bail!("Swap was allowed unbalanced"),
        Err(err) => match err {
            cdk::Error::TransactionUnbalanced(_, _, _) => (),
            _ => {
                println!("{:?}", err);
                bail!("Wrong error returned")
            }
        },
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_mint_enforce_fee() -> Result<()> {
    let mint = new_mint(1).await;

    let keys = mint.pubkeys().await?.keysets.first().unwrap().clone().keys;
    let keyset_id = Id::from(&keys);

    let mut proofs = mint_proofs(&mint, 1010.into(), &SplitTarget::Value(1.into()), keys).await?;

    let five_proofs: Vec<_> = proofs.drain(..5).collect();

    let preswap = PreMintSecrets::random(keyset_id, 5.into(), &SplitTarget::default())?;

    let swap_request = SwapRequest::new(five_proofs.clone(), preswap.blinded_messages());

    // Attempt to swap underpaying fee
    match mint.process_swap_request(swap_request).await {
        Ok(_) => bail!("Swap was allowed unbalanced"),
        Err(err) => match err {
            cdk::Error::TransactionUnbalanced(_, _, _) => (),
            _ => {
                println!("{:?}", err);
                bail!("Wrong error returned")
            }
        },
    }

    let preswap = PreMintSecrets::random(keyset_id, 4.into(), &SplitTarget::default())?;

    let swap_request = SwapRequest::new(five_proofs.clone(), preswap.blinded_messages());

    let _ = mint.process_swap_request(swap_request).await?;

    let thousnad_proofs: Vec<_> = proofs.drain(..1001).collect();

    let preswap = PreMintSecrets::random(keyset_id, 1000.into(), &SplitTarget::default())?;

    let swap_request = SwapRequest::new(thousnad_proofs.clone(), preswap.blinded_messages());

    // Attempt to swap underpaying fee
    match mint.process_swap_request(swap_request).await {
        Ok(_) => bail!("Swap was allowed unbalanced"),
        Err(err) => match err {
            cdk::Error::TransactionUnbalanced(_, _, _) => (),
            _ => {
                println!("{:?}", err);
                bail!("Wrong error returned")
            }
        },
    }

    let preswap = PreMintSecrets::random(keyset_id, 999.into(), &SplitTarget::default())?;

    let swap_request = SwapRequest::new(thousnad_proofs.clone(), preswap.blinded_messages());

    let _ = mint.process_swap_request(swap_request).await?;

    Ok(())
}
