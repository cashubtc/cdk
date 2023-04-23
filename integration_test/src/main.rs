// #![deny(unused)]

use std::str::FromStr;

use cashu_rs::cashu_mint::CashuMint;
use url::Url;

#[tokio::main]
async fn main() {
    let url = Url::from_str("https://legend.lnbits.com/cashu/api/v1/SKvHRus9dmjWHhstHrsazW/keys")
        .unwrap();
    let mint = CashuMint::new(url);

    // test_get_mint_info(&mint).await;

    test_get_mint_keys(&mint).await;
    test_get_mint_keysets(&mint).await;
}

async fn test_get_mint_info(mint: &CashuMint) {
    let mint_info = mint.get_info().await.unwrap();

    println!("{:?}", mint_info);
}

async fn test_get_mint_keys(mint: &CashuMint) {
    let mint_keys = mint.get_keys().await.unwrap();

    println!("{:?}", mint_keys);
}

async fn test_get_mint_keysets(mint: &CashuMint) {
    let mint_keysets = mint.get_keysets().await.unwrap();

    assert!(!mint_keysets.keysets.is_empty())
}
