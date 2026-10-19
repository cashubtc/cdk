use std::str::FromStr;

use anyhow::{anyhow, Result};
use cdk::mint_url::MintUrl;
use cdk::nuts::{Conditions, CurrencyUnit, PublicKey, SpendingConditions};
use cdk::wallet::types::SendKind;
use cdk::wallet::{SendMemo, SendOptions, WalletRepository};
use cdk::Amount;
use clap::Args;

use crate::utils::{get_number_input, get_or_create_wallet};

#[derive(Args)]
pub struct SendSubCommand {
    /// Token Memo
    #[arg(short, long)]
    memo: Option<String>,
    /// Preimage
    #[arg(long, conflicts_with = "hash")]
    preimage: Option<String>,
    /// Hash for HTLC (alternative to preimage)
    #[arg(long, conflicts_with = "preimage")]
    hash: Option<String>,
    /// Required number of signatures
    #[arg(long)]
    required_sigs: Option<u64>,
    /// Locktime before refund keys can be used
    #[arg(short, long)]
    locktime: Option<u64>,
    /// Pubkey to lock proofs to
    #[arg(short, long, action = clap::ArgAction::Append)]
    pubkey: Vec<String>,
    /// Refund keys that can be used after locktime
    #[arg(long, action = clap::ArgAction::Append)]
    refund_keys: Vec<String>,
    /// Token as V3 token
    #[arg(short, long)]
    v3: bool,
    /// Should the send be offline only
    #[arg(short, long)]
    offline: bool,
    /// Include fee to redeem in token
    #[arg(short, long)]
    include_fee: bool,
    /// Amount willing to overpay to avoid a swap
    #[arg(short, long)]
    tolerance: Option<u64>,
    /// Mint URL to use for sending
    #[arg(long)]
    mint_url: Option<String>,
    /// Use P2BK (NUT-28) to blind the receiver's pubkey
    #[arg(long)]
    use_p2bk: bool,
    /// Amount to send
    #[arg(short, long)]
    amount: Option<u64>,
    /// Display the token as an animated QR code (NUT-16)
    ///
    /// Loops QR frames until q or Ctrl+C is pressed; if the token fits a
    /// single frame, one static QR is shown instead.
    #[arg(long)]
    animate: bool,
    /// Number of equal-value tokens to create
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u64).range(1..))]
    count: u64,
}

pub async fn send(
    wallet_repository: &WalletRepository,
    sub_command_args: &SendSubCommand,
    unit: &CurrencyUnit,
) -> Result<()> {
    // Determine which mint to use for sending BEFORE asking for amount
    let selected_mint = if let Some(mint_url) = &sub_command_args.mint_url {
        MintUrl::from_str(mint_url)?
    } else {
        // Get all mints with their balances for the selected unit
        let balances_map = wallet_repository.get_balances_for_unit(unit).await?;
        if balances_map.is_empty() {
            return Err(anyhow!("No mints available in the wallet"));
        }

        let balances_vec: Vec<_> = balances_map.into_iter().collect();

        // If only one mint exists, automatically select it
        if balances_vec.len() == 1 {
            balances_vec[0].0.mint_url.clone()
        } else {
            // Display all mints with their balances and let user select
            println!("\nAvailable mints and balances:");
            for (index, (key, balance)) in balances_vec.iter().enumerate() {
                println!(
                    "  {}: {} ({}) - {} {}",
                    index, key.mint_url, key.unit, balance, unit
                );
            }

            loop {
                let selection: usize = get_number_input("Enter mint number to send from")?;

                if let Some((key, _)) = balances_vec.get(selection) {
                    break key.mint_url.clone();
                }

                println!("Invalid selection, please try again.");
            }
        }
    };

    let token_amount = match sub_command_args.amount {
        Some(amount) => Amount::from(amount),
        None => Amount::from(get_number_input::<u64>(&format!(
            "Enter value of token in {}",
            unit
        ))?),
    };

    // Get or create wallet for the selected mint
    let wallet = get_or_create_wallet(wallet_repository, &selected_mint, unit).await?;

    // Check wallet balance
    let balance = wallet.total_balance().await?;
    let total_amount = token_amount
        .checked_mul(Amount::from(sub_command_args.count))
        .ok_or(anyhow!("Total send amount overflow"))?;
    if balance < total_amount {
        return Err(anyhow!(
            "Insufficient funds. Wallet balance: {}, Required: {}",
            balance,
            total_amount
        ));
    }

    let conditions = match (&sub_command_args.preimage, &sub_command_args.hash) {
        (Some(_), Some(_)) => {
            // This case shouldn't be reached due to Clap's conflicts_with attribute
            unreachable!("Both preimage and hash were provided despite conflicts_with attribute")
        }
        (Some(preimage), None) => {
            let pubkeys = match sub_command_args.pubkey.is_empty() {
                true => None,
                false => Some(
                    sub_command_args
                        .pubkey
                        .iter()
                        .map(|p| PublicKey::from_str(p))
                        .collect::<Result<Vec<_>, _>>()?,
                ),
            };

            let refund_keys = match sub_command_args.refund_keys.is_empty() {
                true => None,
                false => Some(
                    sub_command_args
                        .refund_keys
                        .iter()
                        .map(|p| PublicKey::from_str(p))
                        .collect::<Result<Vec<_>, _>>()?,
                ),
            };

            let conditions = Conditions::new(
                sub_command_args.locktime,
                pubkeys,
                refund_keys,
                sub_command_args.required_sigs,
                None,
                None,
            )?;

            Some(SpendingConditions::new_htlc(
                preimage.clone(),
                Some(conditions),
            )?)
        }
        (None, Some(hash)) => {
            let pubkeys = match sub_command_args.pubkey.is_empty() {
                true => None,
                false => Some(
                    sub_command_args
                        .pubkey
                        .iter()
                        .map(|p| PublicKey::from_str(p))
                        .collect::<Result<Vec<_>, _>>()?,
                ),
            };

            let refund_keys = match sub_command_args.refund_keys.is_empty() {
                true => None,
                false => Some(
                    sub_command_args
                        .refund_keys
                        .iter()
                        .map(|p| PublicKey::from_str(p))
                        .collect::<Result<Vec<_>, _>>()?,
                ),
            };

            let conditions = Conditions::new(
                sub_command_args.locktime,
                pubkeys,
                refund_keys,
                sub_command_args.required_sigs,
                None,
                None,
            )?;

            Some(SpendingConditions::new_htlc_hash(hash, Some(conditions))?)
        }
        (None, None) => match sub_command_args.pubkey.is_empty() {
            true => None,
            false => {
                let pubkeys: Vec<PublicKey> = sub_command_args
                    .pubkey
                    .iter()
                    .map(|p| PublicKey::from_str(p))
                    .collect::<Result<Vec<_>, _>>()?;

                let refund_keys: Vec<PublicKey> = sub_command_args
                    .refund_keys
                    .iter()
                    .map(|p| PublicKey::from_str(p))
                    .collect::<Result<Vec<_>, _>>()?;

                let refund_keys = (!refund_keys.is_empty()).then_some(refund_keys);

                let data_pubkey = pubkeys[0];
                let pubkeys = pubkeys[1..].to_vec();
                let pubkeys = (!pubkeys.is_empty()).then_some(pubkeys);

                let conditions = Conditions::new(
                    sub_command_args.locktime,
                    pubkeys,
                    refund_keys,
                    sub_command_args.required_sigs,
                    None,
                    None,
                )?;

                Some(SpendingConditions::P2PKConditions {
                    data: data_pubkey,
                    conditions: Some(conditions),
                })
            }
        },
    };

    let send_kind = match (sub_command_args.offline, sub_command_args.tolerance) {
        (true, Some(amount)) => SendKind::OfflineTolerance(Amount::from(amount)),
        (true, None) => SendKind::OfflineExact,
        (false, Some(amount)) => SendKind::OnlineTolerance(Amount::from(amount)),
        (false, None) => SendKind::OnlineExact,
    };

    let send_options = SendOptions {
        memo: sub_command_args.memo.clone().map(|memo| SendMemo {
            memo,
            include_memo: true,
        }),
        send_kind,
        include_fee: sub_command_args.include_fee,
        conditions,
        use_p2bk: sub_command_args.use_p2bk,
        ..Default::default()
    };

    for _ in 0..sub_command_args.count {
        let prepared = wallet
            .prepare_send(token_amount, send_options.clone())
            .await?;
        let token = prepared.confirm(send_options.memo.clone()).await?;

        match sub_command_args.v3 {
            true => {
                println!("{}", token.to_v3_string());
            }
            false => {
                println!("{token}");
            }
        }
        if sub_command_args.animate {
            display_animated_qr(&token).await?;
        }
    }

    Ok(())
}

#[cfg(all(test, feature = "federation"))]
mod federation_tests {
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use std::time::Duration;

    use cdk::nuts::{Proof, SecretKey, State};
    use cdk::secret::Secret;
    use cdk::wallet::{WalletConfig, WalletRepositoryBuilder};
    use cdk_common::database::WalletDatabase;
    use cdk_common::wallet::ProofInfo;

    use super::*;
    use crate::test_support::{
        federated_connector_with_faults, member_id, test_bls_signature, MemberFault,
    };

    #[tokio::test]
    async fn command_send_waits_for_valid_threshold_after_invalid_share() {
        let fixture = federated_connector_with_faults([
            MemberFault::SwapShare {
                signer_id: member_id(1),
                delay: Duration::from_millis(20),
            },
            MemberFault::SwapShare {
                signer_id: member_id(2),
                delay: Duration::ZERO,
            },
            MemberFault::SwapShare {
                signer_id: member_id(1),
                delay: Duration::ZERO,
            },
        ]);
        let localstore: Arc<dyn WalletDatabase<cdk_common::database::Error> + Send + Sync> =
            Arc::new(
                cdk_sqlite::wallet::memory::empty()
                    .await
                    .expect("wallet database"),
            );
        let repository = WalletRepositoryBuilder::new()
            .localstore(localstore.clone())
            .seed([0u8; 64])
            .build()
            .await
            .expect("wallet repository");
        repository
            .set_mint_config(
                fixture.mint_url.clone(),
                CurrencyUnit::Sat,
                WalletConfig::new().with_mint_connector(fixture.connector),
            )
            .await
            .expect("federation wallet");
        let proof = Proof::new(
            Amount::from(2),
            fixture.keyset_id,
            Secret::generate(),
            test_bls_signature(b"cli-invalid-share-input"),
        );
        localstore
            .update_proofs(
                vec![ProofInfo::new(
                    proof,
                    fixture.mint_url.clone(),
                    State::Unspent,
                    CurrencyUnit::Sat,
                )
                .expect("proof info")],
                Vec::new(),
            )
            .await
            .expect("store proof");
        let receiver_pubkey = SecretKey::generate().public_key().to_hex();

        send(
            &repository,
            &SendSubCommand {
                animate: false,
                memo: None,
                preimage: None,
                hash: None,
                required_sigs: None,
                locktime: None,
                pubkey: vec![receiver_pubkey],
                refund_keys: Vec::new(),
                v3: false,
                offline: false,
                include_fee: false,
                tolerance: None,
                mint_url: Some(fixture.mint_url.to_string()),
                use_p2bk: false,
                amount: Some(1),
                count: 1,
            },
            &CurrencyUnit::Sat,
        )
        .await
        .expect("valid threshold should ignore one invalid share");

        assert_eq!(fixture.invalid_share_calls.load(Ordering::Relaxed), 1);
        let wallet = repository
            .get_wallet(&fixture.mint_url, &CurrencyUnit::Sat)
            .await
            .expect("federation wallet");
        assert_eq!(
            wallet.total_balance().await.expect("wallet balance"),
            1.into()
        );
    }
}

/// Display a token as an animated QR code (NUT-16)
///
/// Each UR fragment is rendered as one terminal QR frame. Multi-part tokens
/// loop frames every 250 ms until Ctrl+C; a token that fits a single frame
/// is rendered once as a static QR.
async fn display_animated_qr(token: &cdk::nuts::Token) -> Result<()> {
    const TERMINAL_MAX_FRAGMENT_LENGTH: usize = 100;

    use std::io;
    use std::time::Duration;

    use qrcode::render::unicode;
    use qrcode::QrCode;

    // Terminal cells cannot render narrower than one column per QR module.
    // Smaller UR fragments therefore produce smaller, easier-to-scan frames.
    let mut encoder = token.ur_encoder(TERMINAL_MAX_FRAGMENT_LENGTH)?;

    let render_frame = |part: &str| -> Result<String> {
        let qr_payload = part.to_ascii_uppercase();
        let code = QrCode::new(qr_payload.as_bytes())?;
        Ok(code
            .render::<unicode::Dense1x2>()
            .dark_color(unicode::Dense1x2::Light)
            .light_color(unicode::Dense1x2::Dark)
            .build())
    };

    if encoder.is_single_fragment() {
        println!("{}", render_frame(&encoder.next_part()?)?);
        return Ok(());
    }

    println!(
        "Displaying {} QR frames in a loop. Press q or Ctrl+C to stop.",
        encoder.fragment_count()
    );

    // Raw mode reads single keypresses without Enter; note it also turns
    // Ctrl+C into a key event instead of a signal
    crossterm::terminal::enable_raw_mode()?;

    let result = tokio::select! {
        result = async {
            loop {
                let part = encoder.next_part()?;
                let frame = render_frame(&part)?;
                let mut stdout = io::stdout();
                draw_animated_qr_frame(&mut stdout, &frame)?;
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            #[allow(unreachable_code)]
            Ok::<(), anyhow::Error>(())
        } => result,
        _ = quit_key_pressed() => Ok(()),
        _ = tokio::signal::ctrl_c() => Ok(()),
    };

    crossterm::terminal::disable_raw_mode()?;
    println!("\nStopped QR animation.");
    result
}

/// Draws a complete QR frame while the terminal is in raw mode.
fn draw_animated_qr_frame(stdout: &mut impl std::io::Write, frame: &str) -> std::io::Result<()> {
    use crossterm::SynchronizedUpdate;

    // Raw mode disables the terminal's LF-to-CRLF translation. Add carriage
    // returns explicitly so each rendered QR row starts in the first column.
    let frame = frame.replace('\n', "\r\n");

    // Synchronized updates prevent the terminal from displaying a partially
    // drawn frame, which would make a changing QR code briefly unscannable.
    stdout.sync_update(|stdout| {
        crossterm::queue!(
            stdout,
            crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
            crossterm::cursor::MoveTo(0, 0),
            crossterm::style::Print(&frame)
        )
    })?
}

/// Waits until q (or Ctrl+C, delivered as a key event in raw mode) is pressed
async fn quit_key_pressed() {
    use crossterm::event::{Event, EventStream, KeyCode, KeyModifiers};
    use futures::StreamExt;

    let mut events = EventStream::new();
    while let Some(Ok(event)) = events.next().await {
        let quit = matches!(event,
            Event::Key(key)
                if matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q'))
                    || (key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)));
        if quit {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::draw_animated_qr_frame;

    #[test]
    fn animated_qr_rows_return_to_the_first_column() {
        let mut output = Vec::new();

        draw_animated_qr_frame(&mut output, "first row\nsecond row")
            .expect("terminal frame should render");

        let output = String::from_utf8(output).expect("terminal output should be UTF-8");
        assert!(output.contains("first row\r\nsecond row"));
        assert!(!output.contains("first row\nsecond row"));
    }
}
