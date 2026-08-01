#!/usr/bin/env bash
# Shared helper functions for integration test scripts.
# Source this file from each test script:
#   source "$(dirname "$0")/itest_helpers.sh"

# ========================================
# Nextest archive handling
#
# CDK_ITEST_ARCHIVE points to a `cargo nextest archive` tarball (usually the
# `.#itest-archive` Nix build, ~3.4 GB zst / ~19 GB extracted).
# `cargo nextest run --archive-file` re-extracts the WHOLE archive into a
# fresh temp dir on every invocation, which dominates startup time when a
# suite runs several test binaries. To avoid that we extract exactly once per
# archive into a stable directory and then point nextest at the extracted
# metadata directly (--cargo-metadata/--binaries-metadata/--target-dir-remap),
# which skips extraction entirely on subsequent runs.
#
# Layout of the extracted dir:
#   target/nextest/binaries-metadata.json
#   target/nextest/cargo-metadata.json
#   target/debug/deps/...             (test binaries)
#
# Override the extraction root with CDK_NEXTEST_EXTRACT_ROOT (default:
# ${TMPDIR:-/tmp}/cdk-nextest-extract). The directory is keyed by the archive,
# so a new archive gets a fresh directory. Directories unused for three days
# are pruned while a new archive is extracted.
# ========================================

# Echo the extraction directory for the given archive path.
cdk_nextest_extract_dir() {
    local archive="$1"
    local key
    case "$archive" in
        /nix/store/*)
            # Store paths are already unique per build; use the store dir name.
            key=$(basename "$(dirname "$archive")")
            ;;
        *)
            # Key on path + size + mtime so a replaced file gets a fresh dir.
            local stamp
            stamp=$(stat -c '%s-%Y' "$archive" 2>/dev/null || stat -f '%z-%m' "$archive" 2>/dev/null || echo "0-0")
            key=$(printf '%s:%s' "$archive" "$stamp" | sha256sum | cut -c1-16)
            ;;
    esac
    printf '%s/%s\n' "${CDK_NEXTEST_EXTRACT_ROOT:-${TMPDIR:-/tmp}/cdk-nextest-extract}" "$key"
}

# Extract the archive once for reuse by run_test.
# Idempotent: does nothing if the archive was already extracted.
prepare_nextest_archive() {
    if [ -z "${CDK_ITEST_ARCHIVE:-}" ] || [ ! -f "${CDK_ITEST_ARCHIVE:-}" ]; then
        return 0
    fi

    local root dir metadata
    root="${CDK_NEXTEST_EXTRACT_ROOT:-${TMPDIR:-/tmp}/cdk-nextest-extract}"
    case "$root" in
        /*) ;;
        *)
            echo "Nextest extraction root must be an absolute path: '$root'" >&2
            return 1
            ;;
    esac
    case "$root" in
        "" | "/" | "${HOME:-}")
            echo "Refusing to use suspicious nextest extraction root: '$root'" >&2
            return 1
            ;;
    esac

    dir=$(cdk_nextest_extract_dir "$CDK_ITEST_ARCHIVE")
    metadata="$dir/target/nextest/binaries-metadata.json"
    mkdir -p "$root"

    if [ ! -f "$metadata" ]; then
        # No complete extraction: extract under a lock so concurrent callers
        # sharing CDK_NEXTEST_EXTRACT_ROOT don't prune or publish over each
        # other. mkdir is the lock primitive because it is atomic on POSIX.
        local lock waited
        lock="$root/.extract.lock.d"
        waited=0
        while ! mkdir "$lock" 2>/dev/null; do
            # SIGKILL or a host crash can leave the lock behind. Only break a
            # lock much older than the longest expected extraction.
            if find "$lock" -maxdepth 0 -mmin +360 2>/dev/null | grep -q .; then
                echo "Breaking stale nextest extraction lock: $lock" >&2
                rmdir "$lock" 2>/dev/null || true
                continue
            fi
            if [ "$waited" -ge 1800 ]; then
                echo "Timed out waiting for nextest extraction lock" >&2
                return 1
            fi
            sleep 5
            waited=$((waited + 5))
        done

        if ! (
            tmp=""
            cleanup_nextest_extraction() {
                if [ -n "$tmp" ]; then
                    rm -rf "$tmp"
                fi
                rmdir "$lock" 2>/dev/null || true
            }
            trap cleanup_nextest_extraction EXIT HUP INT TERM

            # This root persists across CI runs, and each archive revision adds
            # about 19 GB. prepare_nextest_archive touches a directory whenever
            # a job starts using it, so this does not prune an active CI run.
            find "$root" -mindepth 1 -maxdepth 1 -type d ! -name '.*' -mtime +3 -exec rm -rf {} + 2>/dev/null || true

            # Re-check under the lock: another process may have finished.
            if [ ! -f "$metadata" ]; then
                tmp=$(mktemp -d "$root/.extract.XXXXXX") || exit 1
                echo "Extracting nextest archive to $dir (one-time per archive)..."
                # --no-run extracts and validates the archived test metadata.
                if ! cargo nextest run \
                    --archive-file "$CDK_ITEST_ARCHIVE" \
                    --extract-to "$tmp" \
                    --workspace-remap . \
                    --no-run; then
                    echo "Failed to extract nextest archive: $CDK_ITEST_ARCHIVE" >&2
                    df -h "$root" >&2 || true
                    exit 1
                fi

                # Publish atomically: no half-extracted directory is visible.
                rm -rf "$dir"
                mv "$tmp" "$dir"
                tmp=""
            fi
        ); then
            return 1
        fi
    fi

    if [ ! -f "$metadata" ]; then
        echo "Nextest archive extraction did not produce metadata: $metadata" >&2
        return 1
    fi

    # Refresh the directory mtime so pruning cannot remove an extraction used
    # by a currently starting job. CI jobs are bounded to well under three days.
    touch "$dir"
}

# ========================================
# Helper: run a binary from $PATH (Nix pre-built) or fall back to cargo run
# ========================================
run_bin() {
    local bin_name="$1"
    shift
    if command -v "$bin_name" &>/dev/null; then
        echo "Using pre-built binary: $bin_name"
        "$bin_name" "$@"
    else
        echo "Pre-built binary not found, falling back to: cargo run --bin $bin_name"
        cargo run --bin "$bin_name" -- "$@"
    fi
}

run_bin_bg() {
    local bin_name="$1"
    shift
    if command -v "$bin_name" &>/dev/null; then
        echo "Using pre-built binary: $bin_name"
        "$bin_name" "$@" &
    else
        echo "Pre-built binary not found, falling back to: cargo run --bin $bin_name"
        cargo run --bin "$bin_name" -- "$@" &
    fi
}

# Helper: run cdk-mintd from $PATH (Nix pre-built) or fall back to cargo run
# with grpc-processor feature
run_mintd_bg() {
    if command -v cdk-mintd &>/dev/null; then
        echo "Using pre-built binary: cdk-mintd"
        cdk-mintd &
    else
        echo "Pre-built cdk-mintd not found, falling back to cargo run"
        cargo run --bin cdk-mintd --no-default-features --features grpc-processor &
    fi
}

# Helper: run cargo nextest from the (already extracted) archive if available,
# or fall back to cargo test.
# For nextest: translates cargo test conventions and strips '--' separators.
#
# Usage: run_test <test_name> [extra cargo-test args...]
run_test() {
    local test_name="$1"
    shift
    if [ -n "${CDK_ITEST_ARCHIVE:-}" ] && [ -f "${CDK_ITEST_ARCHIVE:-}" ]; then
        if ! prepare_nextest_archive; then
            echo "nextest archive unavailable, cannot run '$test_name'" >&2
            return 1
        fi
        local extract_dir
        extract_dir=$(cdk_nextest_extract_dir "$CDK_ITEST_ARCHIVE")

        # Build nextest args, translating cargo test conventions
        local nextest_args=()
        local args=("$@")
        local i=0
        while [ "$i" -lt "${#args[@]}" ]; do
            local arg="${args[$i]}"
            if [ "$arg" = "--" ]; then
                i=$((i + 1))
                continue
            fi
            if [ "$arg" = "--nocapture" ]; then
                nextest_args+=("--no-capture")
            elif [ "$arg" = "--test-threads" ]; then
                i=$((i + 1))
                if [ "$i" -lt "${#args[@]}" ]; then
                    nextest_args+=("-j" "${args[$i]}")
                fi
            elif [[ "$arg" == --test-threads=* ]]; then
                nextest_args+=("-j" "${arg#--test-threads=}")
            else
                nextest_args+=("$arg")
            fi
            i=$((i + 1))
        done
        echo "Running test '$test_name' from nextest archive"
        # Point nextest at the extracted metadata directly: no re-extraction.
        cargo nextest run \
            --cargo-metadata "$extract_dir/target/nextest/cargo-metadata.json" \
            --binaries-metadata "$extract_dir/target/nextest/binaries-metadata.json" \
            --workspace-remap . \
            --target-dir-remap "$extract_dir/target" \
            -E "binary(/^${test_name}$/)" "${nextest_args[@]}"
    else
        echo "Running test '$test_name' via cargo test"
        cargo test -p cdk-integration-tests --test "$test_name" "$@"
    fi
}
