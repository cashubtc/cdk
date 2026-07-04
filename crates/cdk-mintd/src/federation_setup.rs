#[cfg(all(feature = "federation", feature = "fakewallet"))]
use std::collections::BTreeMap;
#[cfg(all(feature = "federation", feature = "fakewallet"))]
use std::path::{Path, PathBuf};
#[cfg(all(feature = "federation", feature = "fakewallet"))]
use std::str::FromStr;

#[cfg(all(feature = "federation", feature = "fakewallet"))]
use anyhow::{anyhow, Context};
#[cfg(feature = "federation")]
use anyhow::{bail, Result};
#[cfg(all(feature = "federation", feature = "fakewallet"))]
use bip39::Mnemonic;
#[cfg(all(feature = "federation", feature = "fakewallet"))]
use cdk_common::federation::{
    generate_trusted_dealer_configs, MemberId, ThresholdParams, TrustedDealerConfigBundle,
    TrustedDealerKeysetSpec, TrustedDealerMember,
};
#[cfg(all(feature = "federation", feature = "fakewallet"))]
use cdk_common::mint_url::MintUrl;
#[cfg(all(feature = "federation", feature = "fakewallet"))]
use cdk_common::nuts::nut01::BlsSecretKey;
#[cfg(all(feature = "federation", feature = "fakewallet"))]
use cdk_common::{Amount, CurrencyUnit, SecretKey};

#[cfg(feature = "federation")]
use crate::cli::FederationTrustedDealerArgs;

#[cfg(all(feature = "federation", feature = "fakewallet"))]
const PUBLIC_CONFIG_FILE: &str = "federation-public.json";
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const PRIVATE_CONFIG_FILE: &str = "federation-private.json";
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const MEMBER_CONFIG_FILE: &str = "config.toml";
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const GENERATED_GITIGNORE_FILE: &str = ".gitignore";
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const SMOKE_SCRIPT_FILE: &str = "smoke-local-federation.sh";

#[cfg(all(feature = "federation", feature = "fakewallet"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrustedDealerArtifacts {
    pub(crate) public_config_path: PathBuf,
    pub(crate) smoke_script_path: PathBuf,
    pub(crate) member_artifacts: BTreeMap<MemberId, TrustedDealerMemberArtifacts>,
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrustedDealerMemberArtifacts {
    pub(crate) config_path: PathBuf,
    pub(crate) private_config_path: PathBuf,
    pub(crate) work_dir: PathBuf,
}

#[cfg(feature = "federation")]
pub(crate) fn run_trusted_dealer_command(args: &FederationTrustedDealerArgs) -> Result<()> {
    #[cfg(not(feature = "fakewallet"))]
    {
        let _ = args;
        bail!("the federation trusted-dealer generator requires the fakewallet feature");
    }

    #[cfg(feature = "fakewallet")]
    {
        let artifacts = generate_trusted_dealer_artifacts(args)?;
        println!(
            "Wrote federation public config: {}",
            artifacts.public_config_path.display()
        );
        for (member_id, member) in &artifacts.member_artifacts {
            println!(
                "Wrote member {} config: {}",
                member_id,
                member.config_path.display()
            );
            println!(
                "Run member {} with work dir: {}",
                member_id,
                member.work_dir.display()
            );
        }
        println!(
            "Run local federation smoke test: {}",
            artifacts.smoke_script_path.display()
        );

        Ok(())
    }
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
pub(crate) fn generate_trusted_dealer_artifacts(
    args: &FederationTrustedDealerArgs,
) -> Result<TrustedDealerArtifacts> {
    validate_trusted_dealer_args(args)?;

    let output_dir = create_output_dir(&args.output_dir)?;
    write_generated_gitignore(&output_dir, args.force)?;

    let threshold_params = ThresholdParams::with_bft_consensus(args.members, args.threshold)?;
    let unit = CurrencyUnit::from_str(&args.unit)
        .with_context(|| format!("invalid federation unit {}", args.unit))?;
    let members = trusted_dealer_members(args.members, &args.host, args.port_start)?;
    let keyset_spec = trusted_dealer_keyset_spec(
        unit.clone(),
        args.input_fee_ppk,
        args.final_expiry,
        args.threshold,
        &args.amounts,
    )?;

    let bundle = generate_trusted_dealer_configs(threshold_params, members, vec![keyset_spec])
        .context("failed to generate trusted-dealer federation configs")?;
    validate_bundle(&bundle)?;

    let public_config_path = output_dir.join(PUBLIC_CONFIG_FILE);
    write_json_file(&public_config_path, &bundle.public_config, args.force)?;

    let mut member_artifacts = BTreeMap::new();
    for (member_id, private_config) in &bundle.private_configs {
        let member = bundle
            .public_config
            .member(*member_id)
            .ok_or_else(|| anyhow!("generated bundle is missing member {}", member_id))?;
        let member_dir = output_dir.join(format!("member-{}", member_id.get()));
        std::fs::create_dir_all(&member_dir)
            .with_context(|| format!("failed to create {}", member_dir.display()))?;
        let private_config_path = member_dir.join(PRIVATE_CONFIG_FILE);
        write_json_file(&private_config_path, private_config, args.force)?;

        let work_dir = member_dir.join("work");
        let config_path = member_dir.join(MEMBER_CONFIG_FILE);
        let member_url = member.public_mint_url.to_string();
        let config_toml = member_config_toml(
            member_id,
            &args.host,
            &member_url,
            &member_url,
            args.port_start
                .checked_add(member_id.get() - 1)
                .ok_or_else(|| anyhow!("member ports overflow u16"))?,
            &public_config_path,
            &private_config_path,
            &config_path,
            &work_dir,
            &unit,
        )?;
        write_text_file(&config_path, &config_toml, args.force)?;

        member_artifacts.insert(
            *member_id,
            TrustedDealerMemberArtifacts {
                config_path,
                private_config_path,
                work_dir,
            },
        );
    }

    let smoke_script_path = output_dir.join(SMOKE_SCRIPT_FILE);
    let smoke_script = local_smoke_script(&public_config_path, &member_artifacts)?;
    write_executable_text_file(&smoke_script_path, &smoke_script, args.force)?;

    Ok(TrustedDealerArtifacts {
        public_config_path,
        smoke_script_path,
        member_artifacts,
    })
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn validate_trusted_dealer_args(args: &FederationTrustedDealerArgs) -> Result<()> {
    if args.members == 0 {
        bail!("federation trusted-dealer generation needs at least one member");
    }
    if args.threshold == 0 || args.threshold > args.members {
        bail!(
            "invalid federation threshold {} for {} members",
            args.threshold,
            args.members
        );
    }
    if args.amounts.is_empty() {
        bail!("federation trusted-dealer generation needs at least one amount");
    }
    if args.amounts.iter().any(|amount| *amount == 0) {
        bail!("federation trusted-dealer amounts must be non-zero");
    }
    let last_port_offset = args.members.saturating_sub(1);
    args.port_start
        .checked_add(last_port_offset)
        .ok_or_else(|| anyhow!("member ports overflow u16"))?;

    Ok(())
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn create_output_dir(output_dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;
    output_dir
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", output_dir.display()))
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn write_generated_gitignore(output_dir: &Path, force: bool) -> Result<()> {
    write_text_file(
        &output_dir.join(GENERATED_GITIGNORE_FILE),
        "# Generated local federation material may contain secrets.\n*\n",
        force,
    )
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn trusted_dealer_members(
    member_count: u16,
    host: &str,
    port_start: u16,
) -> Result<Vec<TrustedDealerMember>> {
    (1..=member_count)
        .map(|id| {
            let member_id = MemberId::new(id)?;
            let port = port_start
                .checked_add(id - 1)
                .ok_or_else(|| anyhow!("member ports overflow u16"))?;
            let url = MintUrl::from_str(&format!("http://{host}:{port}"))
                .with_context(|| format!("invalid generated URL for member {member_id}"))?;
            Ok(TrustedDealerMember::new(
                member_id,
                url.clone(),
                url,
                SecretKey::generate(),
            ))
        })
        .collect()
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn trusted_dealer_keyset_spec(
    unit: CurrencyUnit,
    input_fee_ppk: u64,
    final_expiry: Option<u64>,
    threshold: u16,
    amounts: &[u64],
) -> Result<TrustedDealerKeysetSpec> {
    let mut amount_coefficients = BTreeMap::new();
    for amount in amounts {
        let coefficients = random_bls_coefficients(threshold)?;
        if amount_coefficients
            .insert(Amount::from(*amount), coefficients)
            .is_some()
        {
            bail!("duplicate federation amount {amount}");
        }
    }

    Ok(TrustedDealerKeysetSpec::new(
        unit,
        true,
        input_fee_ppk,
        final_expiry,
        amount_coefficients,
    ))
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn random_bls_coefficients(threshold: u16) -> Result<Vec<BlsSecretKey>> {
    (0..threshold)
        .map(|_| {
            let secret_key = SecretKey::generate_bls();
            Ok(secret_key.as_bls()?.clone())
        })
        .collect()
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn validate_bundle(bundle: &TrustedDealerConfigBundle) -> Result<()> {
    bundle
        .public_config
        .validate()
        .context("generated public federation config is invalid")?;
    for private_config in bundle.private_configs.values() {
        private_config
            .validate_against_public(&bundle.public_config)
            .context("generated private federation config is invalid")?;
    }

    Ok(())
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn member_config_toml(
    member_id: &MemberId,
    listen_host: &str,
    mint_url: &str,
    federation_url: &str,
    listen_port: u16,
    public_config_path: &Path,
    private_config_path: &Path,
    config_path: &Path,
    work_dir: &Path,
    unit: &CurrencyUnit,
) -> Result<String> {
    let mnemonic = Mnemonic::generate(12)
        .map_err(|err| anyhow!("failed to generate member mnemonic: {err}"))?
        .to_string();
    let public_config_path = path_to_string(public_config_path);
    let private_config_path = path_to_string(private_config_path);
    let config_path = path_to_string(config_path);
    let work_dir = path_to_string(work_dir);
    let unit = unit.to_string().to_lowercase();

    Ok(format!(
        r#"# Generated by cdk-mintd federation trusted-dealer.
# Contains local test secrets. Do not commit this file.
# Start this member with:
# cdk-mintd --work-dir {work_dir} --config {config_path}

[info]
url = {mint_url}
listen_host = {listen_host}
listen_port = {listen_port}
mnemonic = {mnemonic}
enable_info_page = true

[info.http_cache]
backend = "memory"
ttl = 60
tti = 60

[mint_info]
name = {name}
description = {description}

[database]
engine = "sqlite"

[ln]
ln_backend = "fakewallet"
unit = {unit}
min_mint = 1
max_mint = 500000
min_melt = 1
max_melt = 500000

[fake_wallet]
supported_units = [{unit}]
fee_percent = 0.0
reserve_fee_min = 1
min_delay_time = 0
max_delay_time = 0

[federation]
enabled = true
public_config_path = {public_config_path}
private_config_path = {private_config_path}
"#,
        work_dir = toml_string(&work_dir)?,
        config_path = toml_string(&config_path)?,
        mint_url = toml_string(mint_url)?,
        listen_host = toml_string(listen_host)?,
        mnemonic = toml_string(&mnemonic)?,
        name = toml_string(&format!("CDK federation member {}", member_id.get()))?,
        description = toml_string(&format!(
            "Local fake-wallet federation member {} at {}",
            member_id.get(),
            federation_url
        ))?,
        unit = toml_string(&unit)?,
        public_config_path = toml_string(&public_config_path)?,
        private_config_path = toml_string(&private_config_path)?,
    ))
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn toml_string(value: &str) -> Result<String> {
    Ok(serde_json::to_string(value)?)
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn write_json_file<T>(path: &Path, value: &T, force: bool) -> Result<()>
where
    T: serde::Serialize + ?Sized,
{
    let json = serde_json::to_string_pretty(value)?;
    write_text_file(path, &format!("{json}\n"), force)
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn write_text_file(path: &Path, contents: &str, force: bool) -> Result<()> {
    if path.exists() && !force {
        bail!(
            "{} already exists; pass --force to overwrite generated federation material",
            path.display()
        );
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(path, contents).with_context(|| format!("failed to write {}", path.display()))
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn write_executable_text_file(path: &Path, contents: &str, force: bool) -> Result<()> {
    write_text_file(path, contents, force)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let permissions = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(path, permissions)
            .with_context(|| format!("failed to mark {} executable", path.display()))?;
    }

    Ok(())
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn local_smoke_script(
    public_config_path: &Path,
    member_artifacts: &BTreeMap<MemberId, TrustedDealerMemberArtifacts>,
) -> Result<String> {
    let members = member_artifacts
        .keys()
        .map(|member_id| member_id.get().to_string())
        .collect::<Vec<_>>()
        .join(" ");
    let primary_member = member_artifacts
        .keys()
        .next()
        .ok_or_else(|| anyhow!("local federation smoke script needs at least one member"))?;
    let restart_member = member_artifacts.keys().nth(1).unwrap_or(primary_member);

    let config_cases = member_path_cases(member_artifacts, |member| &member.config_path)?;
    let work_dir_cases = member_path_cases(member_artifacts, |member| &member.work_dir)?;
    let private_config_cases =
        member_path_cases(member_artifacts, |member| &member.private_config_path)?;
    let public_config_path = shell_single_quote(&path_to_string(public_config_path));

    Ok(format!(
        r#"#!/usr/bin/env bash
set -euo pipefail

CDK_MINTD_BIN="${{CDK_MINTD_BIN:-cdk-mintd}}"
PUBLIC_CONFIG={public_config_path}
MEMBERS=({members})
PRIMARY_MEMBER="${{PRIMARY_MEMBER:-{primary_member}}}"
RESTART_MEMBER="${{RESTART_MEMBER:-{restart_member}}}"
STATUS_RETRIES="${{STATUS_RETRIES:-60}}"
STATUS_SLEEP_SECONDS="${{STATUS_SLEEP_SECONDS:-1}}"
PENDING_WALLET_DB="${{PENDING_WALLET_DB:-$(dirname "$PUBLIC_CONFIG")/pending-melt-wallet.sqlite}}"
FAILED_PENDING_WALLET_DB="${{FAILED_PENDING_WALLET_DB:-$(dirname "$PUBLIC_CONFIG")/failed-pending-melt-wallet.sqlite}}"
PIDS=()

member_config() {{
  case "$1" in
{config_cases}    *) return 1 ;;
  esac
}}

member_work_dir() {{
  case "$1" in
{work_dir_cases}    *) return 1 ;;
  esac
}}

member_private_config() {{
  case "$1" in
{private_config_cases}    *) return 1 ;;
  esac
}}

start_member() {{
  local member="$1"
  local config
  local work_dir
  config="$(member_config "$member")"
  work_dir="$(member_work_dir "$member")"
  mkdir -p "$work_dir"
  "$CDK_MINTD_BIN" --work-dir "$work_dir" --config "$config" > "$work_dir/mintd.log" 2>&1 &
  PIDS[$member]=$!
  printf 'started member %s with pid %s\n' "$member" "${{PIDS[$member]}}"
}}

stop_member() {{
  local member="$1"
  local pid="${{PIDS[$member]:-}}"
  if [[ -z "$pid" ]]; then
    return 0
  fi
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  unset 'PIDS[$member]'
  printf 'stopped member %s\n' "$member"
}}

cleanup() {{
  for member in "${{MEMBERS[@]}}"; do
    stop_member "$member"
  done
}}

reset_member_work_dirs() {{
  rm -f "$PENDING_WALLET_DB" "$PENDING_WALLET_DB-shm" "$PENDING_WALLET_DB-wal"
  rm -f "$FAILED_PENDING_WALLET_DB" "$FAILED_PENDING_WALLET_DB-shm" "$FAILED_PENDING_WALLET_DB-wal"
  local member
  for member in "${{MEMBERS[@]}}"; do
    local work_dir
    work_dir="$(member_work_dir "$member")"
    rm -rf "$work_dir"
    mkdir -p "$work_dir"
  done
}}

wait_for_status() {{
  local member="$1"
  local work_dir
  work_dir="$(member_work_dir "$member")"
  for _ in $(seq 1 "$STATUS_RETRIES"); do
    if member_status_line "$member" >/dev/null 2>&1; then
      return 0
    fi
    sleep "$STATUS_SLEEP_SECONDS"
  done
  printf 'member %s did not answer signed federation status\n' "$member" >&2
  tail -n 80 "$work_dir/mintd.log" >&2 || true
  return 1
}}

member_status_line() {{
  local member="$1"
  local private_config
  private_config="$(member_private_config "$member")"
  "$CDK_MINTD_BIN" federation status --public-config "$PUBLIC_CONFIG" --private-config "$private_config" --member "$member" --require-ready | awk '/^member / {{ print; exit }}'
}}

member_payment_report_lines() {{
  local member="$1"
  local private_config
  private_config="$(member_private_config "$member")"
  "$CDK_MINTD_BIN" federation status --public-config "$PUBLIC_CONFIG" --private-config "$private_config" --member "$member" --require-ready | awk '/^  payment_report / {{ print }}'
}}

status_field() {{
  local line="$1"
  local key="$2"
  local field
  for field in $line; do
    case "$field" in
      "$key"=*)
        printf '%s\n' "${{field#*=}}"
        return 0
        ;;
    esac
  done
  return 1
}}

wait_for_converged() {{
  for _ in $(seq 1 "$STATUS_RETRIES"); do
    local baseline_operations=""
    local baseline_order_digest=""
    local baseline_state_digest=""
    local converged=1
    local member
    for member in "${{MEMBERS[@]}}"; do
      local line
      local operations
      local order_digest
      local state_digest
      if ! line="$(member_status_line "$member" 2>/dev/null)"; then
        converged=0
        break
      fi
      if ! operations="$(status_field "$line" operations)"; then
        converged=0
        break
      fi
      if ! order_digest="$(status_field "$line" order_digest)"; then
        converged=0
        break
      fi
      if ! state_digest="$(status_field "$line" state_digest)"; then
        converged=0
        break
      fi
      if [[ -z "$baseline_operations" ]]; then
        baseline_operations="$operations"
        baseline_order_digest="$order_digest"
        baseline_state_digest="$state_digest"
        continue
      fi
      if [[ "$operations" != "$baseline_operations" || "$order_digest" != "$baseline_order_digest" || "$state_digest" != "$baseline_state_digest" ]]; then
        converged=0
        break
      fi
    done
    if [[ "$converged" == "1" ]]; then
      return 0
    fi
    sleep "$STATUS_SLEEP_SECONDS"
  done
  printf 'federation did not converge\n' >&2
  for member in "${{MEMBERS[@]}}"; do
    member_status_line "$member" >&2 || true
  done
  return 1
}}

wait_for_quarantine_report() {{
  local member="$1"
  for _ in $(seq 1 "$STATUS_RETRIES"); do
    if member_payment_report_lines "$member" | grep -q 'quarantined=true'; then
      return 0
    fi
    sleep "$STATUS_SLEEP_SECONDS"
  done
  printf 'member %s did not report an accepted payment observation quarantine\n' "$member" >&2
  local private_config
  private_config="$(member_private_config "$member")"
  "$CDK_MINTD_BIN" federation status --public-config "$PUBLIC_CONFIG" --private-config "$private_config" --member "$member" --require-ready >&2 || true
  return 1
}}

trap cleanup EXIT INT TERM

reset_member_work_dirs

for member in "${{MEMBERS[@]}}"; do
  start_member "$member"
done

for member in "${{MEMBERS[@]}}"; do
  wait_for_status "$member"
done

wait_for_converged
"$CDK_MINTD_BIN" federation smoke-operations --public-config "$PUBLIC_CONFIG"
wait_for_converged

"$CDK_MINTD_BIN" federation smoke-operations --public-config "$PUBLIC_CONFIG" --scenario pending-melt-start --wallet-db "$PENDING_WALLET_DB"
wait_for_converged

stop_member "$RESTART_MEMBER"
start_member "$RESTART_MEMBER"
wait_for_status "$RESTART_MEMBER"

wait_for_converged
"$CDK_MINTD_BIN" federation smoke-operations --public-config "$PUBLIC_CONFIG" --scenario pending-melt-finish --wallet-db "$PENDING_WALLET_DB"
wait_for_converged

"$CDK_MINTD_BIN" federation smoke-operations --public-config "$PUBLIC_CONFIG" --scenario pending-melt-fail-start --wallet-db "$FAILED_PENDING_WALLET_DB"
wait_for_converged

stop_member "$RESTART_MEMBER"
start_member "$RESTART_MEMBER"
wait_for_status "$RESTART_MEMBER"

wait_for_converged
"$CDK_MINTD_BIN" federation smoke-operations --public-config "$PUBLIC_CONFIG" --scenario pending-melt-fail-finish --wallet-db "$FAILED_PENDING_WALLET_DB"
wait_for_converged

"$CDK_MINTD_BIN" federation smoke-operations --public-config "$PUBLIC_CONFIG" --private-config "$(member_private_config "$PRIMARY_MEMBER")" --scenario quarantine-mint-conflict
wait_for_converged
wait_for_quarantine_report "$PRIMARY_MEMBER"

stop_member "$RESTART_MEMBER"
start_member "$RESTART_MEMBER"
wait_for_status "$RESTART_MEMBER"

wait_for_converged
wait_for_quarantine_report "$RESTART_MEMBER"

printf 'local federation smoke test passed\n'
"#,
        public_config_path = public_config_path,
        members = members,
        primary_member = primary_member.get(),
        restart_member = restart_member.get(),
        config_cases = config_cases,
        work_dir_cases = work_dir_cases,
        private_config_cases = private_config_cases,
    ))
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn member_path_cases<F>(
    member_artifacts: &BTreeMap<MemberId, TrustedDealerMemberArtifacts>,
    path: F,
) -> Result<String>
where
    F: Fn(&TrustedDealerMemberArtifacts) -> &Path,
{
    let mut cases = String::new();
    for (member_id, member) in member_artifacts {
        cases.push_str(&format!(
            "    {}) printf '%s\\n' {} ;;\n",
            member_id.get(),
            shell_single_quote(&path_to_string(path(member)))
        ));
    }
    Ok(cases)
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(all(test, feature = "federation", feature = "fakewallet"))]
mod tests {
    use super::*;
    use crate::config;
    use cdk_common::federation::{FederationConfig, MemberConfigPrivate};

    fn temp_output_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "cdk_mintd_federation_{name}_{}",
            std::process::id()
        ))
    }

    fn generator_args(output_dir: PathBuf) -> FederationTrustedDealerArgs {
        FederationTrustedDealerArgs {
            output_dir,
            members: 3,
            threshold: 2,
            unit: "sat".to_string(),
            amounts: vec![1, 2, 4],
            input_fee_ppk: 0,
            final_expiry: None,
            host: "127.0.0.1".to_string(),
            port_start: 8085,
            force: false,
        }
    }

    #[test]
    fn trusted_dealer_artifacts_are_written_and_validated() {
        let output_dir = temp_output_dir("artifacts");
        let _ = std::fs::remove_dir_all(&output_dir);
        let artifacts = generate_trusted_dealer_artifacts(&generator_args(output_dir.clone()))
            .expect("trusted dealer artifacts");

        let public_json =
            std::fs::read_to_string(&artifacts.public_config_path).expect("public config file");
        let public_config: FederationConfig =
            serde_json::from_str(&public_json).expect("public config json");
        public_config.validate().expect("public config validates");
        assert_eq!(public_config.threshold_params.member_count, 3);
        assert_eq!(public_config.threshold_params.signature_threshold, 2);
        assert_eq!(public_config.keysets.len(), 1);

        let smoke_script =
            std::fs::read_to_string(&artifacts.smoke_script_path).expect("smoke script file");
        assert!(smoke_script.starts_with("#!/usr/bin/env bash"));
        assert!(smoke_script.contains("federation status"));
        assert!(smoke_script.contains("federation smoke-operations"));
        assert!(smoke_script.contains("--require-ready"));
        assert!(smoke_script.contains("member_status_line()"));
        assert!(smoke_script.contains("private_config=\"$(member_private_config \"$member\")\""));
        assert!(!smoke_script
            .contains("private_config=\"$(member_private_config \"$PRIMARY_MEMBER\")\""));
        assert!(smoke_script.contains("reset_member_work_dirs"));
        assert!(smoke_script.contains("rm -rf \"$work_dir\""));
        assert!(smoke_script.contains("PRIMARY_MEMBER"));
        assert!(smoke_script.contains("RESTART_MEMBER"));
        assert!(smoke_script.contains("PENDING_WALLET_DB"));
        assert!(smoke_script.contains("FAILED_PENDING_WALLET_DB"));
        assert!(smoke_script.contains("member_payment_report_lines()"));
        assert!(smoke_script.contains("wait_for_quarantine_report()"));
        assert!(smoke_script.contains("--scenario pending-melt-start"));
        assert!(smoke_script.contains("--scenario pending-melt-finish"));
        assert!(smoke_script.contains("--scenario pending-melt-fail-start"));
        assert!(smoke_script.contains("--scenario pending-melt-fail-finish"));
        assert!(smoke_script.contains("--scenario quarantine-mint-conflict"));
        assert!(smoke_script.contains("--wallet-db \"$PENDING_WALLET_DB\""));
        assert!(smoke_script.contains("--wallet-db \"$FAILED_PENDING_WALLET_DB\""));
        assert!(smoke_script
            .contains("--private-config \"$(member_private_config \"$PRIMARY_MEMBER\")\""));
        assert!(smoke_script.contains("wait_for_quarantine_report \"$PRIMARY_MEMBER\""));
        assert!(smoke_script.contains("wait_for_quarantine_report \"$RESTART_MEMBER\""));
        assert!(smoke_script.contains("local federation smoke test passed"));
        assert!(smoke_script.contains(&artifacts.public_config_path.to_string_lossy().to_string()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mode = std::fs::metadata(&artifacts.smoke_script_path)
                .expect("smoke script metadata")
                .permissions()
                .mode();
            assert_ne!(mode & 0o111, 0);
        }

        for (member_id, member_artifacts) in &artifacts.member_artifacts {
            let private_json = std::fs::read_to_string(&member_artifacts.private_config_path)
                .expect("private config file");
            let private_config: MemberConfigPrivate =
                serde_json::from_str(&private_json).expect("private config json");
            assert_eq!(private_config.member_id, *member_id);
            private_config
                .validate_against_public(&public_config)
                .expect("private config validates");

            let settings = config::Settings::try_new(Some(&member_artifacts.config_path))
                .expect("member config toml");
            assert!(
                settings
                    .federation
                    .as_ref()
                    .expect("federation settings")
                    .enabled
            );
            assert_eq!(settings.info.listen_port, 8084 + member_id.get());
            assert!(
                smoke_script.contains(&member_artifacts.config_path.to_string_lossy().to_string())
            );
            assert!(smoke_script.contains(
                &member_artifacts
                    .private_config_path
                    .to_string_lossy()
                    .to_string()
            ));
            assert!(smoke_script.contains(&member_artifacts.work_dir.to_string_lossy().to_string()));
        }

        let _ = std::fs::remove_dir_all(&output_dir);
    }

    #[test]
    fn trusted_dealer_artifacts_refuse_to_overwrite_without_force() {
        let output_dir = temp_output_dir("overwrite");
        let _ = std::fs::remove_dir_all(&output_dir);
        let args = generator_args(output_dir.clone());
        generate_trusted_dealer_artifacts(&args).expect("first trusted dealer artifacts");

        let err = generate_trusted_dealer_artifacts(&args)
            .expect_err("second generation should refuse overwrite");
        assert!(err.to_string().contains("already exists"));

        let mut forced_args = args;
        forced_args.force = true;
        generate_trusted_dealer_artifacts(&forced_args).expect("forced generation");

        let _ = std::fs::remove_dir_all(&output_dir);
    }
}
