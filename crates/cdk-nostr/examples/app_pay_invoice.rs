use std::{str::FromStr, time::Duration};

use lightning_invoice::Bolt11Invoice;
use nostr_database::{MemoryDatabase, MemoryDatabaseOptions};
use nostr_sdk::{
    nips::nip47::{self, NostrWalletConnectURI, PayInvoiceRequestParams},
    Alphabet, Filter, FilterOptions, Keys, Kind, SingleLetterTag,
};
use tokio_stream::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: {} <connect_uri> <invoice>", args[0]);
        std::process::exit(1);
    }
    let connect_uri = NostrWalletConnectURI::from_str(&args[1])?;
    let invoice = Bolt11Invoice::from_str(&args[2])?;

    let keys = Keys::new(connect_uri.secret.clone());
    let client = nostr_sdk::Client::builder()
        .database(MemoryDatabase::with_opts(MemoryDatabaseOptions {
            events: true,
            ..Default::default()
        }))
        .signer(&keys)
        .build();
    client.add_relay(&connect_uri.relay_url).await?;
    client.connect().await;

    let request = nip47::Request::pay_invoice(PayInvoiceRequestParams {
        id: None,
        invoice: invoice.to_string(),
        amount: None,
    });
    let event = request.to_event(&connect_uri)?;
    let event_id = event.id;

    let mut stream = client
        .pool()
        .stream_events_of(
            vec![Filter::new()
                .kind(Kind::WalletConnectResponse)
                .custom_tag(SingleLetterTag::lowercase(Alphabet::E), vec![event_id])],
            Duration::from_secs(60),
            FilterOptions::WaitForEventsAfterEOSE(1),
        )
        .await?;
    client.send_event(event).await?;

    if let Some(event) = stream.next().await {
        match nip47::Response::from_event(&connect_uri, &event) {
            Ok(response) => {
                println!("{:?}", response);
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
    }

    Ok(())
}
