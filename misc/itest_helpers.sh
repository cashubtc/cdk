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
# The extracted directory also contains the pre-built harness binaries
# (start_regtest, start_fake_mint, start_fake_auth_mint, start_regtest_mints)
# so we prepend it to PATH: run_bin* then use binaries built from the SAME
# revision as the tests instead of whatever the current shell provides, and
# never fall back to compiling with cargo.
#
# Layout of the extracted dir:
#   target/nextest/binaries-metadata.json
#   target/nextest/cargo-metadata.json
#   target/nextest/libdirs/host/...   (libstd etc. for the test binaries)
#   target/debug/...                  (test + harness binaries)
#
# Override the extraction root with CDK_NEXTEST_EXTRACT_ROOT (default:
# ${TMPDIR:-/tmp}/cdk-nextest-extract). The directory is keyed by the archive,
# so a new archive gets a fresh directory; stale ones can simply be deleted.
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

# Extract the archive once and prepend its harness binaries to PATH.
# Idempotent: does nothing if the archive was already extracted.
prepare_nextest_archive() {
    if [ -z "${CDK_ITEST_ARCHIVE:-}" ] || [ ! -f "${CDK_ITEST_ARCHIVE:-}" ]; then
        return 0
    fi

    local dir metadata
    dir=$(cdk_nextest_extract_dir "$CDK_ITEST_ARCHIVE")
    metadata="$dir/target/nextest/binaries-metadata.json"

    if [ ! -f "$metadata" ]; then
        # No complete extraction: extract under a lock so concurrent callers
        # (e.g. parallel CI jobs sharing CDK_NEXTEST_EXTRACT_ROOT) don't wipe
        # each other's in-flight extraction. mkdir is the lock primitive: it
        # is atomic on POSIX and always available (flock(1) is not present in
        # `nix develop -i` shells or on macOS).
        case "$dir" in
            "" | "/" | "${HOME:-}")
                echo "Refusing to clean suspicious extract dir: '$dir'" >&2
                return 1
                ;;
        esac
        local root tmp lock waited rc
        root="${CDK_NEXTEST_EXTRACT_ROOT:-${TMPDIR:-/tmp}/cdk-nextest-extract}"
        tmp="$dir.tmp.$$"
        lock="$root/.extract.lock.d"
        mkdir -p "$root"
        waited=0
        while ! mkdir "$lock" 2>/dev/null; do
            # A crashed holder never releases a mkdir lock; break it once it
            # is older than 30 min (extraction takes a few minutes).
            if find "$lock" -maxdepth 0 -mmin +30 2>/dev/null | grep -q .; then
                echo "Breaking stale nextest extraction lock: $lock" >&2
                rmdir "$lock" 2>/dev/null || true
                continue
            fi
            if [ "$waited" -ge 900 ]; then
                echo "Timed out waiting for nextest extraction lock" >&2
                return 1
            fi
            sleep 5
            waited=$((waited + 5))
        done
        rc=0
        # This root persists across runs (on CI it is a fixed host path),
        # and each archive revision adds ~19 GB. Prune stale extractions
        # and aborted tmp dirs. Trade-off: a job re-run still executing
        # from a >3-day-old extraction on this host could lose it mid-run.
        find "$root" -mindepth 1 -maxdepth 1 -type d -name '*-itest-archive' -mtime +3 -exec rm -rf {} + 2>/dev/null || true
        find "$root" -mindepth 1 -maxdepth 1 -type d -name '*.tmp.*' -mtime +1 -exec rm -rf {} + 2>/dev/null || true
        # Re-check under the lock: another process may have finished.
        if [ ! -f "$metadata" ]; then
            rm -rf "$tmp"
            mkdir -p "$tmp"
            echo "Extracting nextest archive to $dir (one-time per archive)..."
            # --no-run: extract and enumerate tests without running any.
            # --workspace-remap . is required even for --no-run: nextest checks
            # that the workspace manifest exists when reusing a build.
            if cargo nextest run --archive-file "$CDK_ITEST_ARCHIVE" --extract-to "$tmp" --workspace-remap . --no-run; then
                # Publish atomically: no half-extracted dir is ever visible.
                rm -rf "$dir"
                mv "$tmp" "$dir"
            else
                echo "Failed to extract nextest archive: $CDK_ITEST_ARCHIVE" >&2
                df -h "$root" >&2 || true
                rm -rf "$tmp"
                rc=1
            fi
        fi
        rmdir "$lock" 2>/dev/null || true
        [ "$rc" -eq 0 ] || return 1
    fi

    # Pre-built harness binaries from the archive (same revision as the tests).
    case ":$PATH:" in
        *":$dir/target/debug:"*) ;;
        *) export PATH="$dir/target/debug:$PATH" ;;
    esac

    # Test binaries record the rustc libdir from build time; prefer the libdir
    # shipped inside the archive so they still start if the original toolchain
    # was garbage-collected from the nix store.
    if [ -d "$dir/target/nextest/libdirs/host" ]; then
        case ":${LD_LIBRARY_PATH:-}:" in
            *":$dir/target/nextest/libdirs/host:"*) ;;
            *) export LD_LIBRARY_PATH="$dir/target/nextest/libdirs/host${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" ;;
        esac
    fi
}

# ========================================
# Helper: run a binary from $PATH (Nix pre-built or nextest archive) or fall
# back to cargo run
# ========================================
run_bin() {
    local bin_name="$1"
    shift
    prepare_nextest_archive
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
    prepare_nextest_archive
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
    prepare_nextest_archive
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
