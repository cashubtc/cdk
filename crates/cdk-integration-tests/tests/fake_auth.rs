use std::str::FromStr;

use cdk::mint_url::MintUrl;
use cdk::nuts::{
    CheckStateRequest, CurrencyUnit, MeltBolt11Request, MeltQuoteBolt11Request, MintBolt11Request,
    MintQuoteBolt11Request, RestoreRequest, SwapRequest,
};
use cdk::wallet::{HttpClient, MintConnector};
use cdk::Error;
use cdk_fake_wallet::create_fake_invoice;

const MINT_URL: &str = "http://127.0.0.1:8087";

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
