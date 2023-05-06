// #![deny(unused)]

use std::str::FromStr;
use std::thread;
use std::time::Duration;

use bitcoin::Amount;
use cashu_crab::cashu_wallet::CashuWallet;
use cashu_crab::client::Client;
use cashu_crab::types::{MintKeys, MintProofs, Proofs, Token};
use lightning_invoice::Invoice;

#[tokio::main]
async fn main() {
    let url = "https://legend.lnbits.com/cashu/api/v1/MWMnRmbewX92AHjcL55mRo/";
    let client = Client::new(url).unwrap();

    // NUT-09
    // test_get_mint_info(&mint).await;

    let keys = test_get_mint_keys(&client).await;
    let wallet = CashuWallet::new(client.to_owned(), keys);
    test_get_mint_keysets(&client).await;
    test_request_mint(&wallet).await;
    let proofs = test_mint(&wallet).await;
    let token = Token::new(
        client.mint_url.clone(),
        proofs,
        Some("Hello World".to_string()),
    );
    let new_token = test_receive(&wallet, &token.to_string()).await;

    let proofs = Token::from_str(&new_token).unwrap().token[0].clone().proofs;
    test_send(&wallet, proofs.clone()).await;

    let spendable = test_check_spendable(&client, &new_token).await;

    let invoice = Invoice::from_str("lnbc20n1pjy3tp8pp5mmrp5vhzrmsz4d6sew77aw0wr7dfxptumxvstsl8peu8ypjhdmwsdq5g9kxy7fqd9h8vmmfvdjscqzpgxqyz5vqsp5aajwlqyxwwtk57tnxzgf9rk6mp0u3z33ksylqj6lu7et7dvlkdvs9qyyssq02rdva0hvamlgvfau0mqnknglk02v6d6x56xh5s8dtx9crtdrwf9hf6f87kk2n7tt0fsjg4xsyd50rqayxln5p9ygvetqtyrrtvy5ygpcjjwek").unwrap();
    test_melt(&wallet, invoice, spendable).await;

    test_check_fees(&client).await;
}

async fn test_get_mint_keys(client: &Client) -> MintKeys {
    let mint_keys = client.get_keys().await.unwrap();
    // println!("{:?}", mint_keys.0.capacity());
    assert!(mint_keys.0.capacity() > 1);

    mint_keys
}

async fn test_get_mint_keysets(client: &Client) {
    let mint_keysets = client.get_keysets().await.unwrap();

    assert!(!mint_keysets.keysets.is_empty())
}

async fn test_request_mint(wallet: &CashuWallet) {
    let mint = wallet.request_mint(Amount::from_sat(5)).await.unwrap();

    assert!(mint.pr.check_signature().is_ok())
}

async fn test_mint(wallet: &CashuWallet) -> Proofs {
    let mint_req = wallet.request_mint(Amount::from_sat(5)).await.unwrap();
    println!("Mint Req: {:?}", mint_req.pr.to_string());

    // Since before the mint happens the invoice in the mint req has to be paid this wait is here
    // probally some way to simulate this in a better way
    // but for now pay it quick
    thread::sleep(Duration::from_secs(30));

    wallet
        .mint_token(Amount::from_sat(5), &mint_req.hash)
        .await
        .unwrap()

    // println!("Mint: {:?}", mint_res.to_string());
}

async fn test_check_fees(mint: &Client) {
    let invoice = Invoice::from_str("lnbc10n1p3a6s0dsp5n55r506t2fv4r0mjcg30v569nk2u9s40ur4v3r3mgtscjvkvnrqqpp5lzfv8fmjzduelk74y9rsrxrayvhyzcdsh3zkdgv0g50napzalvqsdqhf9h8vmmfvdjn5gp58qengdqxq8p3aaymdcqpjrzjqwryaup9lh50kkranzgcdnn2fgvx390wgj5jd07rwr3vxeje0glc7z70cgqqg4sqqqqqqqlgqqqqrucqjq9qyysgqrjky5axsldzhqsjwsc38xa37k6t04le3ws4t26nqej62vst5xkz56qw85r6c4a3tr79588e0ceuuahwgfnkqc6n6269unlwqtvwr5vqqy0ncdq").unwrap();

    let _fee = mint.check_fees(invoice).await.unwrap();
    // println!("{fee:?}");
}

async fn test_receive(wallet: &CashuWallet, token: &str) -> String {
    let prom = wallet.receive(token).await.unwrap();
    println!("{:?}", prom);
    let token = MintProofs {
        mint: wallet.client.mint_url.clone(),
        proofs: prom,
    };

    let token = Token {
        token: vec![token],
        memo: Some("Hello world".to_string()),
    };

    let s = token.to_string();
    println!("{s}");
    s
}

async fn test_check_spendable(client: &Client, token: &str) -> Proofs {
    let mint_keys = client.get_keys().await.unwrap();

    let wallet = CashuWallet::new(client.to_owned(), mint_keys);

    let token_data = Token::from_str(token).unwrap();
    let spendable = wallet
        .check_proofs_spent(token_data.token[0].clone().proofs)
        .await
        .unwrap();

    assert!(!spendable.spendable.is_empty());
    // println!("Spendable: {:?}", spendable);

    spendable.spendable
}

async fn test_send(wallet: &CashuWallet, proofs: Proofs) {
    let send = wallet.send(Amount::from_sat(2), proofs).await.unwrap();

    println!("{:?}", send);

    let keep_token = wallet.proofs_to_token(send.change_proofs, Some("Keeping these".to_string()));

    let send_token = wallet.proofs_to_token(send.send_proofs, Some("Sending these".to_string()));

    println!("Keep Token: {keep_token}");
    println!("Send Token: {send_token}");
}

async fn test_melt(wallet: &CashuWallet, invoice: Invoice, proofs: Proofs) {
    let res = wallet.melt(invoice, proofs).await.unwrap();

    println!("{:?}", res);
}

async fn _test_get_mint_info(mint: &Client) {
    let _mint_info = mint.get_info().await.unwrap();

    // println!("{:?}", mint_info);
}
