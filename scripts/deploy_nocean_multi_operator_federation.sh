#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"

OPERATOR_HOSTS_CSV="${OPERATOR_HOSTS_CSV:-nocean,nocean,nocean,nocean,nocean}"
REMOTE_ROOT="${REMOTE_ROOT:-cdk-federation-multi-operator}"
PUBLIC_BASE_DOMAIN="${PUBLIC_BASE_DOMAIN:-nocean.xyz}"
NETWORK="${NETWORK:-signet}"
MEMBERS=5
THRESHOLD="${THRESHOLD:-3}"
PORT_START="${PORT_START:-21885}"
PROMETHEUS_PORT_START="${PROMETHEUS_PORT_START:-21985}"
SESSION_PREFIX="${SESSION_PREFIX:-cdk-fed-operator}"
WALLET_SESSION="${WALLET_SESSION:-cdk-fed-wallet}"
REQUIRE_UNIQUE_MACHINES="${REQUIRE_UNIQUE_MACHINES:-5}"
STOP_LEGACY_SESSION="${STOP_LEGACY_SESSION:-0}"
STATUS_TIMEOUT_SECS="${STATUS_TIMEOUT_SECS:-1800}"
STATUS_POLL_SECS="${STATUS_POLL_SECS:-3}"
ACTION_TIMEOUT_SECS="${ACTION_TIMEOUT_SECS:-300}"
DKG_MAX_TICKS="${DKG_MAX_TICKS:-7200}"
DKG_RETRY_DELAY_MS="${DKG_RETRY_DELAY_MS:-250}"
DKG_CEREMONY_TIMEOUT_SECONDS="${DKG_CEREMONY_TIMEOUT_SECONDS:-1800}"
DOCKER_PLATFORM="${DOCKER_PLATFORM:-linux/amd64}"
CARGO_REGISTRY_VOLUME="${CARGO_REGISTRY_VOLUME:-cdk-nocean-cargo-registry}"
BUILD_TARGET_DIR="$REPO_ROOT/target/linux-x86_64"
BARK_REPO_ROOT="${BARK_REPO_ROOT:-$REPO_ROOT/../bark-cdk}"
REPORT_FILE="${REPORT_FILE:-$REPO_ROOT/target/nocean-multi-operator-drill.json}"
OPERATOR_SCRIPT="$SCRIPT_DIR/nocean_federation_operator.sh"
WALLET_SCRIPT="$SCRIPT_DIR/nocean_federation_wallet_smoke.sh"
SSH_OPTIONS=(-o BatchMode=yes -o ConnectTimeout=15)
HOSTS=()
STAGING_DIR=""
MAX_DKG_TOTAL_RSS_KB=0
MAX_DKG_MEMBER_RSS_KB=0

usage() {
    cat <<'EOF'
Deploy and exercise the invitation-based five-operator federation setup UX on
Nocean. Every operator uses a separate SSH environment, protected directory,
and named tmux session. Identities and private configuration are generated only
inside those sessions. All DKG services start concurrently.

Usage:
  OPERATOR_HOSTS_CSV=host1,host2,host3,host4,host5 \
    scripts/deploy_nocean_multi_operator_federation.sh

The default host list repeats the existing `nocean` alias. It is intentionally
rejected by the default REQUIRE_UNIQUE_MACHINES=5 gate. To run a clearly labeled
single-host isolated-session rehearsal, set REQUIRE_UNIQUE_MACHINES=1 and, when
the legacy deployment owns the public ports, STOP_LEGACY_SESSION=1.

Important overrides:
  OPERATOR_HOSTS_CSV       Exactly five comma-separated SSH aliases.
  REMOTE_ROOT              Safe relative root created on every operator host.
  REQUIRE_UNIQUE_MACHINES  Minimum distinct /etc/machine-id values (default 5).
  STOP_LEGACY_SESSION      Stop cdk-federation before taking its public ports.
  PUBLIC_BASE_DOMAIN       Preconfigured cdk-{n} and cdk-fed-{n} TLS domain.
  BARK_REPO_ROOT           Sibling checkout needed for Cargo path resolution.
  REPORT_FILE              Local redacted JSON drill report.
EOF
}

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

require_command() {
    command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

require_positive() {
    [[ "$2" =~ ^[1-9][0-9]*$ ]] || die "$1 must be a positive integer"
}

sha256_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

member_host() {
    printf '%s' "${HOSTS[$(($1 - 1))]}"
}

operator_dir() {
    printf '%s/operator-%s' "$REMOTE_ROOT" "$1"
}

public_mint_url() {
    printf 'https://cdk-%s.%s' "$1" "$PUBLIC_BASE_DOMAIN"
}

federation_api_url() {
    printf 'https://cdk-fed-%s.%s' "$1" "$PUBLIC_BASE_DOMAIN"
}

append_shell_word() {
    local variable_name="$1" value="$2" quoted
    printf -v quoted '%q' "$value"
    printf -v "$variable_name" '%s %s' "${!variable_name}" "$quoted"
}

remote_command() {
    local member="$1" script="$2"
    shift 2
    local command host argument
    host="$(member_host "$member")"
    printf -v command 'cd %q && exec bash %q' "$(operator_dir "$member")" "$script"
    for argument in "$@"; do
        append_shell_word command "$argument"
    done
    ssh "${SSH_OPTIONS[@]}" "$host" "$command"
}

remote_operator() {
    local member="$1"
    shift
    remote_command "$member" scripts/nocean_federation_operator.sh "$@"
}

remote_wallet() {
    local member="$1"
    shift
    remote_command "$member" scripts/nocean_federation_wallet_smoke.sh "$@"
}

remote_simple() {
    local member="$1" command="$2"
    ssh "${SSH_OPTIONS[@]}" "$(member_host "$member")" "$command"
}

run_operator_action() {
    local member="$1" label="$2" action="$3" argument="${4:-}"
    remote_operator "$member" session-run-wait "$label" "$action" "$argument"
}

run_action_all() {
    local label="$1" action="$2" argument="${3:-}" member failed=0
    local pids=()
    for member in $(seq 1 "$MEMBERS"); do
        run_operator_action "$member" "$label" "$action" "$argument" &
        pids[$member]=$!
    done
    for member in $(seq 1 "$MEMBERS"); do
        if ! wait "${pids[$member]}"; then
            failed=1
        fi
    done
    ((failed == 0)) || die "operator action failed for at least one member: $action"
}

run_action_range() {
    local first="$1" last="$2" label="$3" action="$4" argument="${5:-}"
    local member failed=0 pids=()
    for member in $(seq "$first" "$last"); do
        run_operator_action "$member" "$label" "$action" "$argument" &
        pids[$member]=$!
    done
    for member in $(seq "$first" "$last"); do
        if ! wait "${pids[$member]}"; then
            failed=1
        fi
    done
    ((failed == 0)) || die "operator action failed for at least one member: $action"
}

service_all() {
    local verb="$1" label="${2:-}" mode="${3:-}" member failed=0 pids=()
    for member in $(seq 1 "$MEMBERS"); do
        if [[ "$verb" == "start" ]]; then
            remote_operator "$member" service-start "$label" "$mode" &
        else
            remote_operator "$member" service-stop &
        fi
        pids[$member]=$!
    done
    for member in $(seq 1 "$MEMBERS"); do
        if ! wait "${pids[$member]}"; then
            failed=1
        fi
    done
    ((failed == 0)) || die "one or more operator services failed to $verb"
}

wait_for_https() {
    local url="$1" expectation="$2" deadline code
    deadline=$((SECONDS + STATUS_TIMEOUT_SECS))
    while ((SECONDS < deadline)); do
        code="$(curl --silent --output /dev/null --write-out '%{http_code}' --max-time 5 "$url" || true)"
        if [[ "$expectation" == "ready" && "$code" == "200" ]]; then
            return 0
        fi
        if [[ "$expectation" == "down" && "$code" != "200" ]]; then
            return 0
        fi
        sleep "$STATUS_POLL_SECS"
    done
    die "HTTPS endpoint did not become $expectation before the timeout"
}

wait_for_public_runtime() {
    local deadline member ready code
    deadline=$((SECONDS + STATUS_TIMEOUT_SECS))
    while ((SECONDS < deadline)); do
        ready=0
        for member in $(seq 1 "$MEMBERS"); do
            code="$(curl --silent --output /dev/null --write-out '%{http_code}' --max-time 8 \
                "$(public_mint_url "$member")/v1/info" || true)"
            [[ "$code" == "200" ]] || continue
            code="$(curl --silent --output /dev/null --write-out '%{http_code}' --max-time 8 \
                "$(public_mint_url "$member")/federation/v1/config" || true)"
            [[ "$code" == "200" ]] && ready=$((ready + 1))
        done
        if ((ready == MEMBERS)); then
            return 0
        fi
        sleep "$STATUS_POLL_SECS"
    done
    die "public federation runtimes did not become healthy before the timeout"
}

transfer_invitation() {
    local label="$1" member="$2" source_host destination_host source_path destination_dir
    local local_invitation
    source_host="$(member_host 1)"
    destination_host="$(member_host "$member")"
    source_path="$(operator_dir 1)/ceremonies/$label/setup-state/invitations/member-$member.invite"
    destination_dir="$(operator_dir "$member")/ceremonies/$label"
    local_invitation="$STAGING_DIR/$label-member-$member.invite"
    scp -q "${SSH_OPTIONS[@]}" "$source_host:$source_path" "$local_invitation"
    chmod 600 "$local_invitation"
    local command
    printf -v command 'mkdir -p %q && chmod 700 %q' "$destination_dir" "$destination_dir"
    ssh "${SSH_OPTIONS[@]}" "$destination_host" "$command"
    scp -q "${SSH_OPTIONS[@]}" "$local_invitation" \
        "$destination_host:$destination_dir/incoming.invite.upload"
    printf -v command 'chmod 600 %q && mv %q %q' \
        "$destination_dir/incoming.invite.upload" \
        "$destination_dir/incoming.invite.upload" \
        "$destination_dir/incoming.invite"
    ssh "${SSH_OPTIONS[@]}" "$destination_host" "$command"
    rm -f -- "$local_invitation"
}

write_operator_env() {
    local member="$1" output="$2" listen_port prometheus_port
    listen_port=$((PORT_START + member - 1))
    prometheus_port=$((PROMETHEUS_PORT_START + member - 1))
    {
        printf 'MEMBER_ID=%q\n' "$member"
        printf 'MEMBERS=%q\n' "$MEMBERS"
        printf 'THRESHOLD=%q\n' "$THRESHOLD"
        printf 'NETWORK=%q\n' "$NETWORK"
        printf 'PUBLIC_MINT_URL=%q\n' "$(public_mint_url "$member")"
        printf 'FEDERATION_API_URL=%q\n' "$(federation_api_url "$member")"
        printf 'COORDINATOR_URL=%q\n' "$(federation_api_url 1)"
        printf 'LISTEN_ADDR=%q\n' "127.0.0.1:$listen_port"
        printf 'LISTEN_PORT=%q\n' "$listen_port"
        printf 'PROMETHEUS_PORT=%q\n' "$prometheus_port"
        printf 'TMUX_SESSION=%q\n' "$SESSION_PREFIX-op-$member"
        printf 'WALLET_SESSION=%q\n' "$WALLET_SESSION"
        printf 'ACTION_TIMEOUT_SECS=%q\n' "$ACTION_TIMEOUT_SECS"
        printf 'DKG_MAX_TICKS=%q\n' "$DKG_MAX_TICKS"
        printf 'DKG_RETRY_DELAY_MS=%q\n' "$DKG_RETRY_DELAY_MS"
        printf 'DKG_CEREMONY_TIMEOUT_SECONDS=%q\n' "$DKG_CEREMONY_TIMEOUT_SECONDS"
    } >"$output"
    chmod 600 "$output"
}

build_linux_binaries() {
    local rust_version rust_image
    rust_version="$(awk -F '"' '/^[[:space:]]*channel[[:space:]]*=/ { print $2; exit }' \
        "$REPO_ROOT/rust-toolchain.toml")"
    [[ -n "$rust_version" ]] || die "could not read rust-toolchain.toml"
    rust_image="${RUST_IMAGE:-rust:${rust_version}-bookworm}"
    printf 'Building invitation-based Nocean binaries for %s...\n' "$DOCKER_PLATFORM"
    docker run --rm \
        --platform "$DOCKER_PLATFORM" \
        --env "RUSTUP_TOOLCHAIN=$rust_version" \
        --mount "type=bind,src=$REPO_ROOT,dst=/work" \
        --mount "type=bind,src=$BARK_REPO_ROOT,dst=/bark-cdk,readonly" \
        --mount "type=volume,src=$CARGO_REGISTRY_VOLUME,dst=/usr/local/cargo/registry" \
        --workdir /work \
        "$rust_image" \
        bash -c '
            set -euo pipefail
            apt-get update >/dev/null
            apt-get install -y --no-install-recommends protobuf-compiler cmake clang >/dev/null
            export CARGO_TARGET_DIR=/work/target/linux-x86_64
            cargo build --locked --release -p cdk-mintd \
                --no-default-features \
                --features fakewallet,sqlite,info-page,federation,prometheus
            cargo build --locked --release -p cdk-cli \
                --no-default-features --features federation
            "$CARGO_TARGET_DIR/release/cdk-mintd" federation setup create --help >/dev/null
            "$CARGO_TARGET_DIR/release/cdk-cli" federation add --help >/dev/null
        '
}

install_operators() {
    local mintd="$BUILD_TARGET_DIR/release/cdk-mintd"
    local cli="$BUILD_TARGET_DIR/release/cdk-cli"
    local mintd_hash cli_hash member host dir env_file command
    [[ -x "$mintd" && -x "$cli" ]] || die "Linux build did not produce both binaries"
    mintd_hash="$(sha256_file "$mintd")"
    cli_hash="$(sha256_file "$cli")"
    for member in $(seq 1 "$MEMBERS"); do
        host="$(member_host "$member")"
        dir="$(operator_dir "$member")"
        printf -v command \
            'test ! -d %q || { echo "error: operator ceremony state already exists" >&2; exit 1; }; mkdir -p %q %q && chmod 700 %q' \
            "$dir/ceremonies" "$dir/bin" "$dir/scripts" "$dir"
        ssh "${SSH_OPTIONS[@]}" "$host" "$command"
        env_file="$STAGING_DIR/operator-$member.env"
        write_operator_env "$member" "$env_file"
        printf -v command \
            'test -x %q && test "$(sha256sum %q | awk '\''{print $1}'\'')" = %q' \
            "$dir/bin/cdk-mintd" "$dir/bin/cdk-mintd" "$mintd_hash"
        if ! ssh "${SSH_OPTIONS[@]}" "$host" "$command"; then
            scp -q "${SSH_OPTIONS[@]}" "$mintd" "$host:$dir/bin/cdk-mintd.upload"
            printf -v command \
                'test "$(sha256sum %q | awk '\''{print $1}'\'')" = %q && chmod 755 %q && mv %q %q' \
                "$dir/bin/cdk-mintd.upload" "$mintd_hash" \
                "$dir/bin/cdk-mintd.upload" "$dir/bin/cdk-mintd.upload" "$dir/bin/cdk-mintd"
            ssh "${SSH_OPTIONS[@]}" "$host" "$command"
        fi
        printf -v command \
            'test -x %q && test "$(sha256sum %q | awk '\''{print $1}'\'')" = %q' \
            "$dir/bin/cdk-cli" "$dir/bin/cdk-cli" "$cli_hash"
        if ! ssh "${SSH_OPTIONS[@]}" "$host" "$command"; then
            scp -q "${SSH_OPTIONS[@]}" "$cli" "$host:$dir/bin/cdk-cli.upload"
            printf -v command \
                'test "$(sha256sum %q | awk '\''{print $1}'\'')" = %q && chmod 755 %q && mv %q %q' \
                "$dir/bin/cdk-cli.upload" "$cli_hash" \
                "$dir/bin/cdk-cli.upload" "$dir/bin/cdk-cli.upload" "$dir/bin/cdk-cli"
            ssh "${SSH_OPTIONS[@]}" "$host" "$command"
        fi
        scp -q "${SSH_OPTIONS[@]}" "$OPERATOR_SCRIPT" \
            "$host:$dir/scripts/nocean_federation_operator.sh.upload"
        scp -q "${SSH_OPTIONS[@]}" "$WALLET_SCRIPT" \
            "$host:$dir/scripts/nocean_federation_wallet_smoke.sh.upload"
        scp -q "${SSH_OPTIONS[@]}" "$env_file" "$host:$dir/operator.env.upload"
        printf -v command \
            'set -eu; cd %q; chmod 755 scripts/*.upload; chmod 600 operator.env.upload; mv scripts/nocean_federation_operator.sh.upload scripts/nocean_federation_operator.sh; mv scripts/nocean_federation_wallet_smoke.sh.upload scripts/nocean_federation_wallet_smoke.sh; mv operator.env.upload operator.env' \
            "$dir"
        ssh "${SSH_OPTIONS[@]}" "$host" "$command"
        remote_operator "$member" validate
    done
}

machine_topology() {
    local member identity existing seen
    local identities=()
    unique_machine_count=0
    for member in $(seq 1 "$MEMBERS"); do
        identity="$(remote_simple "$member" \
            "test -r /etc/machine-id && sha256sum /etc/machine-id | cut -d ' ' -f 1")"
        [[ "$identity" =~ ^[0-9a-f]{64}$ ]] || die "could not verify an operator machine identity"
        seen=0
        for existing in "${identities[@]:-}"; do
            if [[ "$existing" == "$identity" ]]; then
                seen=1
                break
            fi
        done
        if ((seen == 0)); then
            identities+=($identity)
            unique_machine_count=$((unique_machine_count + 1))
        fi
    done
    ((unique_machine_count >= REQUIRE_UNIQUE_MACHINES)) \
        || die "topology has $unique_machine_count unique machine(s), fewer than required $REQUIRE_UNIQUE_MACHINES"
    if ((unique_machine_count == MEMBERS)); then
        topology_class="five-machine"
    elif ((unique_machine_count == 1)); then
        topology_class="single-host-isolated-session-rehearsal"
    else
        topology_class="partial-multi-host"
    fi
}

initialize_sessions() {
    local member failed=0 pids=()
    if [[ "$STOP_LEGACY_SESSION" == "1" ]]; then
        for member in $(seq 1 "$MEMBERS"); do
            remote_simple "$member" 'tmux kill-session -t cdk-federation 2>/dev/null || true'
        done
    fi
    for member in $(seq 1 "$MEMBERS"); do
        remote_operator "$member" session-init &
        pids[$member]=$!
    done
    for member in $(seq 1 "$MEMBERS"); do
        if ! wait "${pids[$member]}"; then
            failed=1
        fi
    done
    ((failed == 0)) || die "one or more operator tmux sessions failed to initialize"
}

sample_dkg_resources() {
    local member sample rss total=0
    for member in $(seq 1 "$MEMBERS"); do
        sample="$(remote_operator "$member" service-metrics 2>/dev/null || true)"
        rss="$(awk -F, 'NF == 3 { print $3 + 0 }' <<<"$sample")"
        rss="${rss:-0}"
        total=$((total + rss))
        if ((rss > MAX_DKG_MEMBER_RSS_KB)); then
            MAX_DKG_MEMBER_RSS_KB="$rss"
        fi
    done
    if ((total > MAX_DKG_TOTAL_RSS_KB)); then
        MAX_DKG_TOTAL_RSS_KB="$total"
    fi
}

collect_dkg_evidence() {
    local member failed=0 pids=() status_file
    for member in $(seq 1 "$MEMBERS"); do
        status_file="$STAGING_DIR/final-status-$member.json"
        remote_operator "$member" status-json active >"$status_file" &
        pids[$member]=$!
    done
    for member in $(seq 1 "$MEMBERS"); do
        if ! wait "${pids[$member]}"; then
            failed=1
        fi
    done
    ((failed == 0)) || die "one or more final DKG status reads failed"

    proposal_hash="$(jq -r '.proposal_hash' "$STAGING_DIR/final-status-1.json")"
    manifest_hash="$(jq -r '.manifest_hash' "$STAGING_DIR/final-status-1.json")"
    transcript_hash="$(jq -r '.dkg.transcript_hash' "$STAGING_DIR/final-status-1.json")"
    final_config_digest="$(jq -r '.dkg.final_config_digest' "$STAGING_DIR/final-status-1.json")"
    for member in $(seq 1 "$MEMBERS"); do
        status_file="$STAGING_DIR/final-status-$member.json"
        [[ "$(jq -r '.dkg.finalized_ready // false' "$status_file")" == "true" ]] \
            || die "member $member did not finalize DKG"
        [[ "$(jq -r '.proposal_hash' "$status_file")" == "$proposal_hash" ]] \
            || die "proposal hash differs across members"
        [[ "$(jq -r '.manifest_hash' "$status_file")" == "$manifest_hash" ]] \
            || die "manifest hash differs across members"
        [[ "$(jq -r '.dkg.transcript_hash' "$status_file")" == "$transcript_hash" ]] \
            || die "DKG transcript hash differs across members"
        [[ "$(jq -r '.dkg.final_config_digest' "$status_file")" == "$final_config_digest" ]] \
            || die "final config digest differs across members"
    done
}

verify_public_config() {
    local member hash
    public_config_hash="$(remote_operator 1 public-config-hash)"
    [[ "$public_config_hash" =~ ^[0-9a-f]{64}$ ]] || die "public config hash is invalid"
    for member in $(seq 2 "$MEMBERS"); do
        hash="$(remote_operator "$member" public-config-hash)"
        [[ "$hash" == "$public_config_hash" ]] \
            || die "activated public config differs across members"
    done
    local public_copy="$STAGING_DIR/federation-public.json"
    scp -q "${SSH_OPTIONS[@]}" \
        "$(member_host 1):$(operator_dir 1)/ceremonies/active/setup-state/activated/federation-public.json" \
        "$public_copy"
    setup_authorization="$(jq -r '.setup_authorization' "$public_copy")"
    [[ -n "$setup_authorization" && "$setup_authorization" != "null" ]] \
        || die "public config has no setup authorization"
}

verify_runtime_consensus() {
    local member
    for member in $(seq 1 "$MEMBERS"); do
        remote_operator "$member" runtime-status >/dev/null
    done
}

record_session_layouts() {
    local member
    : >"$STAGING_DIR/session-layout.csv"
    for member in $(seq 1 "$MEMBERS"); do
        remote_operator "$member" session-layout >>"$STAGING_DIR/session-layout.csv"
    done
    [[ "$(awk -F, '$2 == "operator" { count++ } END { print count + 0 }' \
        "$STAGING_DIR/session-layout.csv")" == "$MEMBERS" ]] \
        || die "operator tmux session audit is incomplete"
    [[ "$(awk -F, '$2 == "service" && $4 == "0" { count++ } END { print count + 0 }' \
        "$STAGING_DIR/session-layout.csv")" == "$MEMBERS" ]] \
        || die "runtime tmux service audit is incomplete"
}

write_report() {
    local revision source_dirty remaining_limit total_seconds
    revision="$(git -C "$REPO_ROOT" rev-parse HEAD)"
    if [[ -z "$(git -C "$REPO_ROOT" status --porcelain -- \
        scripts/deploy_nocean_multi_operator_federation.sh \
        scripts/nocean_federation_operator.sh \
        scripts/nocean_federation_wallet_smoke.sh)" ]]; then
        source_dirty=false
    else
        source_dirty=true
    fi
    if [[ "$topology_class" == "five-machine" ]]; then
        remaining_limit="none for the multi-host setup gate"
    else
        remaining_limit="only one physical Nocean host was reachable; this run is an isolated-session rehearsal and does not close the multi-host gate"
    fi
    total_seconds=$((SECONDS - drill_started_at))
    mkdir -p "$(dirname "$REPORT_FILE")"
    jq -n \
        --arg revision "$revision" \
        --argjson source_dirty "$source_dirty" \
        --arg topology "$topology_class" \
        --argjson members "$MEMBERS" \
        --argjson threshold "$THRESHOLD" \
        --argjson unique_machines "$unique_machine_count" \
        --arg tls_model "public HTTPS reverse proxy to operator-local loopback services" \
        --arg proposal_hash "$proposal_hash" \
        --arg manifest_hash "$manifest_hash" \
        --arg setup_authorization "$setup_authorization" \
        --arg transcript_hash "$transcript_hash" \
        --arg final_config_digest "$final_config_digest" \
        --arg public_config_hash "$public_config_hash" \
        --argjson failed_setup_replaced "$failed_setup_replaced" \
        --argjson coordinator_loss_after_certification "$coordinator_loss_after_certification" \
        --argjson dkg_seconds "$dkg_seconds" \
        --argjson total_seconds "$total_seconds" \
        --argjson max_dkg_total_rss_kb "$MAX_DKG_TOTAL_RSS_KB" \
        --argjson max_dkg_member_rss_kb "$MAX_DKG_MEMBER_RSS_KB" \
        --arg remaining_limit "$remaining_limit" \
        '{
            version: 1,
            result: "passed",
            source: {revision: $revision, deployment_scripts_dirty: $source_dirty},
            topology: {
                classification: $topology,
                members: $members,
                threshold: $threshold,
                unique_machines: $unique_machines,
                independent_operator_directories: true,
                independent_operator_tmux_sessions: true,
                tls_model: $tls_model
            },
            ordinary_cli_transcript: [
                "setup create", "setup join", "setup propose", "setup review",
                "setup approve", "setup status", "setup serve", "setup activate"
            ],
            isolation: {
                operator_local_identity_generation: true,
                shared_private_directories: false,
                centralized_private_config_generation: false,
                exchanged_artifacts: ["protected invitations", "authenticated protocol messages"]
            },
            evidence: {
                proposal_hash: $proposal_hash,
                manifest_hash: $manifest_hash,
                setup_authorization: $setup_authorization,
                transcript_hash: $transcript_hash,
                final_config_digest: $final_config_digest,
                public_config_hash: $public_config_hash,
                public_config_identical: true
            },
            recovery: {
                failed_pre_certification_setup_replaced_with_new_setup_id: $failed_setup_replaced,
                coordinator_loss_after_certification: $coordinator_loss_after_certification,
                one_member_restart_and_catch_up: true,
                coordinated_restart: true,
                encrypted_operator_local_backup_restore: true
            },
            wallet_smoke: {mint: true, send_receive: true, spent_replay_rejected: true, melt: true},
            timing_seconds: {dkg: $dkg_seconds, total: $total_seconds},
            resources: {
                max_dkg_total_rss_kb: $max_dkg_total_rss_kb,
                max_dkg_member_rss_kb: $max_dkg_member_rss_kb
            },
            remaining_limit: $remaining_limit
        }' >"$REPORT_FILE"
    printf 'Redacted Nocean drill report: %s\n' "$REPORT_FILE"
}

case "${1:-}" in
    -h|--help) usage; exit 0 ;;
    "") ;;
    *) usage >&2; die "unexpected argument" ;;
esac

for command in awk curl docker git jq scp seq ssh; do
    require_command "$command"
done
for file in "$OPERATOR_SCRIPT" "$WALLET_SCRIPT"; do
    [[ -f "$file" ]] || die "required deployment helper is missing"
done
[[ -f "$BARK_REPO_ROOT/bark/Cargo.toml" ]] \
    || die "BARK_REPO_ROOT must point to the Bark checkout required by Cargo"
BARK_REPO_ROOT="$(cd -- "$BARK_REPO_ROOT" && pwd)"
for setting in "$THRESHOLD" "$PORT_START" "$PROMETHEUS_PORT_START" \
    "$REQUIRE_UNIQUE_MACHINES" "$STATUS_TIMEOUT_SECS" "$STATUS_POLL_SECS" \
    "$ACTION_TIMEOUT_SECS" "$DKG_MAX_TICKS" "$DKG_RETRY_DELAY_MS" \
    "$DKG_CEREMONY_TIMEOUT_SECONDS"; do
    require_positive numeric_setting "$setting"
done
((THRESHOLD <= MEMBERS)) || die "THRESHOLD exceeds member count"
((PORT_START + MEMBERS - 1 <= 65535)) || die "member ports exceed 65535"
((PROMETHEUS_PORT_START + MEMBERS - 1 <= 65535)) \
    || die "Prometheus ports exceed 65535"
((REQUIRE_UNIQUE_MACHINES <= MEMBERS)) \
    || die "REQUIRE_UNIQUE_MACHINES exceeds member count"
[[ "$STOP_LEGACY_SESSION" == "0" || "$STOP_LEGACY_SESSION" == "1" ]] \
    || die "STOP_LEGACY_SESSION must be 0 or 1"
[[ "$REMOTE_ROOT" =~ ^[A-Za-z0-9._/-]+$ && "$REMOTE_ROOT" != /* ]] \
    || die "REMOTE_ROOT must be a safe relative path"
[[ "/$REMOTE_ROOT/" != *"/../"* ]] || die "REMOTE_ROOT must not contain '..'"
[[ "$PUBLIC_BASE_DOMAIN" =~ ^[A-Za-z0-9.-]+$ ]] \
    || die "PUBLIC_BASE_DOMAIN contains unsupported characters"
[[ "$SESSION_PREFIX" =~ ^[A-Za-z0-9._-]+$ ]] || die "SESSION_PREFIX is invalid"
[[ "$WALLET_SESSION" =~ ^[A-Za-z0-9._-]+$ ]] || die "WALLET_SESSION is invalid"
[[ "$NETWORK" == "signet" ]] || die "the Nocean HTTPS drill currently requires NETWORK=signet"

IFS=',' read -r -a HOSTS <<<"$OPERATOR_HOSTS_CSV"
[[ "${#HOSTS[@]}" == "$MEMBERS" ]] || die "OPERATOR_HOSTS_CSV must contain exactly five aliases"
for host in "${HOSTS[@]}"; do
    [[ "$host" =~ ^[A-Za-z0-9._@-]+$ ]] || die "SSH alias contains unsupported characters"
done

STAGING_DIR="$(mktemp -d)"
cleanup() {
    rm -rf -- "$STAGING_DIR"
}
trap cleanup EXIT

drill_started_at="$SECONDS"
machine_topology
build_linux_binaries
install_operators
initialize_sessions

printf 'Exercising incomplete pre-certification setup...\n'
run_operator_action 1 failed create
remote_operator 1 service-start failed setup
wait_for_https "$(federation_api_url 1)/federation/setup/v1/status" ready
for member in 2 3 4; do
    transfer_invitation failed "$member"
done
run_action_range 2 4 failed join
run_operator_action 1 failed propose-expect-failure
failed_setup_id="$(remote_operator 1 status-json failed | jq -r '.setup_id')"
remote_operator 1 service-stop

printf 'Running the ordinary five-operator setup transcript...\n'
run_operator_action 1 active create
remote_operator 1 service-start active setup
wait_for_https "$(federation_api_url 1)/federation/setup/v1/status" ready
for member in 2 3 4 5; do
    transfer_invitation active "$member"
done
run_action_range 2 5 active join
run_operator_action 1 active propose
proposal_hash="$(remote_operator 1 proposal-hash active)"
[[ "$proposal_hash" =~ ^[0-9a-f]{64}$ ]] || die "proposal hash is invalid"
run_action_all active review
for member in $(seq 1 "$MEMBERS"); do
    [[ "$(remote_operator "$member" review-proposal-hash active)" == "$proposal_hash" ]] \
        || die "member $member reviewed a different proposal"
done
run_action_all active approve "$proposal_hash"
run_action_all active certified-status
active_setup_id="$(remote_operator 1 status-json active | jq -r '.setup_id')"
[[ -n "$failed_setup_id" && -n "$active_setup_id" && "$failed_setup_id" != "$active_setup_id" ]] \
    || die "replacement setup did not use a new setup ID"
failed_setup_replaced=true

printf 'Removing the coordinator after certification, then starting every DKG member concurrently...\n'
remote_operator 1 service-stop
wait_for_https "$(federation_api_url 1)/federation/setup/v1/status" down
coordinator_loss_after_certification=true
dkg_started_at="$SECONDS"
service_all start active setup

deadline=$((SECONDS + STATUS_TIMEOUT_SECS))
dkg_ready=false
attempt=0
while ((SECONDS < deadline)); do
    attempt=$((attempt + 1))
    if ((attempt % 3 == 1)); then
        sample_dkg_resources
    fi
    if remote_operator 1 status-json active >"$STAGING_DIR/readiness-status.json" 2>/dev/null \
        && [[ "$(jq -r '.dkg.finalized_ready // false' "$STAGING_DIR/readiness-status.json")" == "true" ]]; then
        dkg_ready=true
        break
    fi
    sleep "$STATUS_POLL_SECS"
done
[[ "$dkg_ready" == "true" ]] || die "DKG did not finalize before the timeout"
sample_dkg_resources
collect_dkg_evidence
dkg_seconds=$((SECONDS - dkg_started_at))

printf 'Activating identical final configs locally and starting runtimes...\n'
service_all stop
run_action_all active activate
verify_public_config
service_all start active runtime
wait_for_public_runtime
verify_runtime_consensus

printf 'Running the normal federation wallet smoke in its own tmux session...\n'
remote_wallet 1 session-run-wait smoke

printf 'Exercising a one-member outage and catch-up...\n'
remote_operator 5 service-stop
wait_for_https "$(public_mint_url 5)/v1/info" down
remote_wallet 1 session-run-wait offline-operation
remote_operator 5 service-start active runtime
deadline=$((SECONDS + STATUS_TIMEOUT_SECS))
while ((SECONDS < deadline)); do
    if remote_operator 5 runtime-status >/dev/null 2>&1; then
        break
    fi
    sleep "$STATUS_POLL_SECS"
done
remote_operator 5 runtime-status >/dev/null \
    || die "restarted member did not catch up before the timeout"

printf 'Exercising coordinated restart plus operator-local encrypted backup/restore...\n'
service_all stop
run_operator_action 3 active backup-restore
service_all start active runtime
wait_for_public_runtime
verify_runtime_consensus
record_session_layouts
write_report
printf 'Nocean %s deployment drill passed.\n' "$topology_class"
