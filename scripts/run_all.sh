#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
compose_file="${root_dir}/docker-compose.yml"
compose_args=(-f "${compose_file}")

export KOBBLER_DB_PORT="${KOBBLER_DB_PORT:-5432}"
export KOBBLER_API_PORT="${KOBBLER_API_PORT:-6766}"
export KOBBLER_WEB_PORT="${KOBBLER_WEB_PORT:-5173}"
export KOBBLER_BASE_URL="${KOBBLER_BASE_URL:-http://localhost:${KOBBLER_API_PORT}}"

echo "Starting Kobbler stack..."
docker compose "${compose_args[@]}" up --build
