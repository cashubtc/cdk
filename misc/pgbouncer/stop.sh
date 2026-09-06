#!/usr/bin/env bash
set -euo pipefail
pg_test_dir="${CDK_TEST_PGBOUNCER_DIR:-$PWD/.pgbouncer_test}"
for mode in transaction session; do
    if [[ -f "$pg_test_dir/$mode.pid" ]]; then
        kill -QUIT "$(cat "$pg_test_dir/$mode.pid")" 2>/dev/null || true
    fi
done
