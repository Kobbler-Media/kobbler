#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
compose_dev="${root_dir}/docker-compose.dev.yml"
compose_args=(-f "${compose_dev}")

admin_email="${KOBBLER_ADMIN_EMAIL:-admin@localhost}"

reset_flag="${KOBBLER_RESET:-0}"

if [[ "${reset_flag}" == "1" ]]; then
  echo "Resetting volumes..."
  docker compose "${compose_args[@]}" down -v
fi

# Start shared NATS (creates kob_dev network if needed)
nats_compose="${root_dir}/../koblibs/docker-compose.nats.yml"
if [[ -f "${nats_compose}" ]]; then
  echo "Starting shared NATS..."
  docker compose -f "${nats_compose}" up -d
fi

export KOBBLER_DB_PORT="${KOBBLER_DB_PORT:-5432}"
export KOBBLER_API_PORT="${KOBBLER_API_PORT:-6766}"
export KOBBLER_WEB_PORT="${KOBBLER_WEB_PORT:-5173}"
export KOBBLER_BASE_URL="${KOBBLER_BASE_URL:-http://localhost:${KOBBLER_API_PORT}}"
export KOBBLER_WEB_ORIGINS="${KOBBLER_WEB_ORIGINS:-http://localhost:${KOBBLER_WEB_PORT}}"

base_url="${KOBBLER_BASE_URL}"

echo "Starting Kobbler (dev UI)..."
docker compose "${compose_args[@]}" up -d --build

api_id=""
for _ in $(seq 1 20); do
  api_id="$(docker compose "${compose_args[@]}" ps -q api | tail -n 1)"
  if [[ -n "${api_id}" ]]; then
    break
  fi
  sleep 1
done

if [[ -z "${api_id}" ]]; then
  echo "API container not found." >&2
  exit 1
fi

echo "Waiting for API health..."
healthy=0
for _ in $(seq 1 60); do
  if curl -fsS "${base_url}/health" >/dev/null 2>&1; then
    healthy=1
    break
  fi
  sleep 1
done

if [[ "${healthy}" -ne 1 ]]; then
  echo "API health check failed at ${base_url}/health" >&2
  exit 1
fi

admin_password=""
for _ in $(seq 1 30); do
  log_line="$(docker logs "${api_id}" 2>/dev/null | sed -r 's/\\x1b\\[[0-9;]*m//g' | rg -m1 -o "password=[^ ]+" || true)"
  if [[ -n "${log_line}" ]]; then
    admin_password="${log_line#password=}"
    break
  fi
  sleep 1
done

if [[ -n "${admin_password}" ]]; then
  echo "Login details:"
  echo "- Admin email: ${admin_email}"
  echo "- Admin password: ${admin_password}"
else
  echo "Admin password not found in logs (was this an existing volume?)."
fi

echo "UI URL: http://localhost:${KOBBLER_WEB_PORT}"
echo "API URL: ${base_url}"
