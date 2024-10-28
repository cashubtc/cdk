use std::str::FromStr;

use anyhow::Result;
use cdk::nuts::PaymentRequest;
use cdk::util::serialize_to_cbor_diag;
use clap::Args;

#[derive(Args)]
pub struct DecodePaymentRequestSubCommand {
    /// Payment request
    payment_request: String,
}

pub fn decode_payment_request(sub_command_args: &DecodePaymentRequestSubCommand) -> Result<()> {
    let payment_request = PaymentRequest::from_str(&sub_command_args.payment_request)?;

    println!("{:}", serialize_to_cbor_diag(&payment_request)?);
    Ok(())
}
