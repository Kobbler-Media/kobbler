#!/usr/bin/env bash
#
# setup-nas-health-stats.sh
#
# Wires up the host-side half of kobbler's NAS health monitor.
#
# Why this exists: kobbler's `nas_health` module polls `/proc/fs/cifs/Stats`
# for SMB share-reconnect counters to detect NAS contention. When kobbler
# runs under Docker, runc's proc-safety check refuses to bind-mount anything
# inside /proc into the container. This script works around that by
# installing a systemd timer that copies /proc/fs/cifs/Stats to a benign
# host path once a minute. You then bind-mount *that* file into the api
# container at the same path and set KOBBLER_NAS_HEALTH_STATS_PATH to match.
#
# Safe to re-run — idempotent.

set -euo pipefail

STATS_DIR="/var/lib/kobbler"
STATS_FILE="${STATS_DIR}/nas-stats"
SERVICE_NAME="kobbler-nas-stats"
SERVICE_PATH="/etc/systemd/system/${SERVICE_NAME}.service"
TIMER_PATH="/etc/systemd/system/${SERVICE_NAME}.timer"

if [[ $EUID -ne 0 ]]; then
    echo "must run as root (needs to write /etc/systemd/system)" >&2
    exit 1
fi

if [[ ! -r /proc/fs/cifs/Stats ]]; then
    echo "warning: /proc/fs/cifs/Stats does not exist or is unreadable." >&2
    echo "  The kernel cifs module is only loaded once an SMB mount is active." >&2
    echo "  If this host mounts storage via NFS (not CIFS/SMB), the NAS health" >&2
    echo "  monitor has nothing to read and you should leave it disabled." >&2
    echo "  Proceeding anyway so the timer is in place for when a mount appears." >&2
fi

mkdir -p "${STATS_DIR}"
chmod 755 "${STATS_DIR}"

cat > "${SERVICE_PATH}" <<EOF
[Unit]
Description=Copy /proc/fs/cifs/Stats to a bind-mountable file for kobbler
ConditionPathExists=/proc/fs/cifs/Stats

[Service]
Type=oneshot
# Atomic write: dump to .tmp then mv, so readers never see a half-written file.
ExecStart=/bin/bash -c 'cat /proc/fs/cifs/Stats > ${STATS_FILE}.tmp && mv ${STATS_FILE}.tmp ${STATS_FILE}'
EOF

cat > "${TIMER_PATH}" <<EOF
[Unit]
Description=Refresh kobbler NAS health stats every minute

[Timer]
OnBootSec=30s
OnUnitActiveSec=60s
AccuracySec=5s
Unit=${SERVICE_NAME}.service

[Install]
WantedBy=timers.target
EOF

systemctl daemon-reload
systemctl enable --now "${SERVICE_NAME}.timer"

# Kick once immediately so the file exists before docker-compose references it.
systemctl start "${SERVICE_NAME}.service" || true

echo
echo "Installed ${SERVICE_NAME}.timer — refreshing ${STATS_FILE} every 60s."
echo
if [[ -r "${STATS_FILE}" ]]; then
    echo "Current ${STATS_FILE}:"
    head -5 "${STATS_FILE}" | sed 's/^/  /'
    echo "  ..."
else
    echo "(${STATS_FILE} not yet populated — will appear on next tick if /proc/fs/cifs/Stats is readable)"
fi
echo
echo "Next steps:"
echo "  1. Add to the api service in docker-compose.yml:"
echo "       volumes:"
echo "         - ${STATS_FILE}:${STATS_FILE}:ro"
echo "       environment:"
echo "         - KOBBLER_NAS_HEALTH_MONITORING=1"
echo "         - KOBBLER_NAS_HEALTH_STATS_PATH=${STATS_FILE}"
echo "  2. docker compose up -d api"
echo "  3. Check http://<api>/api/v1/admin/system/nas-health — should report 'healthy'."
