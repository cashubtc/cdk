use anyhow::Result;
use cdk_integration_tests::init_regtest::{
    fund_ln, init_bitcoin_client, init_bitcoind, init_cln, init_cln_client, init_lnd,
    init_lnd_client, open_channel, start_cln_mint,
};

#[tokio::main]
async fn main() -> Result<()> {
    let mut bitcoind = init_bitcoind();
    bitcoind.start_bitcoind()?;

    let bitcoin_client = init_bitcoin_client()?;
    bitcoin_client.create_wallet().ok();
    bitcoin_client.load_wallet()?;

    let new_add = bitcoin_client.get_new_address()?;
    bitcoin_client.generate_blocks(&new_add, 200).unwrap();

    let mut clnd = init_cln();
    clnd.start_clnd()?;

    let cln_client = init_cln_client().await?;

    let mut lnd = init_lnd().await;
    lnd.start_lnd().unwrap();

    let lnd_client = init_lnd_client().await.unwrap();

    fund_ln(&bitcoin_client, &cln_client, &lnd_client)
        .await
        .unwrap();

    open_channel(&bitcoin_client, &cln_client, &lnd_client)
        .await
        .unwrap();

    start_cln_mint().await?;

    Ok(())
}
