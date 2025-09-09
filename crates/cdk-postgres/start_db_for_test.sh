#!/usr/bin/env bash
set -euo pipefail

CONTAINER_NAME="rust-test-pg"
DB_USER="test"
DB_PASS="test"
DB_NAME="testdb"
DB_PORT="5433"

echo "Starting fresh PostgreSQL container..."
# removing existing container
docker rm -f  "${CONTAINER_NAME}"
docker run -d --rm \
  --name "${CONTAINER_NAME}" \
  -e POSTGRES_USER="${DB_USER}" \
  -e POSTGRES_PASSWORD="${DB_PASS}" \
  -e POSTGRES_DB="${DB_NAME}" \
  -p 127.0.0.1:${DB_PORT}:5432 \
  postgres:16

echo "Waiting for PostgreSQL to be ready and database '${DB_NAME}' to exist..."
until docker exec -e PGPASSWORD="${DB_PASS}" "${CONTAINER_NAME}" \
    psql -U "${DB_USER}" -d "${DB_NAME}" -c "SELECT 1;" >/dev/null 2>&1; do
  sleep 0.5
done

docker exec -e PGPASSWORD="${DB_PASS}" "${CONTAINER_NAME}" \
  psql -U "${DB_USER}" -d "${DB_NAME}" -c "CREATE DATABASE mintdb;"
docker exec -e PGPASSWORD="${DB_PASS}" "${CONTAINER_NAME}" \
  psql -U "${DB_USER}" -d "${DB_NAME}" -c "CREATE DATABASE mintdb_auth;"

export DATABASE_URL="host=localhost user=${DB_USER} password=${DB_PASS} dbname=${DB_NAME} port=${DB_PORT}"
