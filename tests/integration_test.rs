use std::str::FromStr;
use std::thread;
use std::time::Duration;

use bitcoin::Amount;
use lightning_invoice::Invoice;
use url::Url;

use cashu_rs::{cashu_mint::CashuMint, cashu_wallet::CashuWallet, types::{BlindedMessages, TokenData}};

const MINTURL: &str = "https://legend.lnbits.com/cashu/api/v1/SKvHRus9dmjWHhstHrsazW/";

#[ignore]
#[tokio::test]
async fn test_get_mint_keys() {
    let url = Url::from_str(MINTURL).unwrap();
    let mint = CashuMint::new(url);
    let mint_keys = mint.get_keys().await.unwrap();
    // println!("{:?}", mint_keys.0.capacity());
    assert!(mint_keys.0.capacity() > 1);
}

#[ignore]
#[tokio::test]
async fn test_get_mint_keysets() {
    let url = Url::from_str(MINTURL).unwrap();
    let mint = CashuMint::new(url);
    let mint_keysets = mint.get_keysets().await.unwrap();

    assert!(!mint_keysets.keysets.is_empty())
}

#[ignore]
#[tokio::test]
async fn test_request_mint() {
    let url = Url::from_str(MINTURL).unwrap();
    let mint = CashuMint::new(url);

    let mint = mint.request_mint(Amount::from_sat(21)).await.unwrap();

    assert!(mint.pr.check_signature().is_ok())
}

#[ignore]
#[tokio::test]
async fn test_mint() {
    let url = Url::from_str(MINTURL).unwrap();
    let mint = CashuMint::new(url);
    let mint_req = mint.request_mint(Amount::from_sat(21)).await.unwrap();
    println!("Mint Req: {:?}", mint_req.pr.to_string());

    // Since before the mint happens the invoice in the mint req has to be payed this wait is here
    // probally some way to simulate this in a better way
    // but for now pay it quick
    thread::sleep(Duration::from_secs(30));

    let blinded_messages = BlindedMessages::random(Amount::from_sat(21)).unwrap();
    let mint_res = mint.mint(blinded_messages, &mint_req.hash).await.unwrap();

    println!("Mint: {:?}", mint_res);
}

#[ignore]
#[tokio::test]
async fn test_check_fees() {
    let invoice = Invoice::from_str("lnbc10n1p3a6s0dsp5n55r506t2fv4r0mjcg30v569nk2u9s40ur4v3r3mgtscjvkvnrqqpp5lzfv8fmjzduelk74y9rsrxrayvhyzcdsh3zkdgv0g50napzalvqsdqhf9h8vmmfvdjn5gp58qengdqxq8p3aaymdcqpjrzjqwryaup9lh50kkranzgcdnn2fgvx390wgj5jd07rwr3vxeje0glc7z70cgqqg4sqqqqqqqlgqqqqrucqjq9qyysgqrjky5axsldzhqsjwsc38xa37k6t04le3ws4t26nqej62vst5xkz56qw85r6c4a3tr79588e0ceuuahwgfnkqc6n6269unlwqtvwr5vqqy0ncdq").unwrap();

    let url = Url::from_str(MINTURL).unwrap();
    let mint = CashuMint::new(url);

    let _fee = mint.check_fees(invoice).await.unwrap();
    // println!("{fee:?}");
}

#[ignore]
#[tokio::test]
async fn test_receive() {
    let url = Url::from_str(MINTURL).unwrap();
    let mint = CashuMint::new(url);
    let mint_keys = mint.get_keys().await.unwrap();

    let wallet = CashuWallet::new(mint, mint_keys);
    // FIXME: Have to manully paste an unspent token
    let token = 
    "cashuAeyJ0b2tlbiI6W3sicHJvb2ZzIjpbeyJpZCI6Im9DV2NkWXJyeVRrUiIsImFtb3VudCI6MiwiQyI6IjAyOTMwNTJhNWEwN2FjMTkxMDgyODQyZTExMDVkOTQ2MzliNWI5NmE3MTU3NTQzZTllMjdkOTg3MWU5YjE2NDJkNCIsInNlY3JldCI6IlQxZ0lYUWlpZnBNY21OMU9ENnV4Nk1rMS93bXIxU3VHU2tvVXIyTkpqZE09In1dLCJtaW50IjoiaHR0cHM6Ly9sZWdlbmQubG5iaXRzLmNvbS9jYXNodS9hcGkvdjEvU0t2SFJ1czlkbWpXSGhzdEhyc2F6VyJ9XX0=";

    let prom = wallet.receive(token).await.unwrap();
    // println!("{:?}", prom);
}

#[ignore]
#[tokio::test]
async fn test_check_spendable() {
    let url = Url::from_str(MINTURL).unwrap();
    let mint = CashuMint::new(url);
    let mint_keys = mint.get_keys().await.unwrap();

    let wallet = CashuWallet::new(mint, mint_keys);
    // FIXME: Have to manully paste an unspent token
    let token = 
        "cashuAeyJ0b2tlbiI6W3sicHJvb2ZzIjpbeyJpZCI6Im9DV2NkWXJyeVRrUiIsImFtb3VudCI6MiwiQyI6IjAyNGQ0ZDUxNWIxYzk2MWZkYzYxY2M5MDFmNzBkOGUwZDA0ZWIwYTI2MzBhNWYxYTdmM2I5ZmRhODdmMGJkNjNmNyIsInNlY3JldCI6IkVUc2pXSGJheXYyTUJQeXo1b0toay85dVdoaldIeXJkODdBQy9XY3VjbkE9In1dLCJtaW50IjoiaHR0cHM6Ly9sZWdlbmQubG5iaXRzLmNvbS9jYXNodS9hcGkvdjEvU0t2SFJ1czlkbWpXSGhzdEhyc2F6VyJ9XX0=";

    let token_data = TokenData::from_str(token).unwrap();
    let spendable = wallet.check_proofs_spent(token_data.token[0].clone().proofs).await.unwrap();
    // println!("Spendable: {:?}", spendable);
    
}

// #[ignore]
#[tokio::test]
async fn test_split() {
    let url = Url::from_str("http://localhost:3338").unwrap();
    let mint = CashuMint::new(url);
    let mint_keys = mint.get_keys().await.unwrap();

    let wallet = CashuWallet::new(mint.clone(), mint_keys);
    // FIXME: Have to manully paste an unspent token
    let token = 
    "cashuAeyJ0b2tlbiI6W3sicHJvb2ZzIjpbeyJpZCI6Im9DV2NkWXJyeVRrUiIsImFtb3VudCI6MiwiQyI6IjAyNDVjYjBhYzhlMWNmNGViMjk2ZjAyMTFiMDdjYTBjNTczOWM1MTMwMDEzMzM3MjczOTE1ZTVlMDY2NjZlOTBiZCIsInNlY3JldCI6ImRWNThLbU5VOWE0UU45c0QyVDd5bGkvam9qcWpwb3o0VVhkSGR6dkdRZ289In1dLCJtaW50IjoiaHR0cHM6Ly9sZWdlbmQubG5iaXRzLmNvbS9jYXNodS9hcGkvdjEvU0t2SFJ1czlkbWpXSGhzdEhyc2F6VyJ9XX0=";
    let proofs = wallet.receive(token).await.unwrap();

    let split = wallet.create_split(Amount::ONE_SAT, Amount::ONE_SAT, proofs).await.unwrap();
 
    println!("Split: {:#?}", split);
    println!("splint JSON {:?}", serde_json::to_string(&split.split_payload));

    let split = mint.split(split.split_payload).await;
    println!("Split res: {:#?}", split);
}


#[ignore]
#[tokio::test]
async fn test_send() {
    let url = Url::from_str(MINTURL).unwrap();
    let mint = CashuMint::new(url);
    let mint_keys = mint.get_keys().await.unwrap();

    let wallet = CashuWallet::new(mint, mint_keys);
    // FIXME: Have to manully paste an unspent token
    let token = 
    "cashuAeyJ0b2tlbiI6W3sicHJvb2ZzIjpbeyJpZCI6Im9DV2NkWXJyeVRrUiIsImFtb3VudCI6MiwiQyI6IjAzMGI4NWFhYjI5MDY2MGRlNDk4NTEzODZmYTJhZWY2MTk3YzM2MzRkZDE4OGMzMjM2ZDI2YTFhNDdmODZlNzQxNCIsInNlY3JldCI6IjNET0c3eHM2T2RRYno1Nmk1c0lRQjhndHUzbjRMdjRGSU5TeEtLUkJ6UzA9In1dLCJtaW50IjoiaHR0cHM6Ly9sZWdlbmQubG5iaXRzLmNvbS9jYXNodS9hcGkvdjEvU0t2SFJ1czlkbWpXSGhzdEhyc2F6VyJ9XX0=";
    let prom = wallet.receive(token).await.unwrap();
    let send = wallet.send(Amount::from_sat(2), prom).await.unwrap();

    println!("{:?}", send);
    panic!()
}

#[ignore]
#[tokio::test]
async fn test_get_mint_info() {
    let url = Url::from_str(MINTURL).unwrap();
    let mint = CashuMint::new(url);
    let _mint_info = mint.get_info().await.unwrap();

    // println!("{:?}", mint_info);
}
