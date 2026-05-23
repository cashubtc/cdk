use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use bdk_wallet::bitcoin::{Address, Network};

use crate::error::Error;

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn parse_checked_address<F>(
    address: &str,
    network: Network,
    map_error: F,
) -> Result<Address, Error>
where
    F: Fn(String) -> Error,
{
    Address::from_str(address)
        .map_err(|err| map_error(err.to_string()))?
        .require_network(network)
        .map_err(|err| map_error(err.to_string()))
}
