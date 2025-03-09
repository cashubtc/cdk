use std::fs;
use std::path::Path;

use anyhow::Result;
use cdk::nuts::PublicKey;
use cdk::util::hex;

/// Stores the last checked time for a nostr key in a file
pub async fn store_nostr_last_checked(
    work_dir: &Path,
    verifying_key: &PublicKey,
    last_checked: u32,
) -> Result<()> {
    let key_hex = hex::encode(verifying_key.to_bytes());
    let file_path = work_dir.join(format!("nostr_last_checked_{}", key_hex));

    fs::write(file_path, last_checked.to_string())?;

    Ok(())
}

/// Gets the last checked time for a nostr key from a file
pub async fn get_nostr_last_checked(
    work_dir: &Path,
    verifying_key: &PublicKey,
) -> Result<Option<u32>> {
    let key_hex = hex::encode(verifying_key.to_bytes());
    let file_path = work_dir.join(format!("nostr_last_checked_{}", key_hex));

    match fs::read_to_string(file_path) {
        Ok(content) => {
            let timestamp = content.trim().parse::<u32>()?;
            Ok(Some(timestamp))
        }
        Err(_) => Ok(None),
    }
}
