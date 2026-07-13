use anyhow::Result;
use tonic::Request;

use crate::info::GetInfoRequest;
use crate::InterceptedMintInfoServiceClient;

/// Executes the get_info command against the mint server
///
/// This function fetches the mint's public metadata and prints it.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
pub async fn get_info(client: &mut InterceptedMintInfoServiceClient) -> Result<()> {
    let response = client.get_info(Request::new(GetInfoRequest {})).await?;
    let info = response.into_inner();

    println!(
        "name:             {}",
        info.name.unwrap_or("None".to_string())
    );
    println!(
        "version:          {}",
        info.version.unwrap_or("None".to_string())
    );
    println!(
        "description:      {}",
        info.description.unwrap_or("None".to_string())
    );
    println!(
        "long description: {}",
        info.long_description.unwrap_or("None".to_string())
    );
    println!("motd: {}", info.motd.unwrap_or("None".to_string()));
    println!("icon_url: {}", info.icon_url.unwrap_or("None".to_string()));
    println!("tos_url: {}", info.tos_url.unwrap_or("None".to_string()));

    for url in info.urls {
        println!("mint_url: {url}");
    }

    for contact in info.contact {
        println!("method: {}, info: {}", contact.method, contact.info);
    }

    Ok(())
}
