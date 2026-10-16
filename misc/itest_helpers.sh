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
# so a new archive gets a fresh directory. On Linux, unused revisions are
# pruned before a new archive is extracted while active revisions are protected
# by shared flock leases.
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

nextest_lock_wait_seconds() {
    local wait_seconds=600

    if [ -n "${CDK_ITEST_SUITE_DEADLINE_EPOCH:-}" ]; then
        local remaining_seconds
        remaining_seconds=$((CDK_ITEST_SUITE_DEADLINE_EPOCH - $(date +%s)))
        if [ "$remaining_seconds" -le 0 ]; then
            return 1
        fi
        if [ "$remaining_seconds" -lt "$wait_seconds" ]; then
            wait_seconds=$remaining_seconds
        fi
    fi

    printf '%s\n' "$wait_seconds"
}

acquire_nextest_use_lease() {
    local root="$1"
    local dir="$2"
    local lease lease_wait_seconds

    if ! command -v flock >/dev/null 2>&1; then
        return
    fi
    if [ "${CDK_NEXTEST_LEASE_DIR:-}" = "$dir" ]; then
        return
    fi

    if [ -n "${CDK_NEXTEST_LEASE_FD:-}" ]; then
        exec {CDK_NEXTEST_LEASE_FD}>&-
    fi

    mkdir -p "$root/.leases"
    lease="$root/.leases/$(basename "$dir").lock"
    exec {CDK_NEXTEST_LEASE_FD}>"$lease"
    lease_wait_seconds=$(nextest_lock_wait_seconds) || {
        echo "Integration-test suite deadline reached while waiting for nextest extraction lease" >&2
        exec {CDK_NEXTEST_LEASE_FD}>&-
        unset CDK_NEXTEST_LEASE_FD
        return 1
    }
    if ! flock --shared --timeout "$lease_wait_seconds" "$CDK_NEXTEST_LEASE_FD"; then
        echo "Timed out after ${lease_wait_seconds}s waiting for nextest extraction lease: $dir" >&2
        exec {CDK_NEXTEST_LEASE_FD}>&-
        unset CDK_NEXTEST_LEASE_FD
        return 1
    fi
    CDK_NEXTEST_LEASE_DIR="$dir"
}

prune_unused_nextest_extractions() {
    local root="$1"
    local keep="$2"
    local candidate lease_fd

    for candidate in "$root"/*; do
        if [ ! -d "$candidate" ] || [ "$candidate" = "$keep" ]; then
            continue
        fi

        # A shared lease means another job is using this revision. An
        # exclusive non-blocking lease is released automatically even when a
        # job dies with SIGKILL, unlike a directory lock.
        exec {lease_fd}>"$root/.leases/$(basename "$candidate").lock" || continue
        if flock --exclusive --nonblock "$lease_fd"; then
            echo "Removing unused nextest extraction: $candidate"
            rm -rf "$candidate"
        fi
        exec {lease_fd}>&-
    done
}

extract_nextest_archive_locked() {
    local root="$1"
    local dir="$2"
    local metadata="$3"
    # This function runs in a dedicated subshell. Keep tmp in that shell's
    # scope so the EXIT trap can still read it after the function returns.
    tmp=""

    cleanup_nextest_extraction() {
        if [ -n "$tmp" ]; then
            rm -rf "$tmp"
        fi
    }
    trap cleanup_nextest_extraction EXIT HUP INT TERM

    # Re-check under the lock: another process may have finished.
    if [ -f "$metadata" ]; then
        return
    fi

    if command -v flock >/dev/null 2>&1; then
        prune_unused_nextest_extractions "$root" "$dir"
        # SIGKILL bypasses shell traps but releases flock locks. Remove any
        # unpublished extraction left by the previous lock owner.
        find "$root" -mindepth 1 -maxdepth 1 -type d -name '.extract.*' \
            -exec rm -rf {} +
    else
        # Portable fallback for development hosts without flock. Avoid
        # removing a recent extraction because this path has no usage leases.
        find "$root" -mindepth 1 -maxdepth 1 -type d ! -name '.*' \
            -mtime +3 -exec rm -rf {} + 2>/dev/null || true
    fi

    tmp=$(mktemp -d "$root/.extract.XXXXXX") || return 1
    echo "Extracting nextest archive to $dir (one-time per archive)..."
    # --no-run extracts and validates the archived test metadata.
    if ! cargo nextest run \
        --archive-file "$CDK_ITEST_ARCHIVE" \
        --extract-to "$tmp" \
        --workspace-remap . \
        --no-run; then
        echo "Failed to extract nextest archive: $CDK_ITEST_ARCHIVE" >&2
        df -h "$root" >&2 || true
        return 1
    fi

    # Publish atomically: no half-extracted directory is visible.
    rm -rf "$dir"
    mv "$tmp" "$dir"
    tmp=""
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

    # Hold this shared lease until the integration-test script exits. A job
    # preparing another archive revision can then safely reclaim every
    # extraction that is no longer in use instead of retaining ~19 GB per
    # revision for several days.
    acquire_nextest_use_lease "$root" "$dir" || return 1

    if [ ! -f "$metadata" ]; then
        # No complete extraction: serialize pruning and extraction across all
        # runner containers on the host. flock releases the lock when its
        # owner is killed, so one OOM-killed extractor cannot block every later
        # integration job for the rest of its timeout.
        local lock lock_fd lock_wait_seconds
        lock_wait_seconds=$(nextest_lock_wait_seconds) || {
            echo "Integration-test suite deadline reached while waiting to extract nextest archive" >&2
            return 1
        }

        if command -v flock >/dev/null 2>&1; then
            lock="$root/.extract.lock"
            exec {lock_fd}>"$lock"
            if ! flock --exclusive --timeout "$lock_wait_seconds" "$lock_fd"; then
                echo "Timed out after ${lock_wait_seconds}s waiting for nextest extraction lock" >&2
                exec {lock_fd}>&-
                return 1
            fi

            if ! (extract_nextest_archive_locked "$root" "$dir" "$metadata"); then
                flock --unlock "$lock_fd"
                exec {lock_fd}>&-
                return 1
            fi

            flock --unlock "$lock_fd"
            exec {lock_fd}>&-
        else
            # mkdir remains the portable fallback for developer machines. CI's
            # Linux shell always provides flock through util-linux.
            lock="$root/.extract.lock.d"
            local waited=0
            while ! mkdir "$lock" 2>/dev/null; do
                if find "$lock" -maxdepth 0 -mmin +360 2>/dev/null | grep -q .; then
                    echo "Breaking stale nextest extraction lock: $lock" >&2
                    rmdir "$lock" 2>/dev/null || true
                    continue
                fi
                if [ "$waited" -ge "$lock_wait_seconds" ]; then
                    echo "Timed out after ${lock_wait_seconds}s waiting for nextest extraction lock" >&2
                    return 1
                fi
                sleep 5
                waited=$((waited + 5))
            done

            if ! (extract_nextest_archive_locked "$root" "$dir" "$metadata"); then
                rmdir "$lock" 2>/dev/null || true
                return 1
            fi
            rmdir "$lock" 2>/dev/null || true
        fi
    fi

    if [ ! -f "$metadata" ]; then
        echo "Nextest archive extraction did not produce metadata: $metadata" >&2
        return 1
    fi
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

# Helper: explicitly initialize cdk-mintd from a file, then run it from $PATH
# (Nix pre-built) or fall back to cargo run with the grpc-processor feature.
run_mintd_bg() {
    local work_dir="$1"
    local config_file="$2"

    if command -v cdk-mintd &>/dev/null; then
        echo "Using pre-built binary: cdk-mintd"
        cdk-mintd --work-dir "$work_dir" config init --new-mint --file "$config_file" || return 1
        cdk-mintd --work-dir "$work_dir" &
    else
        echo "Pre-built cdk-mintd not found, falling back to cargo run"
        cargo run --bin cdk-mintd --no-default-features --features grpc-processor,sqlite -- \
            --work-dir "$work_dir" config init --new-mint --file "$config_file" || return 1
        cargo run --bin cdk-mintd --no-default-features --features grpc-processor,sqlite -- \
            --work-dir "$work_dir" &
    fi
}

# Helper: run cargo nextest from the (already extracted) archive if available,
# or fall back to cargo test.
# For nextest: translates cargo test conventions and strips '--' separators.
# Each test binary gets a separate GNU timeout process group, so a hung test
# and any descendants it spawned are terminated before the suite-level CI
# timeout. Override the defaults with CDK_ITEST_TEST_TIMEOUT_SECONDS and
# CDK_ITEST_TEST_KILL_AFTER_SECONDS. When CDK_ITEST_SUITE_DEADLINE_EPOCH is
# set, each invocation also leaves time for suite cleanup before the CI job's
# hard timeout.
#
# Usage: run_test <test_name> [extra cargo-test args...]
run_test_with_timeout() {
    local test_timeout_seconds="${CDK_ITEST_TEST_TIMEOUT_SECONDS:-1200}"
    local kill_after_seconds="${CDK_ITEST_TEST_KILL_AFTER_SECONDS:-30}"

    case "$test_timeout_seconds:$kill_after_seconds:${CDK_ITEST_SUITE_DEADLINE_EPOCH:-}" in
        *[!0-9:]*)
            echo "Integration-test timeout values must be non-negative integers" >&2
            return 2
            ;;
    esac

    if ! command -v timeout >/dev/null 2>&1; then
        echo "GNU timeout is required to run integration test binaries" >&2
        return 1
    fi

    if [ -n "${CDK_ITEST_SUITE_DEADLINE_EPOCH:-}" ]; then
        local remaining_seconds
        remaining_seconds=$((CDK_ITEST_SUITE_DEADLINE_EPOCH - $(date +%s)))
        if [ "$remaining_seconds" -le 0 ]; then
            echo "Integration-test suite deadline reached before starting test binary" >&2
            return 124
        fi
        if [ "$remaining_seconds" -lt "$test_timeout_seconds" ]; then
            test_timeout_seconds=$remaining_seconds
        fi
    fi

    echo "Test binary timeout: ${test_timeout_seconds}s (SIGKILL after ${kill_after_seconds}s)"

    # GNU timeout creates a process group for itself and the test command. Run
    # it asynchronously so cancellation traps can forward signals to the whole
    # group, then reap it before returning.
    timeout \
        --signal=TERM \
        --kill-after="${kill_after_seconds}s" \
        "${test_timeout_seconds}s" \
        "$@" &
    local test_pid=$!
    local old_hup old_int old_term
    old_hup=$(trap -p HUP)
    old_int=$(trap -p INT)
    old_term=$(trap -p TERM)

    forward_test_signal() {
        local signal="$1"
        kill "-$signal" -- "-$test_pid" 2>/dev/null || true
    }
    trap 'forward_test_signal HUP' HUP
    trap 'forward_test_signal INT' INT
    trap 'forward_test_signal TERM' TERM

    local status=0
    wait "$test_pid" || status=$?

    if [ -n "$old_hup" ]; then eval "$old_hup"; else trap - HUP; fi
    if [ -n "$old_int" ]; then eval "$old_int"; else trap - INT; fi
    if [ -n "$old_term" ]; then eval "$old_term"; else trap - TERM; fi
    unset -f forward_test_signal

    return "$status"
}

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
        run_test_with_timeout cargo nextest run \
            --cargo-metadata "$extract_dir/target/nextest/cargo-metadata.json" \
            --binaries-metadata "$extract_dir/target/nextest/binaries-metadata.json" \
            --workspace-remap . \
            --target-dir-remap "$extract_dir/target" \
            -E "binary(/^${test_name}$/)" "${nextest_args[@]}"
    else
        echo "Running test '$test_name' via cargo test"
        run_test_with_timeout cargo test -p cdk-integration-tests --test "$test_name" "$@"
    fi
}
