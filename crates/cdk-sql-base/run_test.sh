#!/usr/bin/env bash
set -euo pipefail

CONTAINER_NAME="rust-test-pg"
DB_USER="test"
DB_PASS="test"
DB_NAME="testdb"
DB_PORT="5433"
DB_URL="postgres://${DB_USER}:${DB_PASS}@localhost:${DB_PORT}/${DB_NAME}"

cleanup() {
  echo "Cleaning up..."
  docker stop "${CONTAINER_NAME}" >/dev/null 2>&1 || true
  docker rm "${CONTAINER_NAME}" >/dev/null 2>&1 || true
}

trap cleanup EXIT INT TERM

echo "Starting fresh PostgreSQL container..."
docker run -d --rm \
  --name "${CONTAINER_NAME}" \
  -e POSTGRES_USER="${DB_USER}" \
  -e POSTGRES_PASSWORD="${DB_PASS}" \
  -e POSTGRES_DB="${DB_NAME}" \
  -p ${DB_PORT}:5432 \
  -v "${PWD}/.docker/pg-init.sql:/docker-entrypoint-initdb.d/init.sql:ro" \
  postgres:16

echo "Waiting for PostgreSQL to be ready..."
until docker exec "${CONTAINER_NAME}" pg_isready -U "${DB_USER}" >/dev/null 2>&1; do
  sleep 0.5
done

export DATABASE_URL="${DB_URL}"

echo "Running cargo tests..."
cargo test

