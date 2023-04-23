use std::str::FromStr;

use url::Url;

use cashu_rs::cashu_mint::CashuMint;

const MINTURL: &str = "https://legend.lnbits.com/cashu/api/v1/SKvHRus9dmjWHhstHrsazW/";

#[ignore]
#[tokio::test]
async fn test_get_mint_info() {
    let url = Url::from_str(MINTURL).unwrap();
    let mint = CashuMint::new(url);
    let mint_info = mint.get_info().await.unwrap();
    // println!("{:?}", mint_info);
}

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
