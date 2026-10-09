#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ ! -f "${root_dir}/.env" ]]; then
    echo "Warning: .env not found. Run ./scripts/setup-prod.sh first." >&2
fi
compose_file="${root_dir}/docker-compose.yml"
compose_args=(-f "${compose_file}")

export KOBBLER_DB_PORT="${KOBBLER_DB_PORT:-5432}"
export KOBBLER_API_PORT="${KOBBLER_API_PORT:-6766}"
export KOBBLER_WEB_PORT="${KOBBLER_WEB_PORT:-5173}"
export KOBBLER_BASE_URL="${KOBBLER_BASE_URL:-http://localhost:${KOBBLER_API_PORT}}"

web_host="${KOBBLER_WEB_HOST:-localhost}"

echo "Starting Kobbler (production UI)..."
docker compose "${compose_args[@]}" up -d --build
echo "UI URL: http://${web_host}:${KOBBLER_WEB_PORT}"
echo "API URL: ${KOBBLER_BASE_URL}"
