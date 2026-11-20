//! Adapt concrete URIs to the dependency's already-resolved instruction parser.

use bitcoin_payment_instructions::amount::Amount;
use bitcoin_payment_instructions::hrn_resolution::{
    DummyHrnResolver, HrnResolution, HrnResolutionFuture, HrnResolver, HumanReadableName,
    LNURLResolutionFuture,
};
use bitcoin_payment_instructions::{ParseError, PaymentInstructions};

struct ConcreteInstruction<'a>(&'a str);

impl HrnResolver for ConcreteInstruction<'_> {
    fn resolve_hrn<'a>(&'a self, _: &'a HumanReadableName) -> HrnResolutionFuture<'a> {
        Box::pin(async {
            Ok(HrnResolution::DNSSEC {
                proof: None,
                result: self.0.to_owned(),
            })
        })
    }

    fn resolve_lnurl<'a>(&'a self, url: &'a str) -> HrnResolutionFuture<'a> {
        DummyHrnResolver.resolve_lnurl(url)
    }

    fn resolve_lnurl_to_invoice<'a>(
        &'a self,
        url: String,
        amount: Amount,
        hash: [u8; 32],
    ) -> LNURLResolutionFuture<'a> {
        DummyHrnResolver.resolve_lnurl_to_invoice(url, amount, hash)
    }
}

pub(super) async fn parse(
    instruction: &str,
    network: bitcoin::Network,
) -> Result<PaymentInstructions, ParseError> {
    // In bitcoin-payment-instructions 0.7.1, scheme detection compares the part
    // before ':' with "bitcoin:", allowing LNURL sniffing to override a URI.
    // Its HRN result path parses concrete instructions without that sniffing.
    // Use a local adapter until the dependency exposes this parser directly.
    // This does no DNS resolution or authentication: BIP353 callers must still
    // validate DNSSEC before handing us the URI. No parameters are rewritten.
    // The placeholder name is internal and discarded during CDK conversion.
    PaymentInstructions::parse(
        "concrete@invalid",
        network,
        &ConcreteInstruction(instruction),
        false,
    )
    .await
}
