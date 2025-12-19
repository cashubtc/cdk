//! Example: Decoding a token and getting proofs using MultiMintWallet
//!
//! This example demonstrates how to:
//! 1. Create a MultiMintWallet
//! 2. Decode a cashu token
//! 3. Use `get_token_data` to extract mint URL and proofs in one call
//! 4. Alternatively, get keysets manually and extract proofs

use std::str::FromStr;
use std::sync::Arc;

use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{CurrencyUnit, Token};
use cdk::wallet::MultiMintWallet;
use cdk_sqlite::wallet::memory;
use rand::random;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Generate a random seed for the wallet
    let seed = random::<[u8; 64]>();

    // Initialize the memory store
    let localstore = Arc::new(memory::empty().await?);

    // Create a new multi-mint wallet for satoshis
    let wallet = MultiMintWallet::new(localstore, seed, CurrencyUnit::Sat).await?;

    // Example: A cashu token string (in practice, this would come from user input)
    let token = Token::from_str("cashuBo2FteB1odHRwczovL2Zha2UudGhlc2ltcGxla2lkLmRldmF1Y3NhdGF0gaJhaUgAlNWndMQKMmFwg6RhYRkIAGFzeEAwYjk0ZjU5ZjU0OTBkNTkzMzI4ZTIwNDllZTNlZmFjYjM5NzljZjU5NzA5ZTM3N2U5YzBmMDQyNDBmZTUyZTVhYWNYIQNGQCYyf1j996pS-LuP_7VsUE-uzRpAm-K4rZiDEFFc1GFko2FlWCBbuMkhvz39ytCzm7xPaY5vdTbqxlxTzXOsks_8S3sf1GFzWCBg22l0CXH5-QLcfJtUJZ2lfylNfC6_o9FTfKClLzthaGFyWCCP2nJ6Qzd8mwLa_85cu8TrwRIprElVgrhqJeoHJwXmSKRhYRkCAGFzeEBhNmMyODliMjMwMTdlMDhjYTFhOTc4ZjAwNGRiNjI4ZDk1NWI5ZTlmNjMwMjY0MjNjZDc4OGExNDBhOWJiYjgxYWNYIQPMXkT68L8Y0a6royMbkoUTbvxOUgsyDwvRZRNTvwUsWWFko2FlWCCj9BFXexBOrlUyUiY_1qEIEHvd1YphWA2l3YhdFwVRh2FzWCBTNgyGeXvGSFtvYKj3MnJCXA8qjI9fzZHFsIw-F_OAGmFyWCDRHiDbVysUuQZucifYx5zMvOKyVIz7zvcJcfd01FoI3KRhYQhhc3hAMWJjOWQ1MjE5ZTZhYzNjZmZhNTM0NTRkY2JjMzE1YzZjZjY5MmM5MDEzYTUzYTA1YzIzN2YwZTBiOTViZTkwMWFjWCEDXd5sxFgxYgUHctpLENYStcr50UtJ4QRojy0g7mkdvWRhZKNhZVggZzSifCUG692E2sW4L6DT_FuKwLZdUFoMnds3tQyMlAdhc1ggtIo0BS2-6arws5fJx_w0phOiCZZcHIFknlrDXSh3C0NhclggM2dDF0kQyuRoOqrOOMHFrmNnvtGiXWxuvqtD7HidR8I")?;

    // Get the mint URL from the token
    let mint_url = token.mint_url()?;
    println!("Token mint URL: {}", mint_url);

    // Get token value
    let value = token.value()?;
    println!("Token value: {} sats", value);

    // Get token memo if present
    if let Some(memo) = token.memo() {
        println!("Token memo: {}", memo);
    }

    // Add the mint to our wallet so we can fetch keysets
    wallet.add_mint(mint_url.clone()).await?;

    // =========================================================================
    // Method 1: Use get_token_data() for a simple one-call approach
    // =========================================================================
    println!("\n--- Using get_token_data() ---");

    let token_data = wallet.get_token_data(&token).await?;
    println!("Mint URL: {}", token_data.mint_url);
    println!("Number of proofs: {}", token_data.proofs.len());

    for (i, proof) in token_data.proofs.iter().enumerate() {
        println!(
            "  Proof {}: {} sats, keyset: {}",
            i + 1,
            proof.amount,
            proof.keyset_id
        );
    }

    // =========================================================================
    // Method 2: Manual approach - get keysets first, then extract proofs
    // =========================================================================
    println!("\n--- Using manual keyset lookup ---");

    // Get the keysets for this mint
    let keysets = wallet.get_mint_keysets(&mint_url).await?;
    println!("Found {} keysets for mint", keysets.len());

    for keyset in &keysets {
        println!(
            "  - Keyset ID: {}, Unit: {:?}, Active: {}",
            keyset.id, keyset.unit, keyset.active
        );
    }

    // Extract proofs from the token using the keysets
    let proofs = token.proofs(&keysets)?;
    println!("\nToken contains {} proofs:", proofs.len());

    // Calculate total amount from proofs
    let total = proofs.total_amount()?;
    println!("Total amount from proofs: {} sats", total);

    // Verify total matches token value
    assert_eq!(total, value, "Proof total should match token value");

    println!("\nSuccessfully decoded token and extracted proofs!");

    Ok(())
}
