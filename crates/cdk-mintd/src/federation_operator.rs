//! Shared authentication inputs for federation operator commands.

#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
use cdk_common::federation::{FederationConfig, MemberConfigPrivate, MemberConfigPublic, MemberId};

#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
use crate::cli::CLIArgs;

#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
pub(crate) async fn load_configs(
    cli_args: &CLIArgs,
    public_config: &Option<PathBuf>,
    private_config: &Option<PathBuf>,
    action: &str,
) -> Result<(FederationConfig, MemberConfigPrivate)> {
    let (public_path, private_path) =
        crate::federation_config_paths(cli_args, public_config, private_config, action).await?;
    let public: FederationConfig =
        crate::read_federation_json(&public_path, "public federation config")?;
    let private: MemberConfigPrivate =
        crate::read_federation_json(&private_path, "private federation config")?;
    private
        .validate_against_public(&public)
        .context("private federation config does not match public federation config")?;
    Ok((public, private))
}

#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
pub(crate) fn target_member(
    public: &FederationConfig,
    private: &MemberConfigPrivate,
    requested: Option<u16>,
) -> Result<MemberConfigPublic> {
    let member_id = match requested {
        Some(member_id) => MemberId::new(member_id)?,
        None => private.member_id,
    };
    public
        .member(member_id)
        .cloned()
        .with_context(|| format!("federation public config has no member {member_id}"))
}

/// Use the peer protocol's nanosecond sequence scale with process-wide uniqueness.
pub(crate) fn operator_sequence() -> Result<u64> {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos())?;
    let mut last = LAST.load(Ordering::Relaxed);
    loop {
        let next = now.max(last.checked_add(1).context("operator sequence exhausted")?);
        match LAST.compare_exchange_weak(last, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return Ok(next),
            Err(observed) => last = observed,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn concurrent_commands_have_distinct_nanosecond_sequences() {
        let sequences = std::thread::scope(|scope| {
            let tasks: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        (0..128)
                            .map(|_| operator_sequence().expect("sequence"))
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            tasks
                .into_iter()
                .flat_map(|task| task.join().expect("command"))
                .collect::<BTreeSet<_>>()
        });
        assert_eq!(sequences.len(), 8 * 128);
        assert!(sequences.first().copied().expect("sequence") > 1_000_000_000_000_000_000);
    }
}
