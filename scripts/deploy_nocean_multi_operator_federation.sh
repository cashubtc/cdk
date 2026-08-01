#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"

OPERATOR_HOSTS_CSV="${OPERATOR_HOSTS_CSV:-nocean,nocean,nocean,nocean,nocean}"
REMOTE_ROOT="${REMOTE_ROOT:-cdk-federation-iroh-multi-operator}"
PUBLIC_BASE_DOMAIN="${PUBLIC_BASE_DOMAIN:-nocean.xyz}"
PUBLIC_TRANSPORT="${PUBLIC_TRANSPORT:-iroh}"
NETWORK="${NETWORK:-signet}"
ESPLORA_URL="${ESPLORA_URL:-https://esplora.signet.2nd.dev/}"
CHAIN_SOURCE_ID="${CHAIN_SOURCE_ID:-2nddev-signet-esplora}"
MEMBERS=5
THRESHOLD="${THRESHOLD:-3}"
PORT_START="${PORT_START:-21885}"
PROMETHEUS_PORT_START="${PROMETHEUS_PORT_START:-21985}"
SESSION_PREFIX="${SESSION_PREFIX:-cdk-iroh-fed-operator}"
WALLET_SESSION="${WALLET_SESSION:-cdk-iroh-fed-wallet}"
REQUIRE_UNIQUE_MACHINES="${REQUIRE_UNIQUE_MACHINES:-1}"
STOP_LEGACY_SESSION="${STOP_LEGACY_SESSION:-0}"
STOP_PRIOR_MULTI_OPERATOR_SESSIONS="${STOP_PRIOR_MULTI_OPERATOR_SESSIONS:-0}"
PRIOR_SESSION_PREFIX="${PRIOR_SESSION_PREFIX:-cdk-fed-operator}"
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
SSH_OPTIONS=(
    -o BatchMode=yes
    -o ConnectTimeout=30
    -o ConnectionAttempts=3
    -o ServerAliveInterval=15
    -o ServerAliveCountMax=4
)
HOSTS=()
IROH_URLS=()
STAGING_DIR=""
MAX_DKG_TOTAL_RSS_KB=0
MAX_DKG_MEMBER_RSS_KB=0

usage() {
    cat <<'EOF'
Deploy and exercise the invitation-based five-operator federation setup UX on
one Nocean server. Every operator uses a separate protected directory and named
tmux session. Public wallet traffic, setup, DKG, consensus, checkpoint,
catch-up, and operator traffic all use persistent Iroh endpoints. HTTP is
disabled. Identities and private configuration are generated only inside each
operator environment. All DKG services start concurrently.

Usage:
  OPERATOR_HOSTS_CSV=host1,host2,host3,host4,host5 \
    scripts/deploy_nocean_multi_operator_federation.sh

The default host list repeats the existing `nocean` alias for the accepted
single-server, five-operator topology. When the legacy deployment owns the
public ports, set STOP_LEGACY_SESSION=1. For a strict future multi-machine
production drill, supply five SSH aliases and set REQUIRE_UNIQUE_MACHINES=5.

Important overrides:
  OPERATOR_HOSTS_CSV       Exactly five comma-separated SSH aliases.
  REMOTE_ROOT              Safe relative root created on every operator host.
  REQUIRE_UNIQUE_MACHINES  Minimum distinct /etc/machine-id values (default 1).
  STOP_LEGACY_SESSION      Stop cdk-federation before taking its public ports.
  STOP_PRIOR_MULTI_OPERATOR_SESSIONS
                            Stop PRIOR_SESSION_PREFIX-op-1..5 before startup.
  PRIOR_SESSION_PREFIX     Previous operator session prefix to stop.
  PUBLIC_TRANSPORT         Must be iroh for this all-Iroh drill.
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

scp_retry() {
    local attempt
    for attempt in 1 2 3; do
        if scp -q "${SSH_OPTIONS[@]}" "$@"; then
            return 0
        fi
        if ((attempt < 3)); then
            sleep "$attempt"
        fi
    done
    die "SCP transfer failed after three bounded attempts"
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
    if [[ "$PUBLIC_TRANSPORT" == "iroh" ]]; then
        federation_api_url "$1"
    else
        printf 'https://cdk-%s.%s' "$1" "$PUBLIC_BASE_DOMAIN"
    fi
}

federation_api_url() {
    printf '%s' "${IROH_URLS[$(($1 - 1))]}"
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

wait_for_public_member() {
    local member="$1" expectation="$2" deadline
    deadline=$((SECONDS + STATUS_TIMEOUT_SECS))
    while ((SECONDS < deadline)); do
        if remote_wallet 1 probe-url "$(public_mint_url "$member")" "$member" \
            >/dev/null 2>&1; then
            [[ "$expectation" == "ready" ]] && return 0
        elif [[ "$expectation" == "down" ]]; then
            return 0
        fi
        sleep "$STATUS_POLL_SECS"
    done
    die "Iroh public member $member did not become $expectation before the timeout"
}

wait_for_setup_service() {
    local member="$1" label="$2" deadline
    deadline=$((SECONDS + STATUS_TIMEOUT_SECS))
    while ((SECONDS < deadline)); do
        if remote_operator "$member" status-json "$label" >/dev/null 2>&1; then
            return 0
        fi
        sleep "$STATUS_POLL_SECS"
    done
    die "authenticated Iroh setup service did not become ready before the timeout"
}

wait_for_public_runtime() {
    local deadline member ready
    deadline=$((SECONDS + STATUS_TIMEOUT_SECS))
    while ((SECONDS < deadline)); do
        ready=0
        for member in $(seq 1 "$MEMBERS"); do
            if remote_wallet 1 probe-url "$(public_mint_url "$member")" "$member" \
                >/dev/null 2>&1; then
                ready=$((ready + 1))
            fi
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
    scp_retry "$source_host:$source_path" "$local_invitation"
    chmod 600 "$local_invitation"
    local command
    printf -v command 'mkdir -p %q && chmod 700 %q' "$destination_dir" "$destination_dir"
    ssh "${SSH_OPTIONS[@]}" "$destination_host" "$command"
    scp_retry "$local_invitation" \
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
        printf 'ESPLORA_URL=%q\n' "$ESPLORA_URL"
        printf 'CHAIN_SOURCE_ID=%q\n' "$CHAIN_SOURCE_ID"
        printf 'PUBLIC_MINT_URL=%q\n' "$(public_mint_url "$member")"
        if [[ "$PUBLIC_TRANSPORT" == "iroh" ]]; then
            printf 'HTTP_ENABLED=false\n'
        else
            printf 'HTTP_ENABLED=true\n'
        fi
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
                --features sqlite,info-page,federation,iroh,prometheus
            cargo build --locked --release -p cdk-cli \
                --no-default-features --features federation
            cargo build --locked --release \
                --manifest-path scripts/fed-cdk-tui/Cargo.toml
            "$CARGO_TARGET_DIR/release/cdk-mintd" federation setup create --help >/dev/null
            "$CARGO_TARGET_DIR/release/cdk-cli" federation add --help >/dev/null
            "$CARGO_TARGET_DIR/release/fed-cdk-tui" check --help >/dev/null
        '
}

install_operators() {
    local mintd="$BUILD_TARGET_DIR/release/cdk-mintd"
    local cli="$BUILD_TARGET_DIR/release/cdk-cli"
    local tui="$BUILD_TARGET_DIR/release/fed-cdk-tui"
    local mintd_hash cli_hash tui_hash member host dir env_file command iroh_url
    [[ -x "$mintd" && -x "$cli" && -x "$tui" ]] \
        || die "Linux build did not produce every required binary"
    mintd_hash="$(sha256_file "$mintd")"
    cli_hash="$(sha256_file "$cli")"
    tui_hash="$(sha256_file "$tui")"
    for member in $(seq 1 "$MEMBERS"); do
        host="$(member_host "$member")"
        dir="$(operator_dir "$member")"
        printf -v command \
            'test ! -d %q || { echo "error: operator ceremony state already exists" >&2; exit 1; }; mkdir -p %q %q && chmod 700 %q' \
            "$dir/ceremonies" "$dir/bin" "$dir/scripts" "$dir"
        ssh "${SSH_OPTIONS[@]}" "$host" "$command"
        printf -v command \
            'test -x %q && test "$(sha256sum %q | awk '\''{print $1}'\'')" = %q' \
            "$dir/bin/cdk-mintd" "$dir/bin/cdk-mintd" "$mintd_hash"
        if ! ssh "${SSH_OPTIONS[@]}" "$host" "$command"; then
            scp_retry "$mintd" "$host:$dir/bin/cdk-mintd.upload"
            printf -v command \
                'test "$(sha256sum %q | awk '\''{print $1}'\'')" = %q && chmod 755 %q && mv %q %q' \
                "$dir/bin/cdk-mintd.upload" "$mintd_hash" \
                "$dir/bin/cdk-mintd.upload" "$dir/bin/cdk-mintd.upload" "$dir/bin/cdk-mintd"
            ssh "${SSH_OPTIONS[@]}" "$host" "$command"
        fi
        if ((member == 1)); then
            printf -v command \
                'test -x %q && test "$(sha256sum %q | awk '\''{print $1}'\'')" = %q' \
                "$dir/bin/fed-cdk-tui" "$dir/bin/fed-cdk-tui" "$tui_hash"
            if ! ssh "${SSH_OPTIONS[@]}" "$host" "$command"; then
                scp_retry "$tui" "$host:$dir/bin/fed-cdk-tui.upload"
                printf -v command \
                    'test "$(sha256sum %q | awk '\''{print $1}'\'')" = %q && chmod 755 %q && mv %q %q' \
                    "$dir/bin/fed-cdk-tui.upload" "$tui_hash" \
                    "$dir/bin/fed-cdk-tui.upload" "$dir/bin/fed-cdk-tui.upload" \
                    "$dir/bin/fed-cdk-tui"
                ssh "${SSH_OPTIONS[@]}" "$host" "$command"
            fi
        fi
        printf -v command \
            'set -eu; cd %q; mkdir -p work; chmod 700 work; bin/cdk-mintd --work-dir work iroh init' \
            "$dir"
        iroh_url="$(ssh "${SSH_OPTIONS[@]}" "$host" "$command")"
        [[ "$iroh_url" =~ ^iroh://[a-z0-9]+$ ]] \
            || die "operator $member returned an invalid Iroh endpoint URL"
        IROH_URLS[$((member - 1))]="$iroh_url"
        env_file="$STAGING_DIR/operator-$member.env"
        write_operator_env "$member" "$env_file"
        printf -v command \
            'test -x %q && test "$(sha256sum %q | awk '\''{print $1}'\'')" = %q' \
            "$dir/bin/cdk-cli" "$dir/bin/cdk-cli" "$cli_hash"
        if ! ssh "${SSH_OPTIONS[@]}" "$host" "$command"; then
            scp_retry "$cli" "$host:$dir/bin/cdk-cli.upload"
            printf -v command \
                'test "$(sha256sum %q | awk '\''{print $1}'\'')" = %q && chmod 755 %q && mv %q %q' \
                "$dir/bin/cdk-cli.upload" "$cli_hash" \
                "$dir/bin/cdk-cli.upload" "$dir/bin/cdk-cli.upload" "$dir/bin/cdk-cli"
            ssh "${SSH_OPTIONS[@]}" "$host" "$command"
        fi
        scp_retry "$OPERATOR_SCRIPT" \
            "$host:$dir/scripts/nocean_federation_operator.sh.upload"
        scp_retry "$WALLET_SCRIPT" \
            "$host:$dir/scripts/nocean_federation_wallet_smoke.sh.upload"
        scp_retry "$env_file" "$host:$dir/operator.env.upload"
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
        topology_class="single-host-five-operator"
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
    if [[ "$STOP_PRIOR_MULTI_OPERATOR_SESSIONS" == "1" ]]; then
        for member in $(seq 1 "$MEMBERS"); do
            remote_simple "$member" \
                "tmux kill-session -t $PRIOR_SESSION_PREFIX-op-$member 2>/dev/null || true"
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
    scp_retry \
        "$(member_host 1):$(operator_dir 1)/ceremonies/active/setup-state/activated/federation-public.json" \
        "$public_copy"
    setup_authorization="$(jq -r '.setup_authorization' "$public_copy")"
    [[ -n "$setup_authorization" && "$setup_authorization" != "null" ]] \
        || die "public config has no setup authorization"
    jq -e '.members | to_entries | all(
        .value.public_mint_url | startswith("iroh://")
    )' "$public_copy" >/dev/null \
        || die "public config contains a non-Iroh wallet URL"
    jq -e '.members | to_entries | all(
        .value.public_mint_url == .value.federation_api_url
    )' "$public_copy" >/dev/null \
        || die "public and private member routes do not use the same Iroh endpoint"
}

verify_runtime_consensus() {
    local member
    for member in $(seq 1 "$MEMBERS"); do
        remote_operator "$member" runtime-status >/dev/null
    done
}

verify_http_listeners_disabled() {
    local member
    for member in $(seq 1 "$MEMBERS"); do
        remote_operator "$member" http-listener-disabled
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
        crates/cdk-mintd/src/federation_setup_command.rs \
        crates/cdk-mintd/src/lib.rs \
        crates/cdk/src/wallet/federation.rs \
        scripts/fed-cdk-tui \
        scripts/deploy_nocean_multi_operator_federation.sh \
        scripts/nocean_federation_operator.sh \
        scripts/nocean_federation_wallet_smoke.sh)" ]]; then
        source_dirty=false
    else
        source_dirty=true
    fi
    if [[ "$topology_class" == "five-machine" ]]; then
        remaining_limit="none for the separate parent-plan multi-host gate"
    else
        remaining_limit="accepted for the multi-operator setup milestone; not evidence for the separate parent-plan multi-host gate"
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
        --arg tls_model "public wallet, setup, DKG, and private federation traffic over authenticated Iroh N0; HTTP disabled" \
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
                tls_model: $tls_model,
                public_wallet_transport: "iroh",
                public_http_enabled: false,
                setup_and_dkg_http_enabled: false,
                configured_tcp_listener_absence_verified: true,
                private_federation_transport: "iroh",
                private_https_or_dns_required: false
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
            wallet_smoke: {
                federation_import_over_iroh: true,
                threshold_mint_info_over_iroh: true,
                empty_balance: true,
                paid_operation: false
            },
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
[[ "$STOP_PRIOR_MULTI_OPERATOR_SESSIONS" == "0" || "$STOP_PRIOR_MULTI_OPERATOR_SESSIONS" == "1" ]] \
    || die "STOP_PRIOR_MULTI_OPERATOR_SESSIONS must be 0 or 1"
[[ "$REMOTE_ROOT" =~ ^[A-Za-z0-9._/-]+$ && "$REMOTE_ROOT" != /* ]] \
    || die "REMOTE_ROOT must be a safe relative path"
[[ "/$REMOTE_ROOT/" != *"/../"* ]] || die "REMOTE_ROOT must not contain '..'"
[[ "$PUBLIC_BASE_DOMAIN" =~ ^[A-Za-z0-9.-]+$ ]] \
    || die "PUBLIC_BASE_DOMAIN contains unsupported characters"
[[ "$PUBLIC_TRANSPORT" == "iroh" ]] \
    || die "PUBLIC_TRANSPORT must be iroh for the all-Iroh deployment"
[[ "$SESSION_PREFIX" =~ ^[A-Za-z0-9._-]+$ ]] || die "SESSION_PREFIX is invalid"
[[ "$WALLET_SESSION" =~ ^[A-Za-z0-9._-]+$ ]] || die "WALLET_SESSION is invalid"
[[ "$PRIOR_SESSION_PREFIX" =~ ^[A-Za-z0-9._-]+$ ]] \
    || die "PRIOR_SESSION_PREFIX is invalid"
[[ "$NETWORK" == "signet" ]] || die "the Nocean Iroh drill currently requires NETWORK=signet"

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
wait_for_setup_service 1 failed
remote_operator 1 http-listener-disabled
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
wait_for_setup_service 1 active
remote_operator 1 http-listener-disabled
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
remote_operator 1 service-stopped
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
verify_http_listeners_disabled

printf 'Activating identical final configs locally and starting runtimes...\n'
service_all stop
run_action_all active activate
verify_public_config
service_all start active runtime
wait_for_public_runtime
verify_runtime_consensus
verify_http_listeners_disabled

printf 'Running the normal federation wallet smoke in its own tmux session...\n'
remote_wallet 1 session-run-wait smoke

printf 'Exercising a one-member outage and catch-up...\n'
remote_operator 5 service-stop
wait_for_public_member 5 down
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
