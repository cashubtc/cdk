#!/usr/bin/env bash

set -euo pipefail

# Keep the default usable in CI runner containers, where /data is not mounted.
# CDK_CI_TMP_ROOT can still select a dedicated scratch mount when available.
tmp_root="${CDK_CI_TMP_ROOT:-/var/tmp}"
mkdir -p "$tmp_root"

ci_tmpdir=$(mktemp -d "$tmp_root/cdk-itest.XXXXXX")

cleanup_ci_tmpdir() {
    rm -rf "$ci_tmpdir"
}
trap cleanup_ci_tmpdir EXIT

export TMPDIR="$ci_tmpdir"

if [ -n "${CDK_ITEST_SUITE_TIMEOUT_SECONDS:-}" ] &&
    [ -z "${CDK_ITEST_SUITE_DEADLINE_EPOCH:-}" ]; then
    case "$CDK_ITEST_SUITE_TIMEOUT_SECONDS" in
        *[!0-9]* | "")
            echo "CDK_ITEST_SUITE_TIMEOUT_SECONDS must be a non-negative integer" >&2
            exit 2
            ;;
    esac
    export CDK_ITEST_SUITE_DEADLINE_EPOCH
    CDK_ITEST_SUITE_DEADLINE_EPOCH=$((
        $(date +%s) + CDK_ITEST_SUITE_TIMEOUT_SECONDS
    ))
fi

set +m
if command -v setsid >/dev/null 2>&1; then
    setsid "$@" &
else
    set -m
    "$@" &
    set +m
fi
command_pid=$!

interrupted_status=0
stop_command_group() {
    local signal="$1"
    local signal_status="$2"
    kill "-$signal" -- "-$command_pid" 2>/dev/null || true

    local waited=0
    while kill -0 -- "-$command_pid" 2>/dev/null && [ "$waited" -lt 10 ]; do
        sleep 1
        waited=$((waited + 1))
    done
    if kill -0 -- "-$command_pid" 2>/dev/null; then
        kill -KILL -- "-$command_pid" 2>/dev/null || true
    fi
    wait "$command_pid" 2>/dev/null || true
    interrupted_status=$signal_status
}
trap 'stop_command_group HUP 129' HUP
trap 'stop_command_group INT 130' INT
trap 'stop_command_group TERM 143' TERM

status=0
wait "$command_pid" || status=$?
if [ "$interrupted_status" -ne 0 ]; then
    status=$interrupted_status
fi
exit "$status"
