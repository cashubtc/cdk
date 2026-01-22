use anyhow::Result;
use tonic::transport::Channel;
use tonic::Request;

use crate::cdk_mint_reporting_client::CdkMintReportingClient;
use crate::GetInfoRequest;

/// Executes the get_info command against the mint server
///
/// This function sends an RPC request to retrieve public information about the mint,
/// including its name, version, descriptions, message of the day, icon URL, terms of
/// service URL, mint URLs, and contact information. The information is displayed in
/// a formatted output.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
pub async fn get_info(client: &mut CdkMintReportingClient<Channel>) -> Result<()> {
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
    println!(
        "motd:             {}",
        info.motd.unwrap_or("None".to_string())
    );
    println!(
        "icon_url:         {}",
        info.icon_url.unwrap_or("None".to_string())
    );
    println!(
        "tos_url:          {}",
        info.tos_url.unwrap_or("None".to_string())
    );
    for url in info.urls {
        println!("mint_url:         {url}");
    }
    for contact in info.contact {
        println!("contact:          method: {}", contact.method);
        println!("                  info:  {}", contact.info);
    }

    Ok(())
}
