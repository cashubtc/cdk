use std::env;
use std::str::FromStr;
use std::sync::Arc;

use bip39::Mnemonic;
use cdk::amount::{Amount, SplitTarget};
use cdk::cdk_database::WalletMemoryDatabase;
use cdk::mint_url::MintUrl;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{
    AuthProof, AuthToken, BlindAuthToken, CheckStateRequest, CurrencyUnit, MeltBolt11Request,
    MeltQuoteBolt11Request, MeltQuoteState, MintBolt11Request, MintQuoteBolt11Request,
    RestoreRequest, State, SwapRequest,
};
use cdk::wallet::{HttpClient, MintConnector, Wallet};
use cdk::{Error, OidcClient};
use cdk_fake_wallet::create_fake_invoice;
use cdk_integration_tests::init_auth_mint::top_up_blind_auth_proofs;
use cdk_integration_tests::{fund_wallet, wait_for_mint_to_be_paid};

const MINT_URL: &str = "http://127.0.0.1:8087";
const ENV_OIDC_USER: &str = "CDK_TEST_OIDC_USER";
const ENV_OIDC_PASSWORD: &str = "CDK_TEST_OIDC_PASSWORD";

fn get_oidc_credentials() -> (String, String) {
    let user = env::var(ENV_OIDC_USER).unwrap_or_else(|_| "test".to_string());
    let password = env::var(ENV_OIDC_PASSWORD).unwrap_or_else(|_| "test".to_string());
    (user, password)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_quote_status_without_auth() {
    let client = HttpClient::new(MintUrl::from_str(MINT_URL).expect("Valid mint url"));

    // Test mint quote status
    {
        let quote_res = client
            .get_mint_quote_status("123e4567-e89b-12d3-a456-426614174000", None)
            .await;

        assert!(
            matches!(quote_res, Err(Error::AuthRequired)),
            "Expected AuthRequired error, got {:?}",
            quote_res
        );
    }

    // Test melt quote status
    {
        let quote_res = client
            .get_melt_quote_status("123e4567-e89b-12d3-a456-426614174000", None)
            .await;

        assert!(
            matches!(quote_res, Err(Error::AuthRequired)),
            "Expected AuthRequired error, got {:?}",
            quote_res
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_mint_without_auth() {
    let client = HttpClient::new(MintUrl::from_str(MINT_URL).expect("Valid mint url"));
    {
        let request = MintQuoteBolt11Request {
            unit: CurrencyUnit::Sat,
            amount: 10.into(),
            description: None,
            pubkey: None,
        };

        let quote_res = client.post_mint_quote(request, None).await;

        assert!(
            matches!(quote_res, Err(Error::AuthRequired)),
            "Expected AuthRequired error, got {:?}",
            quote_res
        );
    }

    {
        let request = MintBolt11Request {
            quote: "123e4567-e89b-12d3-a456-426614174000".to_string(),
            outputs: vec![],
            signature: None,
        };

        let mint_res = client.post_mint(request, None).await;

        assert!(
            matches!(mint_res, Err(Error::AuthRequired)),
            "Expected AuthRequired error, got {:?}",
            mint_res
        );
    }

    {
        let mint_res = client
            .get_mint_quote_status("123e4567-e89b-12d3-a456-426614174000", None)
            .await;

        assert!(
            matches!(mint_res, Err(Error::AuthRequired)),
            "Expected AuthRequired error, got {:?}",
            mint_res
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_swap_without_auth() {
    let client = HttpClient::new(MintUrl::from_str(MINT_URL).expect("Valid mint url"));

    let request = SwapRequest {
        inputs: vec![],
        outputs: vec![],
    };

    let quote_res = client.post_swap(request, None).await;

    assert!(
        matches!(quote_res, Err(Error::AuthRequired)),
        "Expected AuthRequired error, got {:?}",
        quote_res
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_melt_without_auth() {
    let client = HttpClient::new(MintUrl::from_str(MINT_URL).expect("Valid mint url"));

    // Test melt quote request
    {
        let request = MeltQuoteBolt11Request {
            request: create_fake_invoice(100, "".to_string()),
            unit: CurrencyUnit::Sat,
            options: None,
        };

        let quote_res = client.post_melt_quote(request, None).await;

        assert!(
            matches!(quote_res, Err(Error::AuthRequired)),
            "Expected AuthRequired error, got {:?}",
            quote_res
        );
    }

    // Test melt quote
    {
        let request = MeltQuoteBolt11Request {
            request: create_fake_invoice(100, "".to_string()),
            unit: CurrencyUnit::Sat,
            options: None,
        };

        let quote_res = client.post_melt_quote(request, None).await;

        assert!(
            matches!(quote_res, Err(Error::AuthRequired)),
            "Expected AuthRequired error, got {:?}",
            quote_res
        );
    }

    // Test melt
    {
        let request = MeltBolt11Request {
            inputs: vec![],
            outputs: None,
            quote: "123e4567-e89b-12d3-a456-426614174000".to_string(),
        };

        let melt_res = client.post_melt(request, None).await;

        assert!(
            matches!(melt_res, Err(Error::AuthRequired)),
            "Expected AuthRequired error, got {:?}",
            melt_res
        );
    }

    // Check melt quote state
    {
        let melt_res = client
            .get_melt_quote_status("123e4567-e89b-12d3-a456-426614174000", None)
            .await;

        assert!(
            matches!(melt_res, Err(Error::AuthRequired)),
            "Expected AuthRequired error, got {:?}",
            melt_res
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_check_without_auth() {
    let client = HttpClient::new(MintUrl::from_str(MINT_URL).expect("Valid mint url"));

    let request = CheckStateRequest { ys: vec![] };

    let quote_res = client.post_check_state(request, None).await;

    assert!(
        matches!(quote_res, Err(Error::AuthRequired)),
        "Expected AuthRequired error, got {:?}",
        quote_res
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_restore_without_auth() {
    let client = HttpClient::new(MintUrl::from_str(MINT_URL).expect("Valid mint url"));

    let request = RestoreRequest { outputs: vec![] };

    let restore_res = client.post_restore(request, None).await;

    assert!(
        matches!(restore_res, Err(Error::AuthRequired)),
        "Expected AuthRequired error, got {:?}",
        restore_res
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_mint_blind_auth() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
        None,
    )
    .expect("Wallet");

    let wallet = Arc::new(wallet);

    let (user, password) = get_oidc_credentials();
    top_up_blind_auth_proofs(wallet.clone(), 10, &user, &password).await;

    let proofs = wallet
        .get_unspent_auth_proofs()
        .await
        .expect("Could not get auth proofs");

    assert!(proofs.len() == 10)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_mint_with_auth() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
        None,
    )
    .expect("Wallet");

    let wallet = Arc::new(wallet);

    let (user, password) = get_oidc_credentials();
    top_up_blind_auth_proofs(wallet.clone(), 10, &user, &password).await;

    let mint_amount: Amount = 100.into();

    let mint_quote = wallet
        .mint_quote(mint_amount, None)
        .await
        .expect("failed to get mint quote");

    wait_for_mint_to_be_paid(&wallet, &mint_quote.id, 60)
        .await
        .expect("failed to wait for payment");

    let proofs = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await
        .expect("could not mint");

    assert!(proofs.total_amount().expect("Could not get proofs amount") == mint_amount);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_swap_with_auth() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
        None,
    )
    .expect("Wallet");

    let wallet = Arc::new(wallet);

    let (user, password) = get_oidc_credentials();
    top_up_blind_auth_proofs(wallet.clone(), 10, &user, &password).await;

    fund_wallet(wallet.clone(), 100.into()).await;

    let proofs = wallet
        .get_unspent_proofs()
        .await
        .expect("Could not get proofs");

    let swapped_proofs = wallet
        .swap(
            Some(proofs.total_amount().unwrap()),
            SplitTarget::default(),
            proofs.clone(),
            None,
            false,
        )
        .await
        .expect("Could not swap")
        .expect("Could not swap");

    let check_spent = wallet
        .check_proofs_spent(proofs.clone())
        .await
        .expect("Could not check proofs");

    for state in check_spent {
        if state.state != State::Spent {
            panic!("Input proofs should be spent");
        }
    }

    assert!(swapped_proofs.total_amount().unwrap() == proofs.total_amount().unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_melt_with_auth() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
        None,
    )
    .expect("Wallet");

    let wallet = Arc::new(wallet);

    let (user, password) = get_oidc_credentials();
    top_up_blind_auth_proofs(wallet.clone(), 10, &user, &password).await;

    fund_wallet(wallet.clone(), 100.into()).await;

    let bolt11 = create_fake_invoice(2_000, "".to_string());

    let melt_quote = wallet
        .melt_quote(bolt11.to_string(), None)
        .await
        .expect("Could not get melt quote");

    let after_melt = wallet.melt(&melt_quote.id).await.expect("Could not melt");

    assert!(after_melt.state == MeltQuoteState::Paid);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_mint_auth_over_max() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
        None,
    )
    .expect("Wallet");

    let wallet = Arc::new(wallet);

    let mint_info = wallet
        .get_mint_info()
        .await
        .expect("Mint info not found")
        .expect("Mint info not found");

    let openid_discovery = mint_info
        .nuts
        .nut21
        .expect("Nutxx defined")
        .openid_discovery;

    let oidc_client = OidcClient::new(openid_discovery);

    let (user, password) = get_oidc_credentials();
    let access_token = oidc_client
        .get_access_token_with_user_password(user, password)
        .await
        .expect("Could not get cat");

    {
        let mut cat = wallet.cat.write().await;

        *cat = Some(access_token);
    }

    let auth_proofs = wallet
        .mint_blind_auth((mint_info.nuts.nut22.expect("Auth enabled").bat_max_mint + 1).into())
        .await;

    assert!(
        matches!(
            auth_proofs,
            Err(Error::AmountOutofLimitRange(
                Amount::ZERO,
                Amount::ZERO,
                Amount::ZERO,
            ))
        ),
        "Expected amount out of limit error, got {:?}",
        auth_proofs
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_reuse_auth_proof() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
        None,
    )
    .expect("Wallet");

    let wallet = Arc::new(wallet);

    let (user, password) = get_oidc_credentials();
    top_up_blind_auth_proofs(wallet.clone(), 10, &user, &password).await;

    let auth_token = wallet
        .get_blind_auth_token()
        .await
        .expect("Could not get auth token")
        .expect("Wallet has auth balance");

    let client = HttpClient::new(MintUrl::from_str(MINT_URL).expect("Valid mint url"));
    {
        let request = MintQuoteBolt11Request {
            unit: CurrencyUnit::Sat,
            amount: 10.into(),
            description: None,
            pubkey: None,
        };

        let _quote_res = client
            .post_mint_quote(request, Some(auth_token.clone()))
            .await
            .expect("Auth is valid");
    }

    {
        let request = MintQuoteBolt11Request {
            unit: CurrencyUnit::Sat,
            amount: 10.into(),
            description: None,
            pubkey: None,
        };

        let quote_res = client.post_mint_quote(request, Some(auth_token)).await;

        assert!(
            matches!(quote_res, Err(Error::TokenAlreadySpent)),
            "Expected AuthRequired error, got {:?}",
            quote_res
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_melt_with_invalid_auth() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
        None,
    )
    .expect("Wallet");

    let wallet = Arc::new(wallet);

    let (user, password) = get_oidc_credentials();
    top_up_blind_auth_proofs(wallet.clone(), 10, &user, &password).await;

    fund_wallet(wallet.clone(), 1.into()).await;

    let proofs = wallet
        .get_unspent_proofs()
        .await
        .expect("wallet has proofs");

    println!("{:#?}", proofs);
    let proof = proofs.first().expect("wallet has one proof");

    let client = HttpClient::new(MintUrl::from_str(MINT_URL).expect("Valid mint url"));
    {
        let invalid_auth_proof = AuthProof {
            keyset_id: proof.keyset_id,
            secret: proof.secret.clone(),
            c: proof.c,
        };

        let auth_token = AuthToken::BlindAuth(BlindAuthToken::new(invalid_auth_proof));

        let request = MintQuoteBolt11Request {
            unit: CurrencyUnit::Sat,
            amount: 10.into(),
            description: None,
            pubkey: None,
        };

        let quote_res = client.post_mint_quote(request, Some(auth_token)).await;

        assert!(
            matches!(quote_res, Err(Error::AuthRequired)),
            "Expected AuthRequired error, got {:?}",
            quote_res
        );
    }

    {
        let blind_auth_keyset = wallet
            .get_active_mint_blind_auth_keyset()
            .await
            .expect("Could not get blind auth keyset");

        let invalid_auth_proof = AuthProof {
            keyset_id: blind_auth_keyset.id,
            secret: proof.secret.clone(),
            c: proof.c,
        };

        let auth_token = AuthToken::BlindAuth(BlindAuthToken::new(invalid_auth_proof));

        let request = MintQuoteBolt11Request {
            unit: CurrencyUnit::Sat,
            amount: 10.into(),
            description: None,
            pubkey: None,
        };

        let quote_res = client.post_mint_quote(request, Some(auth_token)).await;

        assert!(quote_res.is_err())
    }
}
