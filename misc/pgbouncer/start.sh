#!/usr/bin/env bash
# Test-only proxy configuration; never use these credentials/auth settings in production.
set -euo pipefail

pg_test_dir="${CDK_TEST_PGBOUNCER_DIR:-$PWD/.pgbouncer_test}"
mkdir -p "$pg_test_dir"
chmod 700 "$pg_test_dir"
pg_test_host="${CDK_TEST_PG_HOST:-127.0.0.1}"
pg_test_port="${CDK_TEST_PG_PORT:-5432}"
pg_test_user="${CDK_TEST_PG_USER:-cdk_user}"
pg_test_password="${CDK_TEST_PG_PASSWORD:-cdk_password}"
pg_test_database="${CDK_TEST_PG_DATABASE:-cdk_mint}"
printf '"%s" "%s"\n' "$pg_test_user" "$pg_test_password" > "$pg_test_dir/users.txt"
chmod 600 "$pg_test_dir/users.txt"

for mode in transaction session; do
    if [[ "$mode" == transaction ]]; then
        port="${CDK_TEST_PGBOUNCER_TRANSACTION_PORT:-6432}"
        size=2
    else
        port="${CDK_TEST_PGBOUNCER_SESSION_PORT:-6433}"
        size=32
    fi
    if [[ -f "$pg_test_dir/$mode.pid" ]] && kill -0 "$(cat "$pg_test_dir/$mode.pid")" 2>/dev/null; then
        echo "PgBouncer $mode is already running on port $port"
        continue
    fi
    cat > "$pg_test_dir/$mode.ini" <<EOF
[databases]
$pg_test_database = host=$pg_test_host port=$pg_test_port dbname=$pg_test_database
[pgbouncer]
listen_addr = 127.0.0.1
listen_port = $port
unix_socket_dir = $pg_test_dir
auth_type = plain
auth_file = $pg_test_dir/users.txt
admin_users = $pg_test_user
pool_mode = $mode
default_pool_size = $size
max_client_conn = 300
max_prepared_statements = 200
server_round_robin = 1
pidfile = $pg_test_dir/$mode.pid
logfile = $pg_test_dir/$mode.log
EOF
    pgbouncer -d "$pg_test_dir/$mode.ini"
    ready=0
    for _attempt in {1..100}; do
        if PGPASSWORD="$pg_test_password" psql -h 127.0.0.1 -p "$port" -U "$pg_test_user" -d "$pg_test_database" -c 'SELECT 1' >/dev/null 2>&1; then
            ready=1
            break
        fi
        sleep 0.1
    done
    if [[ "$ready" != 1 ]]; then
        cat "$pg_test_dir/$mode.log" >&2
        exit 1
    fi
    echo "PgBouncer $mode ready on port $port"
done
