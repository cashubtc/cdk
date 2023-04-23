use std::str::FromStr;
use std::thread;
use std::time::Duration;

use bitcoin::Amount;
use lightning_invoice::Invoice;
use url::Url;

use cashu_rs::{cashu_mint::CashuMint, types::BlindedMessages};

const MINTURL: &str = "https://legend.lnbits.com/cashu/api/v1/SKvHRus9dmjWHhstHrsazW/";

#[tokio::test]
async fn test_get_mint_keys() {
    let url = Url::from_str(MINTURL).unwrap();
    let mint = CashuMint::new(url);
    let mint_keys = mint.get_keys().await.unwrap();
    // println!("{:?}", mint_keys.0.capacity());
    assert!(mint_keys.0.capacity() > 1);
}

#[tokio::test]
async fn test_get_mint_keysets() {
    let url = Url::from_str(MINTURL).unwrap();
    let mint = CashuMint::new(url);
    let mint_keysets = mint.get_keysets().await.unwrap();

    assert!(!mint_keysets.keysets.is_empty())
}

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
    thread::sleep(Duration::from_secs(10));

    let blinded_messages = BlindedMessages::random(Amount::from_sat(21)).unwrap();
    let mint_res = mint.mint(blinded_messages, &mint_req.hash).await.unwrap();

    println!("Mint: {:?}", mint_res);
}

#[tokio::test]
async fn test_check_fees() {
    let invoice = Invoice::from_str("lnbc10n1p3a6s0dsp5n55r506t2fv4r0mjcg30v569nk2u9s40ur4v3r3mgtscjvkvnrqqpp5lzfv8fmjzduelk74y9rsrxrayvhyzcdsh3zkdgv0g50napzalvqsdqhf9h8vmmfvdjn5gp58qengdqxq8p3aaymdcqpjrzjqwryaup9lh50kkranzgcdnn2fgvx390wgj5jd07rwr3vxeje0glc7z70cgqqg4sqqqqqqqlgqqqqrucqjq9qyysgqrjky5axsldzhqsjwsc38xa37k6t04le3ws4t26nqej62vst5xkz56qw85r6c4a3tr79588e0ceuuahwgfnkqc6n6269unlwqtvwr5vqqy0ncdq").unwrap();

    let url = Url::from_str(MINTURL).unwrap();
    let mint = CashuMint::new(url);

    let fee = mint.check_fees(invoice).await.unwrap();
    println!("{fee:?}");
}

#[ignore]
#[tokio::test]
async fn test_get_mint_info() {
    let url = Url::from_str(MINTURL).unwrap();
    let mint = CashuMint::new(url);
    let _mint_info = mint.get_info().await.unwrap();

    // println!("{:?}", mint_info);
}
