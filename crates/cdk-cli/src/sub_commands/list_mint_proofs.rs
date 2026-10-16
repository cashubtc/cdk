use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::nuts::{CurrencyUnit, Proof};
use cdk::wallet::WalletRepository;
use clap::Args;

use crate::terminal::escape_control;

const REDACTED_SECRET: &str = "<redacted>";

#[derive(Debug, Args)]
pub struct ListMintProofsSubCommand {
    /// Print proof secrets. Treat the output as sensitive export material.
    #[arg(long)]
    show_secrets: bool,
}

pub async fn proofs(
    wallet_repository: &WalletRepository,
    sub_command_args: &ListMintProofsSubCommand,
) -> Result<()> {
    list_proofs(wallet_repository, sub_command_args.show_secrets).await?;
    Ok(())
}

async fn list_proofs(
    wallet_repository: &WalletRepository,
    show_secrets: bool,
) -> Result<Vec<(MintUrl, (Vec<Proof>, CurrencyUnit))>> {
    let mut proofs_vec = Vec::new();

    let wallets = wallet_repository.get_wallets().await;

    for (i, wallet) in wallets.iter().enumerate() {
        let mint_url = wallet.mint_url.clone();
        println!("{i}: {}", escape_control(&mint_url.to_string()));
        println!("|   Amount | Unit | State    | Secret                                                           | DLEQ proof included");
        println!("|----------|------|----------|------------------------------------------------------------------|--------------------");

        // Unspent proofs
        let unspent_proofs = wallet.get_unspent_proofs().await?;
        for proof in unspent_proofs.iter() {
            println!(
                "| {:8} | {:4} | {:8} | {:64} | {}",
                proof.amount,
                escape_control(&wallet.unit.to_string()),
                "unspent",
                render_secret(&proof.secret.to_string(), show_secrets),
                proof.dleq.is_some()
            );
        }

        // Pending proofs
        let pending_proofs = wallet.get_pending_proofs().await?;
        for proof in pending_proofs {
            println!(
                "| {:8} | {:4} | {:8} | {:64} | {}",
                proof.amount,
                escape_control(&wallet.unit.to_string()),
                "pending",
                render_secret(&proof.secret.to_string(), show_secrets),
                proof.dleq.is_some()
            );
        }

        // Reserved proofs
        let reserved_proofs = wallet.get_reserved_proofs().await?;
        for proof in reserved_proofs {
            println!(
                "| {:8} | {:4} | {:8} | {:64} | {}",
                proof.amount,
                escape_control(&wallet.unit.to_string()),
                "reserved",
                render_secret(&proof.secret.to_string(), show_secrets),
                proof.dleq.is_some()
            );
        }

        println!();
        proofs_vec.push((mint_url, (unspent_proofs, wallet.unit.clone())));
    }
    Ok(proofs_vec)
}

fn render_secret(secret: &str, show_secrets: bool) -> String {
    if show_secrets {
        escape_control(secret)
    } else {
        REDACTED_SECRET.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_redacted_by_default() {
        assert_eq!(render_secret("sensitive-secret", false), REDACTED_SECRET);
    }

    #[test]
    fn explicitly_shown_secrets_are_terminal_safe() {
        assert_eq!(
            render_secret("secret\u{1b}]52;evil\u{07}", true),
            "secret\\e]52;evil\\a"
        );
    }
}
