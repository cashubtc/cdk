// #![deny(unused)]

use std::str::FromStr;
use std::thread;
use std::time::Duration;

use bitcoin::Amount;
use cashu_crab::cashu_mint::CashuMint;
use cashu_crab::cashu_wallet::CashuWallet;
use cashu_crab::types::{BlindedMessages, MintKeys, ProofsStatus, Token, TokenData};
use lightning_invoice::Invoice;
use url::Url;

#[tokio::main]
async fn main() {
    let url =
        Url::from_str("https://legend.lnbits.com/cashu/api/v1/SKvHRus9dmjWHhstHrsazW/").unwrap();
    let mint = CashuMint::new(url);

    // NUT-09
    // test_get_mint_info(&mint).await;

    let keys = test_get_mint_keys(&mint).await;
    let wallet = CashuWallet::new(mint.to_owned(), keys);
    test_get_mint_keysets(&mint).await;
    test_request_mint(&wallet).await;
    let token = test_mint(&wallet).await;
    let new_token = test_receive(&wallet, &token).await;

    test_check_spendable(&mint, &new_token).await;

    test_check_fees(&mint).await;
}

async fn test_get_mint_keys(mint: &CashuMint) -> MintKeys {
    let mint_keys = mint.get_keys().await.unwrap();
    // println!("{:?}", mint_keys.0.capacity());
    assert!(mint_keys.0.capacity() > 1);

    mint_keys
}

async fn test_get_mint_keysets(mint: &CashuMint) {
    let mint_keysets = mint.get_keysets().await.unwrap();

    assert!(!mint_keysets.keysets.is_empty())
}

async fn test_request_mint(wallet: &CashuWallet) {
    let mint = wallet.request_mint(Amount::from_sat(21)).await.unwrap();

    assert!(mint.pr.check_signature().is_ok())
}

async fn test_mint(wallet: &CashuWallet) -> String {
    let mint_req = wallet.request_mint(Amount::from_sat(21)).await.unwrap();
    println!("Mint Req: {:?}", mint_req.pr.to_string());

    // Since before the mint happens the invoice in the mint req has to be paid this wait is here
    // probally some way to simulate this in a better way
    // but for now pay it quick
    thread::sleep(Duration::from_secs(30));

    let mint_res = wallet
        .mint_token(Amount::from_sat(21), &mint_req.hash)
        .await
        .unwrap();

    println!("Mint: {:?}", mint_res.to_string());

    mint_res.to_string()
}

async fn test_check_fees(mint: &CashuMint) {
    let invoice = Invoice::from_str("lnbc10n1p3a6s0dsp5n55r506t2fv4r0mjcg30v569nk2u9s40ur4v3r3mgtscjvkvnrqqpp5lzfv8fmjzduelk74y9rsrxrayvhyzcdsh3zkdgv0g50napzalvqsdqhf9h8vmmfvdjn5gp58qengdqxq8p3aaymdcqpjrzjqwryaup9lh50kkranzgcdnn2fgvx390wgj5jd07rwr3vxeje0glc7z70cgqqg4sqqqqqqqlgqqqqrucqjq9qyysgqrjky5axsldzhqsjwsc38xa37k6t04le3ws4t26nqej62vst5xkz56qw85r6c4a3tr79588e0ceuuahwgfnkqc6n6269unlwqtvwr5vqqy0ncdq").unwrap();

    let _fee = mint.check_fees(invoice).await.unwrap();
    // println!("{fee:?}");
}

async fn test_receive(wallet: &CashuWallet, token: &str) -> String {
    let prom = wallet.receive(token).await.unwrap();
    println!("{:?}", prom);
    let token = Token {
        mint: wallet.mint.url.clone(),
        proofs: prom,
    };

    let token = TokenData {
        token: vec![token],
        memo: Some("Hello world".to_string()),
    };

    let s = token.to_string();
    println!("{s}");
    s
}

async fn test_check_spendable(mint: &CashuMint, token: &str) {
    let mint_keys = mint.get_keys().await.unwrap();

    let wallet = CashuWallet::new(mint.to_owned(), mint_keys);

    let token_data = TokenData::from_str(token).unwrap();
    let _spendable = wallet
        .check_proofs_spent(token_data.token[0].clone().proofs)
        .await
        .unwrap();
    // println!("Spendable: {:?}", spendable);
}

async fn test_split(mint: &CashuMint, token: &str) {
    let mint_keys = mint.get_keys().await.unwrap();

    let wallet = CashuWallet::new(mint.clone(), mint_keys);
    let proofs = wallet.receive(token).await.unwrap();

    let split = wallet
        .create_split(Amount::ONE_SAT, Amount::ONE_SAT, proofs)
        .await
        .unwrap();

    println!("Split: {:#?}", split);
    println!(
        "splint JSON {:?}",
        serde_json::to_string(&split.split_payload)
    );

    let split = mint.split(split.split_payload).await;
    println!("Split res: {:#?}", split);
}

async fn test_send(mint: &CashuMint, token: &str) {
    let mint_keys = mint.get_keys().await.unwrap();

    let wallet = CashuWallet::new(mint.to_owned(), mint_keys);
    let prom = wallet.receive(token).await.unwrap();
    let send = wallet.send(Amount::from_sat(2), prom).await.unwrap();

    println!("{:?}", send);
}

async fn test_get_mint_info(mint: &CashuMint) {
    let _mint_info = mint.get_info().await.unwrap();

    // println!("{:?}", mint_info);
}
