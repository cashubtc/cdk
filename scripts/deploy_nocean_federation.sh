#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"

SSH_HOST="${SSH_HOST:-nocean}"
REMOTE_DIR="${REMOTE_DIR:-cdk-federation}"
PUBLIC_BASE_DOMAIN="${PUBLIC_BASE_DOMAIN:-nocean.xyz}"
STATUS_TIMEOUT_SECS="${STATUS_TIMEOUT_SECS:-300}"
STATUS_POLL_SECS="${STATUS_POLL_SECS:-3}"
DOCKER_PLATFORM="${DOCKER_PLATFORM:-linux/amd64}"
BUILD_TARGET_DIR="$REPO_ROOT/target/linux-x86_64"
CARGO_REGISTRY_VOLUME="${CARGO_REGISTRY_VOLUME:-cdk-nocean-cargo-registry}"

usage() {
    cat <<'EOF'
Build the CDK federation binaries locally for x86_64 Linux, deploy them to the
Nocean server with SCP, restart (or start) the federation, and verify health.

Usage:
  scripts/deploy_nocean_federation.sh

Environment overrides:
  SSH_HOST               SSH host alias (default: nocean)
  REMOTE_DIR              Directory below the remote home (default: cdk-federation)
  PUBLIC_BASE_DOMAIN      Base domain for cdk-1 through cdk-5 (default: nocean.xyz)
  STATUS_TIMEOUT_SECS     Overall remote readiness timeout (default: 300)
  STATUS_POLL_SECS        Delay between status attempts (default: 3)
  DOCKER_PLATFORM         Docker build platform (default: linux/amd64)
  CARGO_REGISTRY_VOLUME   Docker volume used to cache the Cargo registry
  RUST_IMAGE              Docker Rust image (default: rust:<toolchain>-bookworm)

The script does not erase federation state or rerun setup when finalized member
configs already exist. It preserves the previous binaries as *.previous.
EOF
}

die() {
    echo "error: $*" >&2
    exit 1
}

require_command() {
    command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

sha256_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

case "${1:-}" in
    -h|--help)
        usage
        exit 0
        ;;
    "") ;;
    *)
        usage >&2
        die "unexpected argument: $1"
        ;;
esac

require_command docker
require_command scp
require_command ssh
require_command awk

[[ "$REMOTE_DIR" =~ ^[A-Za-z0-9._/-]+$ ]] \
    || die "REMOTE_DIR must be a relative path containing only letters, numbers, '.', '_', '-', and '/'"
[[ "$REMOTE_DIR" != /* ]] || die "REMOTE_DIR must be relative to the remote home directory"
[[ "/$REMOTE_DIR/" != *"/../"* ]] || die "REMOTE_DIR must not contain '..' path segments"
[[ "$PUBLIC_BASE_DOMAIN" =~ ^[A-Za-z0-9.-]+$ ]] \
    || die "PUBLIC_BASE_DOMAIN contains unsupported characters"
[[ "$STATUS_TIMEOUT_SECS" =~ ^[1-9][0-9]*$ ]] \
    || die "STATUS_TIMEOUT_SECS must be a positive integer"
[[ "$STATUS_POLL_SECS" =~ ^[1-9][0-9]*$ ]] \
    || die "STATUS_POLL_SECS must be a positive integer"

RUST_VERSION="$({
    awk -F '"' '/^[[:space:]]*channel[[:space:]]*=/ { print $2; exit }' \
        "$REPO_ROOT/rust-toolchain.toml"
} || true)"
[[ -n "$RUST_VERSION" ]] || die "could not read the Rust channel from rust-toolchain.toml"
RUST_IMAGE="${RUST_IMAGE:-rust:${RUST_VERSION}-bookworm}"

MINTD="$BUILD_TARGET_DIR/release/cdk-mintd"
CLI="$BUILD_TARGET_DIR/release/cdk-cli"

echo "Building cdk-mintd and cdk-cli locally for $DOCKER_PLATFORM with $RUST_IMAGE..."
docker run --rm \
    --platform "$DOCKER_PLATFORM" \
    --env "RUSTUP_TOOLCHAIN=$RUST_VERSION" \
    --mount "type=bind,src=$REPO_ROOT,dst=/work" \
    --mount "type=volume,src=$CARGO_REGISTRY_VOLUME,dst=/usr/local/cargo/registry" \
    --workdir /work \
    "$RUST_IMAGE" \
    bash -c '
        set -euo pipefail
        apt-get update >/dev/null
        apt-get install -y --no-install-recommends protobuf-compiler >/dev/null
        export CARGO_TARGET_DIR=/work/target/linux-x86_64
        cargo build --locked --release -p cdk-mintd \
            --no-default-features \
            --features fakewallet,sqlite,info-page,federation
        cargo build --locked --release -p cdk-cli \
            --no-default-features \
            --features federation
        "$CARGO_TARGET_DIR/release/cdk-mintd" --version
        "$CARGO_TARGET_DIR/release/cdk-mintd" federation compare-digests --help >/dev/null
        "$CARGO_TARGET_DIR/release/cdk-cli" --version
        "$CARGO_TARGET_DIR/release/cdk-cli" federation add --help >/dev/null
    '

[[ -x "$MINTD" ]] || die "build did not produce $MINTD"
[[ -x "$CLI" ]] || die "build did not produce $CLI"

MINTD_HASH="$(sha256_file "$MINTD")"
CLI_HASH="$(sha256_file "$CLI")"
MINTD_UPLOAD="cdk-mintd.upload-$MINTD_HASH"
CLI_UPLOAD="cdk-cli.upload-$CLI_HASH"

echo "Uploading locally built binaries with SCP..."
scp "$MINTD" "$SSH_HOST:$REMOTE_DIR/bin/$MINTD_UPLOAD"
scp "$CLI" "$SSH_HOST:$REMOTE_DIR/bin/$CLI_UPLOAD"

echo "Installing binaries, starting the federation, and checking status..."
ssh "$SSH_HOST" bash -s -- \
    "$REMOTE_DIR" \
    "$MINTD_HASH" \
    "$CLI_HASH" \
    "$MINTD_UPLOAD" \
    "$CLI_UPLOAD" \
    "$PUBLIC_BASE_DOMAIN" \
    "$STATUS_TIMEOUT_SECS" \
    "$STATUS_POLL_SECS" <<'REMOTE_SCRIPT'
set -Eeuo pipefail

REMOTE_DIR="$1"
MINTD_HASH="$2"
CLI_HASH="$3"
MINTD_UPLOAD="$4"
CLI_UPLOAD="$5"
PUBLIC_BASE_DOMAIN="$6"
STATUS_TIMEOUT_SECS="$7"
STATUS_POLL_SECS="$8"

cd "$HOME/$REMOTE_DIR"

for command in sha256sum tmux curl timeout awk find; do
    command -v "$command" >/dev/null 2>&1 || {
        echo "error: required remote command not found: $command" >&2
        exit 1
    }
done

for required in \
    "bin/$MINTD_UPLOAD" \
    "bin/$CLI_UPLOAD" \
    scripts/start-federation.sh \
    scripts/restart-runtime.sh
do
    [[ -e "$required" ]] || {
        echo "error: missing remote deployment file: $required" >&2
        exit 1
    }
done

final_config_count="$({
    find member-* -path '*/dkg-final/federation-public.json' -type f 2>/dev/null || true
} | wc -l | tr -d '[:space:]')"
case "$final_config_count" in
    5) ;;
    0)
        [[ -f config/dkg-setup-public.json ]] || {
            echo "error: no final configs or DKG setup config exist; run federation setup first" >&2
            exit 1
        }
        ;;
    *)
        echo "error: found $final_config_count final public configs; refusing a partial deployment" >&2
        exit 1
        ;;
esac

actual_mintd_hash="$(sha256sum "bin/$MINTD_UPLOAD" | awk '{print $1}')"
actual_cli_hash="$(sha256sum "bin/$CLI_UPLOAD" | awk '{print $1}')"
[[ "$actual_mintd_hash" == "$MINTD_HASH" ]] || {
    echo "error: uploaded cdk-mintd hash mismatch" >&2
    exit 1
}
[[ "$actual_cli_hash" == "$CLI_HASH" ]] || {
    echo "error: uploaded cdk-cli hash mismatch" >&2
    exit 1
}

chmod 755 "bin/$MINTD_UPLOAD" "bin/$CLI_UPLOAD"
"bin/$MINTD_UPLOAD" --version
"bin/$MINTD_UPLOAD" federation compare-digests --help >/dev/null
"bin/$CLI_UPLOAD" --version
"bin/$CLI_UPLOAD" federation add --help >/dev/null

if [[ -x bin/cdk-mintd ]]; then
    cp -p bin/cdk-mintd bin/cdk-mintd.previous
fi
if [[ -x bin/cdk-cli ]]; then
    cp -p bin/cdk-cli bin/cdk-cli.previous
fi
mv "bin/$MINTD_UPLOAD" bin/cdk-mintd
mv "bin/$CLI_UPLOAD" bin/cdk-cli

case "$final_config_count" in
    5)
        bash scripts/restart-runtime.sh
        ;;
    0)
        [[ -f config/dkg-setup-public.json ]] || {
            echo "error: no final configs or DKG setup config exist; run federation setup first" >&2
            exit 1
        }
        bash scripts/start-federation.sh
        ;;
esac

digest_output="$(mktemp)"
trap 'rm -f "$digest_output"' EXIT

tmux_is_healthy() {
    tmux has-session -t cdk-federation 2>/dev/null || return 1

    local panes member_processes
    panes="$(tmux list-panes -t cdk-federation \
        -F '#{pane_title} #{pane_current_command} dead=#{pane_dead}')"
    [[ "$(printf '%s\n' "$panes" | wc -l | tr -d '[:space:]')" == "6" ]] || return 1
    ! grep -q 'dead=1' <<<"$panes" || return 1
    member_processes="$(awk '$2 == "cdk-mintd" { count++ } END { print count + 0 }' <<<"$panes")"
    [[ "$member_processes" == "5" ]]
}

public_routes_are_healthy() {
    local member base info_status config_status old_config_status

    for member in 1 2 3 4 5; do
        base="https://cdk-$member.$PUBLIC_BASE_DOMAIN"
        info_status="$(curl -sS -o /dev/null -w '%{http_code}' --max-time 10 "$base/v1/info")" \
            || return 1
        config_status="$(curl -sS -o /dev/null -w '%{http_code}' --max-time 10 \
            "$base/federation/v1/config")" || return 1
        old_config_status="$(curl -sS -o /dev/null -w '%{http_code}' --max-time 10 \
            "$base/v1/config")" || return 1
        [[ "$info_status" == "200" ]] || return 1
        [[ "$config_status" == "200" ]] || return 1
        [[ "$old_config_status" == "404" ]] || return 1
    done
}

digests_are_converged() {
    [[ -f member-1/dkg-final/federation-public.json ]] || return 1
    [[ -f member-1/dkg-final/federation-private.json ]] || return 1

    timeout 30 bin/cdk-mintd federation compare-digests \
        --public-config member-1/dkg-final/federation-public.json \
        --private-config member-1/dkg-final/federation-private.json \
        >"$digest_output" 2>&1 \
        || return 1
    grep -q 'converged=true' "$digest_output" || return 1
    grep -q 'members=1,2,3,4,5 ready=5/5 sync_states=healthy' "$digest_output"
}

deadline=$((SECONDS + STATUS_TIMEOUT_SECS))
attempt=0
echo "Waiting up to $STATUS_TIMEOUT_SECS seconds for federation health..."
while (( SECONDS < deadline )); do
    attempt=$((attempt + 1))
    if tmux_is_healthy \
        && public_routes_are_healthy \
        && digests_are_converged
    then
        echo
        echo "Federation deployment is healthy."
        tmux list-panes -t cdk-federation \
            -F '#{pane_index} #{pane_title} #{pane_current_command} dead=#{pane_dead}'
        echo
        for member in 1 2 3 4 5; do
            echo "member-$member info=200 federation-config=200 old-config=404"
        done
        echo
        cat "$digest_output"
        echo
        sha256sum bin/cdk-mintd bin/cdk-cli
        exit 0
    fi

    if (( attempt % 10 == 0 )); then
        echo "Still waiting for all five members and signed digest convergence..."
    fi
    sleep "$STATUS_POLL_SECS"
done

echo "error: federation did not become healthy within $STATUS_TIMEOUT_SECS seconds" >&2
tmux list-panes -t cdk-federation \
    -F '#{pane_index} #{pane_title} #{pane_current_command} dead=#{pane_dead}' >&2 \
    || true
status_pane="$(tmux list-panes -t cdk-federation \
    -F '#{pane_index} #{pane_title}' 2>/dev/null \
    | awk '$2 == "status" { print $1; exit }')"
if [[ -n "$status_pane" ]]; then
    tmux capture-pane -p -J -t "cdk-federation:0.$status_pane" -S -80 >&2 || true
fi
cat "$digest_output" >&2 || true
exit 1
REMOTE_SCRIPT

echo
echo "Deployment completed successfully."
