use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use anyhow::Result;
use cdk::mint_url::MintUrl;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct TokenData {
    pub mint_url: String,
    pub access_token: String,
    pub refresh_token: String,
}

/// Stores authentication tokens in the work directory
pub async fn save_tokens(
    work_dir: &Path,
    mint_url: &MintUrl,
    access_token: &str,
    refresh_token: &str,
) -> Result<()> {
    let token_data = TokenData {
        mint_url: mint_url.to_string(),
        access_token: access_token.to_string(),
        refresh_token: refresh_token.to_string(),
    };

    let json = serde_json::to_string_pretty(&token_data)?;
    let file_path = work_dir.join(format!(
        "auth_tokens_{}",
        mint_url.to_string().replace("/", "_")
    ));
    let mut file = File::create(file_path)?;
    file.write_all(json.as_bytes())?;

    Ok(())
}

/// Gets authentication tokens from the work directory
pub async fn get_token_for_mint(work_dir: &Path, mint_url: &MintUrl) -> Result<Option<TokenData>> {
    let file_path = work_dir.join(format!(
        "auth_tokens_{}",
        mint_url.to_string().replace("/", "_")
    ));

    if !file_path.exists() {
        return Ok(None);
    }

    let mut file = File::open(file_path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;

    let token_data: TokenData = serde_json::from_str(&contents)?;

    if token_data.mint_url == mint_url.to_string() {
        Ok(Some(token_data))
    } else {
        Ok(None)
    }
}
