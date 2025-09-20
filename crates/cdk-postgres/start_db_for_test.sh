#!/usr/bin/env bash
set -euo pipefail

CONTAINER_NAME="rust-test-pg"
DB_USER="test"
DB_PASS="test"
DB_NAME="testdb"
DB_PORT="5433"

echo "Starting fresh PostgreSQL container..."
docker run -d --rm \
  --name "${CONTAINER_NAME}" \
  -e POSTGRES_USER="${DB_USER}" \
  -e POSTGRES_PASSWORD="${DB_PASS}" \
  -e POSTGRES_DB="${DB_NAME}" \
  -p ${DB_PORT}:5432 \
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

# Export environment variables for both main and auth databases
export DATABASE_URL="host=localhost user=${DB_USER} password=${DB_PASS} dbname=${DB_NAME} port=${DB_PORT}"
export CDK_MINTD_POSTGRES_URL="postgresql://${DB_USER}:${DB_PASS}@localhost:${DB_PORT}/mintdb"
export CDK_MINTD_AUTH_POSTGRES_URL="postgresql://${DB_USER}:${DB_PASS}@localhost:${DB_PORT}/mintdb_auth"

echo "Database URLs configured:"
echo "Main database: ${CDK_MINTD_POSTGRES_URL}"
echo "Auth database: ${CDK_MINTD_AUTH_POSTGRES_URL}"
