use anyhow::Result;
use cdk_common::terminal::escape_control;
use clap::{Args, ValueEnum};
use tonic::transport::Channel;
use tonic::Request;

use crate::keyset::keyset_service_client::KeysetServiceClient;
use crate::keyset::{KeysetVersion, RotateNextKeysetRequest};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliKeysetVersion {
    V1,
    V2,
}

impl From<CliKeysetVersion> for KeysetVersion {
    fn from(version: CliKeysetVersion) -> Self {
        match version {
            CliKeysetVersion::V1 => Self::V1,
            CliKeysetVersion::V2 => Self::V2,
        }
    }
}

/// Command to rotate to the next keyset for the mint
///
/// This command instructs the mint to rotate to a new keyset, which generates new keys
/// for signing tokens of the specified unit type.
#[derive(Args, Debug)]
pub struct RotateNextKeysetCommand {
    /// The unit type for the keyset (e.g., "sat")
    #[arg(short, long)]
    #[arg(default_value = "sat")]
    unit: String,
    /// The amounts that can be minted with this keyset (e.g., "1,2,4,8,16")
    #[arg(short, long)]
    amounts: Option<String>,
    /// The input fee in parts per thousand to apply when minting with this keyset
    #[arg(short, long)]
    input_fee_ppk: Option<u64>,
    /// Use keyset v2; cannot be combined with --keyset-version
    #[arg(long)]
    use_keyset_v2: Option<bool>,
    /// Keyset version to create; cannot be combined with --use-keyset-v2
    #[arg(long, value_enum, conflicts_with = "use_keyset_v2")]
    keyset_version: Option<CliKeysetVersion>,
    /// Final expiry unix timestamp for the keyset
    #[arg(long)]
    final_expiry: Option<u64>,
}

/// Executes the rotate_next_keyset command against the mint server
///
/// This function sends an RPC request to the mint to rotate to a new keyset with the
/// specified parameters and prints the resulting keyset information.
///
/// # Arguments
/// * `client` - The RPC client used to communicate with the mint
/// * `sub_command_args` - The arguments specifying how the new keyset should be configured
pub async fn rotate_next_keyset(
    client: &mut KeysetServiceClient<Channel>,
    sub_command_args: &RotateNextKeysetCommand,
) -> Result<()> {
    let response = client
        .rotate_next_keyset(Request::new(sub_command_args.to_request()?))
        .await?;

    let response = response.into_inner();

    println!(
        "Rotated to new keyset {} for unit {} with amounts {} and fee of {}",
        escape_control(&response.id),
        escape_control(&response.unit),
        serde_json::to_string(&response.amounts)?,
        response.input_fee_ppk
    );

    Ok(())
}

impl RotateNextKeysetCommand {
    fn to_request(&self) -> Result<RotateNextKeysetRequest> {
        let amounts = if let Some(amounts_str) = &self.amounts {
            amounts_str
                .split(',')
                .map(|s| s.trim().parse::<u64>())
                .collect::<Result<Vec<u64>, _>>()?
        } else {
            vec![]
        };

        let keyset_version = self.keyset_version.map(KeysetVersion::from);
        // Send the effective legacy selector too, so older servers honor the choice.
        let use_keyset_v2 = keyset_version
            .map(|version| version == KeysetVersion::V2)
            .or(self.use_keyset_v2);
        Ok(RotateNextKeysetRequest {
            unit: self.unit.clone(),
            amounts,
            input_fee_ppk: self.input_fee_ppk,
            use_keyset_v2,
            keyset_version: keyset_version.map(i32::from),
            final_expiry: self.final_expiry,
        })
    }
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, FromArgMatches, Parser};

    use super::*;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        rotation: RotateNextKeysetCommand,
    }

    #[test]
    fn version_selection_preserves_legacy_wire_behavior() {
        for (args, expected) in [
            (vec![], None),
            (vec!["--use-keyset-v2", "false"], Some(false)),
            (vec!["--use-keyset-v2", "true"], Some(true)),
            (vec!["--keyset-version", "v1"], Some(false)),
            (vec!["--keyset-version", "v2"], Some(true)),
        ] {
            let matches = Cli::command()
                .try_get_matches_from(std::iter::once("test").chain(args))
                .unwrap();
            let command = Cli::from_arg_matches(&matches).unwrap();
            let request = command.rotation.to_request().unwrap();
            assert_eq!(request.use_keyset_v2, expected);
            let expected_version = match command.rotation.keyset_version {
                Some(CliKeysetVersion::V1) => Some(KeysetVersion::V1 as i32),
                Some(CliKeysetVersion::V2) => Some(KeysetVersion::V2 as i32),
                None => None,
            };
            assert_eq!(request.keyset_version, expected_version);
        }
    }

    #[test]
    fn version_selectors_cannot_be_combined() {
        for version in ["v1", "v2"] {
            for legacy in ["true", "false"] {
                for args in [
                    ["--keyset-version", version, "--use-keyset-v2", legacy],
                    ["--use-keyset-v2", legacy, "--keyset-version", version],
                ] {
                    let error = Cli::try_parse_from(std::iter::once("test").chain(args))
                        .err()
                        .expect("combining version selectors must fail");
                    assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
                }
            }
        }
    }

    #[test]
    fn unsupported_versions_are_rejected_by_cli() {
        for version in ["00", "01", "v3", "02", "unknown", ""] {
            assert!(Cli::try_parse_from(["test", "--keyset-version", version]).is_err());
        }
    }
}
