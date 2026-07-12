use anyhow::Result;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::WalletRepository;
use cdk::Amount;

use crate::terminal::escape_control;

fn pending_proofs_status(pending_amount: Amount, unit: &CurrencyUnit) -> String {
    format!(
        "Checked pending proofs: {} {} still pending",
        pending_amount,
        escape_control(&unit.to_string())
    )
}

pub async fn check_pending(wallet_repository: &WalletRepository) -> Result<()> {
    let wallets = wallet_repository.get_wallets().await;

    for (i, wallet) in wallets.iter().enumerate() {
        let mint_url = wallet.mint_url.clone();
        println!("{i}: {}", escape_control(&mint_url.to_string()));

        // Check all orphaned pending proofs (not managed by active sagas)
        // This function queries the mint and marks spent proofs accordingly
        match wallet.check_all_pending_proofs().await {
            Ok(pending_amount) => {
                if pending_amount == Amount::ZERO {
                    println!("No orphaned pending proofs found");
                } else {
                    println!("{}", pending_proofs_status(pending_amount, &wallet.unit));
                }
            }
            Err(e) => println!(
                "Error checking pending proofs: {}",
                escape_control(&e.to_string())
            ),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_status_escapes_custom_currency_unit() {
        let unit = CurrencyUnit::custom("usd\u{1b}]52;c;clipboard\u{07}\r\u{85}\u{202e}");
        let output = pending_proofs_status(Amount::from(7), &unit);

        assert_eq!(
            output,
            "Checked pending proofs: 7 usd\\e]52;c;clipboard\\a\\r\\u{85}\\u{202e} still pending"
        );
        assert!(!output
            .chars()
            .any(|ch| { ch.is_control() || cdk_common::terminal::is_bidi_control_character(ch) }));
    }
}
