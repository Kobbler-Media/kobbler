# NAS health monitoring

Kobbler can watch a CIFS/SMB NAS for contention by reading the kernel's
share-reconnect counter out of `/proc/fs/cifs/Stats`. Share reconnects only
happen when the SMB session drops, which only happens when the NAS is so
overwhelmed it stops responding — a clean "things are too hot" signal that
doesn't false-positive on normal load.

When the reconnect delta over the last hour crosses **5**, the dashboard
shows a yellow "NAS contention detected" banner with guidance to lower
`max_concurrent_io` on affected media roots. When the stats file itself
becomes unreadable, the banner turns red ("NAS monitoring unreachable").

This feature is **opt-in** and **off by default**.

## Who needs it

You want this if:
- Your media lives on a CIFS/SMB share (`mount.cifs`, not `mount.nfs`).
- You've seen backfill runs stall the NAS in the past.
- You want early warning before the SMB credit window collapses.

You do **not** need this if:
- Storage is local SSD/HDD or NFS.
- The kernel `cifs` module is not loaded on the host (`ls /proc/fs/cifs`
  will error if that's the case).

## Why it needs a script

Runc (the container runtime Docker uses) forbids bind-mounting any file
that lives inside `/proc` into a container — it's a deliberate
container-escape-prevention check that cannot be overridden, not even
with `--privileged`. So kobbler can't read `/proc/fs/cifs/Stats` directly
from inside its container.

The workaround: a tiny systemd timer on the host copies that procfs file to
`/var/lib/kobbler/nas-stats` once a minute (atomic write via `mv`), and the
api container bind-mounts *that* benign file instead. Kobbler's
`KOBBLER_NAS_HEALTH_STATS_PATH` env var tells it to read from the mirrored
path.

## Setup

### 1. Install the host-side refresher

```bash
sudo ./scripts/setup-nas-health-stats.sh
```

This installs `kobbler-nas-stats.service` + `.timer`, enables the timer,
and kicks off an immediate refresh. It's safe to re-run.

Verify the file is landing:

```bash
ls -l /var/lib/kobbler/nas-stats
# ... should show a recent mtime
head -3 /var/lib/kobbler/nas-stats
# Resources in use
# CIFS Session: 1
# Share (unique mount targets): 2
```

If `/var/lib/kobbler/nas-stats` stays empty, check
`systemctl status kobbler-nas-stats.timer` — most likely cause is
`/proc/fs/cifs/Stats` not existing (no active CIFS mount on the host),
in which case the feature has nothing to monitor and you should leave
it disabled until you actually mount something via SMB.

### 2. Wire the api container

In `docker-compose.yml` (or your `docker-compose.prod.yml`), add to the
`api` service:

```yaml
services:
  api:
    environment:
      - KOBBLER_NAS_HEALTH_MONITORING=1
      - KOBBLER_NAS_HEALTH_STATS_PATH=/var/lib/kobbler/nas-stats
    volumes:
      - /var/lib/kobbler/nas-stats:/var/lib/kobbler/nas-stats:ro
```

Notes:
- Bind-mount is a **single file**, not a directory — this is what sidesteps
  runc's proc-safety check.
- The env var path must match the bind-mount target path.
- Leave the env var set to a non-procfs path even on bare metal if you
  prefer; it works fine as long as *something* keeps the file fresh.

### 3. Restart and verify

```bash
docker compose up -d api
curl -u admin http://<api>/api/v1/admin/system/nas-health
# → {"status":"healthy","reconnects_last_hour":0,"note":null}
```

The dashboard banner stays hidden unless status is `degraded` or
`unreachable`. If you want to force a test render, temporarily edit
`/var/lib/kobbler/nas-stats` and pad a fake "share reconnects: 999"
line — the banner will appear within 60s.

## Tuning

The threshold (`5 reconnects/hour`), window (`1 hour`), and poll interval
(`60s`) are currently hardcoded in `api/src/nas_health.rs`. If you find
yourself with a noisier NAS that naturally drops a session occasionally,
bump `DEGRADED_THRESHOLD` in that file rather than disabling the whole
feature — a higher threshold still catches real contention.

## Rolling back

If the feature misfires or the host refresher is causing noise:

```bash
sudo systemctl disable --now kobbler-nas-stats.timer
sudo rm /etc/systemd/system/kobbler-nas-stats.{service,timer}
sudo systemctl daemon-reload
```

Then unset `KOBBLER_NAS_HEALTH_MONITORING` in compose and restart the api
container. The `/var/lib/kobbler/nas-stats` file and directory can stay —
they're harmless once nothing's reading them.

## Related

- `api/src/nas_health.rs` — the monitor itself.
- `api/src/routes/jobs_concurrency.rs::get_nas_health` — the
  `/api/v1/admin/system/nas-health` endpoint.
- `web/src/routes/dashboard/+page.svelte` — dashboard banner (polls every
  60s, stops if the endpoint reports `disabled`).
- Per-media-root `max_concurrent_io` in the media root edit UI — the knob
  you reach for when the banner lights up.
