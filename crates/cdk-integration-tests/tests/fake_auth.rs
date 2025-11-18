use std::env;
use std::str::FromStr;
use std::sync::Arc;

use bip39::Mnemonic;
use cashu::{MintAuthRequest, MintInfo};
use cdk::amount::{Amount, SplitTarget};
use cdk::mint_url::MintUrl;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{
    AuthProof, AuthToken, BlindAuthToken, CheckStateRequest, CurrencyUnit, MeltQuoteBolt11Request,
    MeltQuoteState, MeltRequest, MintQuoteBolt11Request, MintRequest, RestoreRequest, State,
    SwapRequest,
};
use cdk::wallet::{AuthHttpClient, AuthMintConnector, HttpClient, MintConnector, WalletBuilder};
use cdk::{Error, OidcClient};
use cdk_fake_wallet::create_fake_invoice;
use cdk_integration_tests::fund_wallet;
use cdk_sqlite::wallet::memory;

const MINT_URL: &str = "http://127.0.0.1:8087";
const ENV_OIDC_USER: &str = "CDK_TEST_OIDC_USER";
const ENV_OIDC_PASSWORD: &str = "CDK_TEST_OIDC_PASSWORD";

fn get_oidc_credentials() -> (String, String) {
    let user = env::var(ENV_OIDC_USER).unwrap_or_else(|_| "test".to_string());
    let password = env::var(ENV_OIDC_PASSWORD).unwrap_or_else(|_| "test".to_string());
    (user, password)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_invalid_credentials() {
    let db = Arc::new(memory::empty().await.unwrap());

    let wallet = WalletBuilder::new()
        .mint_url(MintUrl::from_str(MINT_URL).expect("Valid mint url"))
        .unit(CurrencyUnit::Sat)
        .localstore(db.clone())
        .seed(Mnemonic::generate(12).unwrap().to_seed_normalized(""))
        .build()
        .expect("Wallet");

    let mint_info = wallet
        .fetch_mint_info()
        .await
        .expect("mint info")
        .expect("could not get mint info");

    // Try to get a token with invalid credentials
    let token_result =
        get_custom_access_token(&mint_info, "invalid_user", "invalid_password").await;

    // Should fail with an error
    assert!(
        token_result.is_err(),
        "Expected authentication to fail with invalid credentials"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_quote_status_without_auth() {
    let client = HttpClient::new(MintUrl::from_str(MINT_URL).expect("Valid mint url"), None);

    // Test mint quote status
    {
        let quote_res = client
            .get_mint_quote_status("123e4567-e89b-12d3-a456-426614174000")
            .await;

        assert!(
            matches!(quote_res, Err(Error::BlindAuthRequired)),
            "Expected AuthRequired error, got {:?}",
            quote_res
        );
    }

    // Test melt quote status
    {
        let quote_res = client
            .get_melt_quote_status("123e4567-e89b-12d3-a456-426614174000")
            .await;

        assert!(
            matches!(quote_res, Err(Error::BlindAuthRequired)),
            "Expected AuthRequired error, got {:?}",
            quote_res
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_mint_without_auth() {
    let client = HttpClient::new(MintUrl::from_str(MINT_URL).expect("Valid mint url"), None);
    {
        let request = MintQuoteBolt11Request {
            unit: CurrencyUnit::Sat,
            amount: 10.into(),
            description: None,
            pubkey: None,
        };

        let quote_res = client.post_mint_quote(request).await;

        assert!(
            matches!(quote_res, Err(Error::BlindAuthRequired)),
            "Expected AuthRequired error, got {:?}",
            quote_res
        );
    }

    {
        let request = MintRequest {
            quote: "123e4567-e89b-12d3-a456-426614174000".to_string(),
            outputs: vec![],
            signature: None,
        };

        let mint_res = client.post_mint(request).await;

        assert!(
            matches!(mint_res, Err(Error::BlindAuthRequired)),
            "Expected AuthRequired error, got {:?}",
            mint_res
        );
    }

    {
        let mint_res = client
            .get_mint_quote_status("123e4567-e89b-12d3-a456-426614174000")
            .await;

        assert!(
            matches!(mint_res, Err(Error::BlindAuthRequired)),
            "Expected AuthRequired error, got {:?}",
            mint_res
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_mint_bat_without_cat() {
    let client = AuthHttpClient::new(MintUrl::from_str(MINT_URL).expect("valid mint url"), None);

    let res = client
        .post_mint_blind_auth(MintAuthRequest { outputs: vec![] })
        .await;

    assert!(
        matches!(res, Err(Error::ClearAuthRequired)),
        "Expected AuthRequired error, got {:?}",
        res
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_swap_without_auth() {
    let client = HttpClient::new(MintUrl::from_str(MINT_URL).expect("Valid mint url"), None);

    let request = SwapRequest::new(vec![], vec![]);

    let quote_res = client.post_swap(request).await;

    assert!(
        matches!(quote_res, Err(Error::BlindAuthRequired)),
        "Expected AuthRequired error, got {:?}",
        quote_res
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_melt_without_auth() {
    let client = HttpClient::new(MintUrl::from_str(MINT_URL).expect("Valid mint url"), None);

    // Test melt quote request
    {
        let request = MeltQuoteBolt11Request {
            request: create_fake_invoice(100, "".to_string()),
            unit: CurrencyUnit::Sat,
            options: None,
        };

        let quote_res = client.post_melt_quote(request).await;

        assert!(
            matches!(quote_res, Err(Error::BlindAuthRequired)),
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

        let quote_res = client.post_melt_quote(request).await;

        assert!(
            matches!(quote_res, Err(Error::BlindAuthRequired)),
            "Expected AuthRequired error, got {:?}",
            quote_res
        );
    }

    // Test melt
    {
        let request = MeltRequest::new(
            "123e4567-e89b-12d3-a456-426614174000".to_string(),
            vec![],
            None,
        );

        let melt_res = client.post_melt(request).await;

        assert!(
            matches!(melt_res, Err(Error::BlindAuthRequired)),
            "Expected AuthRequired error, got {:?}",
            melt_res
        );
    }

    // Check melt quote state
    {
        let melt_res = client
            .get_melt_quote_status("123e4567-e89b-12d3-a456-426614174000")
            .await;

        assert!(
            matches!(melt_res, Err(Error::BlindAuthRequired)),
            "Expected AuthRequired error, got {:?}",
            melt_res
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_check_without_auth() {
    let client = HttpClient::new(MintUrl::from_str(MINT_URL).expect("Valid mint url"), None);

    let request = CheckStateRequest { ys: vec![] };

    let quote_res = client.post_check_state(request).await;

    assert!(
        matches!(quote_res, Err(Error::BlindAuthRequired)),
        "Expected AuthRequired error, got {:?}",
        quote_res
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_restore_without_auth() {
    let client = HttpClient::new(MintUrl::from_str(MINT_URL).expect("Valid mint url"), None);

    let request = RestoreRequest { outputs: vec![] };

    let restore_res = client.post_restore(request).await;

    assert!(
        matches!(restore_res, Err(Error::BlindAuthRequired)),
        "Expected AuthRequired error, got {:?}",
        restore_res
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_mint_blind_auth() {
    let db = Arc::new(memory::empty().await.unwrap());

    let wallet = WalletBuilder::new()
        .mint_url(MintUrl::from_str(MINT_URL).expect("Valid mint url"))
        .unit(CurrencyUnit::Sat)
        .localstore(db.clone())
        .seed(Mnemonic::generate(12).unwrap().to_seed_normalized(""))
        .build()
        .expect("Wallet");
    let mint_info = wallet.fetch_mint_info().await.unwrap().unwrap();

    let (access_token, _) = get_access_token(&mint_info).await;

    wallet.set_cat(access_token).await.unwrap();

    wallet
        .mint_blind_auth(10.into())
        .await
        .expect("Could not mint blind auth");

    let proofs = wallet
        .get_unspent_auth_proofs()
        .await
        .expect("Could not get auth proofs");

    assert!(proofs.len() == 10)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_mint_with_auth() {
    let db = Arc::new(memory::empty().await.unwrap());

    let wallet = WalletBuilder::new()
        .mint_url(MintUrl::from_str(MINT_URL).expect("Valid mint url"))
        .unit(CurrencyUnit::Sat)
        .localstore(db.clone())
        .seed(Mnemonic::generate(12).unwrap().to_seed_normalized(""))
        .build()
        .expect("Wallet");

    let mint_info = wallet
        .fetch_mint_info()
        .await
        .expect("mint info")
        .expect("could not get mint info");

    let (access_token, _) = get_access_token(&mint_info).await;

    println!("st{}", access_token);

    wallet.set_cat(access_token).await.unwrap();

    wallet
        .mint_blind_auth(10.into())
        .await
        .expect("Could not mint blind auth");

    let wallet = Arc::new(wallet);

    let mint_amount: Amount = 100.into();

    let quote = wallet.mint_quote(mint_amount, None).await.unwrap();

    let proofs = wallet
        .wait_and_mint_quote(
            quote.clone(),
            SplitTarget::default(),
            None,
            tokio::time::Duration::from_secs(60),
        )
        .await
        .expect("payment");

    assert!(proofs.total_amount().expect("Could not get proofs amount") == mint_amount);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_swap_with_auth() {
    let db = Arc::new(memory::empty().await.unwrap());

    let wallet = WalletBuilder::new()
        .mint_url(MintUrl::from_str(MINT_URL).expect("Valid mint url"))
        .unit(CurrencyUnit::Sat)
        .localstore(db.clone())
        .seed(Mnemonic::generate(12).unwrap().to_seed_normalized(""))
        .build()
        .expect("Wallet");
    let mint_info = wallet.fetch_mint_info().await.unwrap().unwrap();
    let (access_token, _) = get_access_token(&mint_info).await;

    wallet.set_cat(access_token).await.unwrap();

    let wallet = Arc::new(wallet);

    wallet.mint_blind_auth(10.into()).await.unwrap();

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
    let db = Arc::new(memory::empty().await.unwrap());

    let wallet = WalletBuilder::new()
        .mint_url(MintUrl::from_str(MINT_URL).expect("Valid mint url"))
        .unit(CurrencyUnit::Sat)
        .localstore(db.clone())
        .seed(Mnemonic::generate(12).unwrap().to_seed_normalized(""))
        .build()
        .expect("Wallet");

    let mint_info = wallet
        .fetch_mint_info()
        .await
        .expect("Mint info not found")
        .expect("Mint info not found");

    let (access_token, _) = get_access_token(&mint_info).await;

    wallet.set_cat(access_token).await.unwrap();

    let wallet = Arc::new(wallet);

    wallet.mint_blind_auth(10.into()).await.unwrap();

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
    let db = Arc::new(memory::empty().await.unwrap());

    let wallet = WalletBuilder::new()
        .mint_url(MintUrl::from_str(MINT_URL).expect("Valid mint url"))
        .unit(CurrencyUnit::Sat)
        .localstore(db.clone())
        .seed(Mnemonic::generate(12).unwrap().to_seed_normalized(""))
        .build()
        .expect("Wallet");

    let wallet = Arc::new(wallet);

    let mint_info = wallet
        .fetch_mint_info()
        .await
        .expect("Mint info not found")
        .expect("Mint info not found");

    let (access_token, _) = get_access_token(&mint_info).await;

    wallet.set_cat(access_token).await.unwrap();

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
    let db = Arc::new(memory::empty().await.unwrap());

    let wallet = WalletBuilder::new()
        .mint_url(MintUrl::from_str(MINT_URL).expect("Valid mint url"))
        .unit(CurrencyUnit::Sat)
        .localstore(db.clone())
        .seed(Mnemonic::generate(12).unwrap().to_seed_normalized(""))
        .build()
        .expect("Wallet");
    let mint_info = wallet.fetch_mint_info().await.unwrap().unwrap();

    let (access_token, _) = get_access_token(&mint_info).await;

    wallet.set_cat(access_token).await.unwrap();

    wallet.mint_blind_auth(1.into()).await.unwrap();

    let proofs = wallet
        .localstore
        .get_proofs(None, Some(CurrencyUnit::Auth), None, None)
        .await
        .unwrap();

    assert!(proofs.len() == 1);

    {
        let quote = wallet
            .mint_quote(10.into(), None)
            .await
            .expect("Quote should be allowed");

        assert!(quote.amount == Some(10.into()));
    }

    let mut tx = wallet.localstore.begin_db_transaction().await.unwrap();
    tx.update_proofs(proofs, vec![]).await.unwrap();
    tx.commit().await.unwrap();

    {
        let quote_res = wallet.mint_quote(10.into(), None).await;
        assert!(
            matches!(quote_res, Err(Error::TokenAlreadySpent)),
            "Expected AuthRequired error, got {:?}",
            quote_res
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_melt_with_invalid_auth() {
    let db = Arc::new(memory::empty().await.unwrap());

    let wallet = WalletBuilder::new()
        .mint_url(MintUrl::from_str(MINT_URL).expect("Valid mint url"))
        .unit(CurrencyUnit::Sat)
        .localstore(db.clone())
        .seed(Mnemonic::generate(12).unwrap().to_seed_normalized(""))
        .build()
        .expect("Wallet");
    let mint_info = wallet.fetch_mint_info().await.unwrap().unwrap();

    let (access_token, _) = get_access_token(&mint_info).await;

    wallet.set_cat(access_token).await.unwrap();

    wallet.mint_blind_auth(10.into()).await.unwrap();

    fund_wallet(Arc::new(wallet.clone()), 1.into()).await;

    let proofs = wallet
        .get_unspent_proofs()
        .await
        .expect("wallet has proofs");

    println!("{:#?}", proofs);
    let proof = proofs.first().expect("wallet has one proof");

    let client = HttpClient::new(MintUrl::from_str(MINT_URL).expect("Valid mint url"), None);
    {
        let invalid_auth_proof = AuthProof {
            keyset_id: proof.keyset_id,
            secret: proof.secret.clone(),
            c: proof.c,
            dleq: proof.dleq.clone(),
        };

        let _auth_token = AuthToken::BlindAuth(BlindAuthToken::new(invalid_auth_proof));

        let request = MintQuoteBolt11Request {
            unit: CurrencyUnit::Sat,
            amount: 10.into(),
            description: None,
            pubkey: None,
        };

        let quote_res = client.post_mint_quote(request).await;

        assert!(
            matches!(quote_res, Err(Error::BlindAuthRequired)),
            "Expected AuthRequired error, got {:?}",
            quote_res
        );
    }

    {
        let (access_token, _) = get_access_token(&mint_info).await;

        wallet.set_cat(access_token).await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_refresh_access_token() {
    let db = Arc::new(memory::empty().await.unwrap());

    let wallet = WalletBuilder::new()
        .mint_url(MintUrl::from_str(MINT_URL).expect("Valid mint url"))
        .unit(CurrencyUnit::Sat)
        .localstore(db.clone())
        .seed(Mnemonic::generate(12).unwrap().to_seed_normalized(""))
        .build()
        .expect("Wallet");

    let mint_info = wallet
        .fetch_mint_info()
        .await
        .expect("mint info")
        .expect("could not get mint info");

    let (access_token, refresh_token) = get_access_token(&mint_info).await;

    // Set the initial access token and refresh token
    wallet.set_cat(access_token.clone()).await.unwrap();
    wallet
        .set_refresh_token(refresh_token.clone())
        .await
        .unwrap();

    // Mint some blind auth tokens with the initial access token
    wallet.mint_blind_auth(5.into()).await.unwrap();

    // Refresh the access token
    wallet.refresh_access_token().await.unwrap();

    // Verify we can still perform operations with the refreshed token
    let mint_amount: Amount = 10.into();

    // Try to mint more blind auth tokens with the refreshed token
    let auth_proofs = wallet.mint_blind_auth(5.into()).await.unwrap();
    assert_eq!(auth_proofs.len(), 5);

    let total_auth_proofs = wallet.get_unspent_auth_proofs().await.unwrap();
    assert_eq!(total_auth_proofs.len(), 10); // 5 from before refresh + 5 after refresh

    // Try to get a mint quote with the refreshed token
    let mint_quote = wallet
        .mint_quote(mint_amount, None)
        .await
        .expect("failed to get mint quote with refreshed token");

    assert_eq!(mint_quote.amount, Some(mint_amount));

    // Verify the total number of auth tokens
    let total_auth_proofs = wallet.get_unspent_auth_proofs().await.unwrap();
    assert_eq!(total_auth_proofs.len(), 9); // 5 from before refresh + 5 after refresh - 1 for the quote
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_invalid_refresh_token() {
    let db = Arc::new(memory::empty().await.unwrap());

    let wallet = WalletBuilder::new()
        .mint_url(MintUrl::from_str(MINT_URL).expect("Valid mint url"))
        .unit(CurrencyUnit::Sat)
        .localstore(db.clone())
        .seed(Mnemonic::generate(12).unwrap().to_seed_normalized(""))
        .build()
        .expect("Wallet");

    let mint_info = wallet
        .fetch_mint_info()
        .await
        .expect("mint info")
        .expect("could not get mint info");

    let (access_token, _) = get_access_token(&mint_info).await;

    // Set the initial access token
    wallet.set_cat(access_token.clone()).await.unwrap();

    // Set an invalid refresh token
    wallet
        .set_refresh_token("invalid_refresh_token".to_string())
        .await
        .unwrap();

    // Attempt to refresh the access token with an invalid refresh token
    let refresh_result = wallet.refresh_access_token().await;

    // Should fail with an error
    assert!(refresh_result.is_err(), "Expected refresh token error");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_auth_token_spending_order() {
    let db = Arc::new(memory::empty().await.unwrap());

    let wallet = WalletBuilder::new()
        .mint_url(MintUrl::from_str(MINT_URL).expect("Valid mint url"))
        .unit(CurrencyUnit::Sat)
        .localstore(db.clone())
        .seed(Mnemonic::generate(12).unwrap().to_seed_normalized(""))
        .build()
        .expect("Wallet");

    let mint_info = wallet
        .fetch_mint_info()
        .await
        .expect("mint info")
        .expect("could not get mint info");

    let (access_token, _) = get_access_token(&mint_info).await;

    wallet.set_cat(access_token).await.unwrap();

    // Mint auth tokens in two batches to test ordering
    wallet.mint_blind_auth(2.into()).await.unwrap();

    // Get the first batch of auth proofs
    let first_batch = wallet.get_unspent_auth_proofs().await.unwrap();
    assert_eq!(first_batch.len(), 2);

    // Mint a second batch
    wallet.mint_blind_auth(3.into()).await.unwrap();

    // Get all auth proofs
    let all_proofs = wallet.get_unspent_auth_proofs().await.unwrap();
    assert_eq!(all_proofs.len(), 5);

    // Use tokens and verify they're used in the expected order (FIFO)
    for i in 0..3 {
        let mint_quote = wallet
            .mint_quote(10.into(), None)
            .await
            .expect("failed to get mint quote");

        assert_eq!(mint_quote.amount, Some(10.into()));

        // Check remaining tokens after each operation
        let remaining = wallet.get_unspent_auth_proofs().await.unwrap();
        assert_eq!(
            remaining.len(),
            5 - (i + 1),
            "Expected {} remaining auth tokens after {} operations",
            5 - (i + 1),
            i + 1
        );
    }
}

async fn get_access_token(mint_info: &MintInfo) -> (String, String) {
    let openid_discovery = mint_info
        .nuts
        .nut21
        .clone()
        .expect("Nutxx defined")
        .openid_discovery;

    let oidc_client = OidcClient::new(openid_discovery, None);

    // Get the token endpoint from the OIDC configuration
    let token_url = oidc_client
        .get_oidc_config()
        .await
        .expect("Failed to get OIDC config")
        .token_endpoint;

    // Create the request parameters
    let (user, password) = get_oidc_credentials();
    let params = [
        ("grant_type", "password"),
        ("client_id", "cashu-client"),
        ("username", &user),
        ("password", &password),
    ];

    // Make the token request directly
    let client = reqwest::Client::new();
    let response = client
        .post(token_url)
        .form(&params)
        .send()
        .await
        .expect("Failed to send token request");

    let token_response: serde_json::Value = response
        .json()
        .await
        .expect("Failed to parse token response");

    let access_token = token_response["access_token"]
        .as_str()
        .expect("No access token in response")
        .to_string();

    let refresh_token = token_response["refresh_token"]
        .as_str()
        .expect("No access token in response")
        .to_string();

    (access_token, refresh_token)
}

/// Get a new access token with custom credentials
async fn get_custom_access_token(
    mint_info: &MintInfo,
    username: &str,
    password: &str,
) -> Result<(String, String), Error> {
    let openid_discovery = mint_info
        .nuts
        .nut21
        .clone()
        .expect("Nutxx defined")
        .openid_discovery;

    let oidc_client = OidcClient::new(openid_discovery, None);

    // Get the token endpoint from the OIDC configuration
    let token_url = oidc_client
        .get_oidc_config()
        .await
        .map_err(|_| Error::Custom("Failed to get OIDC config".to_string()))?
        .token_endpoint;

    // Create the request parameters
    let params = [
        ("grant_type", "password"),
        ("client_id", "cashu-client"),
        ("username", username),
        ("password", password),
    ];

    // Make the token request directly
    let client = reqwest::Client::new();
    let response = client
        .post(token_url)
        .form(&params)
        .send()
        .await
        .map_err(|_| Error::Custom("Failed to send token request".to_string()))?;

    if !response.status().is_success() {
        return Err(Error::Custom(format!(
            "Token request failed with status: {}",
            response.status()
        )));
    }

    let token_response: serde_json::Value = response
        .json()
        .await
        .map_err(|_| Error::Custom("Failed to parse token response".to_string()))?;

    let access_token = token_response["access_token"]
        .as_str()
        .ok_or_else(|| Error::Custom("No access token in response".to_string()))?
        .to_string();

    let refresh_token = token_response["refresh_token"]
        .as_str()
        .ok_or_else(|| Error::Custom("No refresh token in response".to_string()))?
        .to_string();

    Ok((access_token, refresh_token))
}
