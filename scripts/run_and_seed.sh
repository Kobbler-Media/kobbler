#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
compose_dev="${root_dir}/docker-compose.dev.yml"
compose_args=(-f "${compose_dev}")

admin_email="${KOBBLER_ADMIN_EMAIL:-admin@localhost}"
user_email="${KOBBLER_USER_EMAIL:-user@example.com}"
user_password="${KOBBLER_USER_PASSWORD:-testpass}"

reset_flag="${KOBBLER_RESET:-1}"

if [[ "${reset_flag}" == "1" ]]; then
  echo "Resetting volumes..."
  docker compose "${compose_args[@]}" down -v
  # Also reset shared NATS data to avoid stale consumer state
  nats_compose="${root_dir}/../koblibs/docker-compose.nats.yml"
  if [[ -f "${nats_compose}" ]]; then
    echo "Resetting NATS volumes..."
    docker compose -f "${nats_compose}" down -v
  fi
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

echo "Starting Kobbler stack..."
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

# After a volume reset, cargo watch must recompile the entire project from
# scratch (~3-5 min for a debug build). Give it plenty of time.
max_wait=300
echo "Waiting for API health (up to ${max_wait}s — first build after reset is slow)..."
healthy=0
for i in $(seq 1 "${max_wait}"); do
  if curl -fsS "${base_url}/health" >/dev/null 2>&1; then
    healthy=1
    break
  fi
  if (( i % 30 == 0 )); then
    echo "  Still waiting... (${i}s)"
  fi
  sleep 1
done

if [[ "${healthy}" -ne 1 ]]; then
  echo "API health check failed at ${base_url}/health" >&2
  exit 1
fi

# Try to get ingest token directly from database first (works for dev restores)
db_id="$(docker compose "${compose_args[@]}" ps -q kobbler-db | tail -n 1)"
ingest_token=""
if [[ -n "${db_id}" ]]; then
  ingest_token="$(docker exec "${db_id}" psql -U kobbler -d kobbler -t -c "SELECT token FROM ingest_tokens LIMIT 1;" 2>/dev/null | tr -d ' \n' || true)"
fi

# Default dev password (used when restoring from dev backup)
dev_password="${KOBBLER_DEV_PASSWORD:-kob-dev-password}"

if [[ -n "${ingest_token}" ]]; then
  echo "Found existing ingest token from dev backup, skipping bootstrap..."
  admin_password="${dev_password}"
else
  # Need to bootstrap - find password from logs (fresh start)
  admin_password=""
  for attempt in 1 2; do
    for _ in $(seq 1 30); do
      log_line="$(docker logs "${api_id}" 2>/dev/null | sed -r 's/\\x1b\\[[0-9;]*m//g' | rg -m1 -o "password=[^ ]+" || true)"
      if [[ -n "${log_line}" ]]; then
        admin_password="${log_line#password=}"
        break
      fi
      sleep 1
    done

    if [[ -n "${admin_password}" ]]; then
      break
    fi

    if [[ "${attempt}" -eq 1 && "${reset_flag}" != "1" ]]; then
      echo "Admin password not found in logs; resetting volumes for a fresh bootstrap..."
      docker compose "${compose_args[@]}" down -v
      docker compose "${compose_args[@]}" up -d --build
      api_id="$(docker compose "${compose_args[@]}" ps -q api | tail -n 1)"
    fi
  done

  if [[ -z "${admin_password}" ]]; then
    echo "Admin password not found in logs. Try KOBBLER_RESET=1 ./scripts/run_and_seed.sh" >&2
    exit 1
  fi

  echo "Bootstrapping user/device/tokens..."
  bootstrap_out="$("${root_dir}/scripts/bootstrap.py" \
    --base-url "${base_url}" \
    --admin-email "${admin_email}" \
    --admin-password "${admin_password}" \
    --user-email "${user_email}" \
    --user-password "${user_password}")"

  echo "${bootstrap_out}"

  ingest_token="$(echo "${bootstrap_out}" | rg -n "Calibre Ingest Token" | sed -n 's/.*Token: //p' | tail -n1)"
  if [[ -z "${ingest_token}" ]]; then
    echo "Ingest token not found in bootstrap output." >&2
    exit 1
  fi
fi

echo ""
echo "Login details:"
echo "- Admin email: ${admin_email}"
echo "- Admin password: ${admin_password}"
echo "- User email: ${user_email}"
echo "- User password: ${user_password}"

seed_count="${KOBBLER_SEED_COUNT:-0}"

# Get device ID from database for proxy log seeding
device_id=""
if [[ -n "${db_id}" ]]; then
  device_id="$(docker exec "${db_id}" psql -U kobbler -d kobbler -t -c "SELECT id FROM devices LIMIT 1;" 2>/dev/null | tr -d ' \n' || true)"
fi

echo "Seeding demo data..."
python "${root_dir}/scripts/seed_demo_data.py" \
  --base-url "${base_url}" \
  --token "${ingest_token}" \
  --admin-email "${admin_email}" \
  --admin-password "${admin_password}" \
  --db-container "$(docker compose "${compose_args[@]}" ps -q kobbler-db | tail -n 1)" \
  --device-id "${device_id}" \
  --count "${seed_count}"

echo "UI URL: http://localhost:${KOBBLER_WEB_PORT}"
echo "API URL: ${base_url}"
echo "Done."
