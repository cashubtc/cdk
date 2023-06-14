// #![deny(unused)]

use std::str::FromStr;
use std::thread;
use std::time::Duration;

use bitcoin::Amount;
use cashu_crab::cashu_wallet::CashuWallet;
use cashu_crab::client::Client;
use cashu_crab::keyset::Keys;
use cashu_crab::types::{Invoice, MintProofs, Proofs, Token};

const MINTURL: &str = "https://testnut.cashu.space";

const MINTAMOUNT: u64 = 21;
const SENDAMOUNT: u64 = 5;

const MELTINVOICE: &str = "lnbc10n1pj9d299pp5dwz8y2xuk3yrrfuayclwpffgnd3htfzmxvd8xqa4hzcp58kdeq2sdq5g9kxy7fqd9h8vmmfvdjscqzzsxqyz5vqsp5hdjrv0gx59qy7m4a9xk66m0kvja9ljjzh6zp6wn8np8kky5xnqks9qyyssqd09y339sp53l0apkrt8klmrrvl7qmxd2237p8m8cz8xcl725ems8al55hkjl6c2sjx755s7m8rlmhwej9u8glv2ltwzfssddzpwk05cp4re0t3";

#[tokio::main]
async fn main() {
    let client = Client::new(MINTURL).unwrap();

    // NUT-09
    test_get_mint_info(&client).await;

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
    let new_token = test_receive(&wallet, &token.convert_to_string().unwrap()).await;

    let _proofs = Token::from_str(&new_token).unwrap().token[0].clone().proofs;
    let spendable = test_check_spendable(&client, &new_token).await;
    let proofs = test_send(&wallet, spendable).await;

    test_melt(
        &wallet,
        Invoice::from_str(MELTINVOICE).unwrap(),
        proofs,
        // TODO:
        Amount::from_sat(10),
    )
    .await;

    test_check_fees(&client).await;
}

async fn test_get_mint_keys(client: &Client) -> Keys {
    let mint_keys = client.get_keys().await.unwrap();
    // println!("{:?}", mint_keys.0.capacity());
    assert!(mint_keys.as_hashmap().capacity() > 1);

    mint_keys
}

async fn test_get_mint_keysets(client: &Client) {
    let mint_keysets = client.get_keysets().await.unwrap();

    assert!(!mint_keysets.keysets.is_empty())
}

async fn test_request_mint(wallet: &CashuWallet) {
    let mint = wallet
        .request_mint(Amount::from_sat(MINTAMOUNT))
        .await
        .unwrap();

    assert!(mint.pr.check_signature().is_ok())
}

async fn test_mint(wallet: &CashuWallet) -> Proofs {
    let mint_req = wallet
        .request_mint(Amount::from_sat(MINTAMOUNT))
        .await
        .unwrap();
    println!("Mint Req: {:?}", mint_req.pr.to_string());

    // Since before the mint happens the invoice in the mint req has to be paid this wait is here
    // probally some way to simulate this in a better way
    // but for now pay it quick
    thread::sleep(Duration::from_secs(30));

    wallet
        .mint(Amount::from_sat(MINTAMOUNT), &mint_req.hash)
        .await
        .unwrap()

    // println!("Mint: {:?}", mint_res.to_string());
}

async fn test_check_fees(mint: &Client) {
    let invoice = Invoice::from_str(MELTINVOICE).unwrap();

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

    let s = token.convert_to_string();
    // println!("{s}");
    s.unwrap()
}

async fn test_check_spendable(client: &Client, token: &str) -> Proofs {
    let mint_keys = client.get_keys().await.unwrap();

    let wallet = CashuWallet::new(client.to_owned(), mint_keys);

    let token_data = Token::from_str(token).unwrap();
    let spendable = wallet
        .check_proofs_spent(&token_data.token[0].clone().proofs)
        .await
        .unwrap();

    println!("Spendable: {:?}", spendable);
    assert!(!spendable.spendable.is_empty());

    spendable.spendable
}

async fn test_send(wallet: &CashuWallet, proofs: Proofs) -> Proofs {
    let send = wallet
        .send(Amount::from_sat(SENDAMOUNT), proofs)
        .await
        .unwrap();

    println!("{:?}", send);

    let keep_token = wallet
        .proofs_to_token(send.change_proofs, Some("Keeping these".to_string()))
        .unwrap();

    let send_token = wallet
        .proofs_to_token(send.send_proofs.clone(), Some("Sending these".to_string()))
        .unwrap();

    println!("Keep Token: {keep_token}");
    println!("Send Token: {send_token}");

    send.send_proofs
}

async fn test_melt(wallet: &CashuWallet, invoice: Invoice, proofs: Proofs, fee_reserve: Amount) {
    let res = wallet.melt(invoice, proofs, fee_reserve).await.unwrap();

    println!("{:?}", res);
}

async fn test_get_mint_info(_mint: &Client) {
    // let mint_info = mint.get_info().await.unwrap();

    // println!("{:?}", mint_info);
}
