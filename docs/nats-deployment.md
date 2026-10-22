# NATS Deployment Guide

Kobbler uses NATS JetStream for job processing and cross-service communication with Kobload.

## Quick Start

Both Kobbler and Kobload include NATS in their docker-compose files. By default, each runs its own NATS instance - no additional configuration needed.

```bash
# Kobbler
docker compose up -d

# Kobload (separate deployment)
docker compose up -d
```

## Deployment Scenarios

### Standalone (Default)

Each project runs its own NATS instance. Jobs are processed locally within each service. This is the simplest setup and works well when:
- Running only Kobbler (no Kobload integration)
- Running only Kobload (no Kobbler integration)
- Services are on different networks/hosts

### Shared NATS (Cross-Service Communication)

To enable cross-service features like:
- Watch pattern sync (Kobbler wanted items → Kobload)
- Download completion events (Kobload → Kobbler ingest)
- Manual search requests (Kobbler → Kobload → Prowlarr)

Configure both services to use the same NATS instance:

```bash
# Option 1: Point Kobload to Kobbler's NATS
# In Kobload's environment:
NATS_URL=nats://kobbler-host:4222

# Option 2: Use a dedicated NATS deployment
# In both services:
NATS_URL=nats://nats.example.com:4222
```

### Development (Local)

For local development, a shared NATS instance is managed separately in `koblibs/docker-compose.nats.yml`. Both projects' `run_dev.sh` scripts start this shared NATS automatically.

**Either project can be started first** - they both use the same shared NATS:

```bash
# Start either project first
cd kobbler && ./scripts/run_dev.sh
# or
cd kobload && ./scripts/run_dev.sh

# Then start the other
cd kobload && ./scripts/run_dev.sh
# or
cd kobbler && ./scripts/run_dev.sh
```

The dev setup:
- `../koblibs/docker-compose.nats.yml` defines the shared NATS container on `kob_dev` network
- Both `run_dev.sh` scripts call `docker compose -f ../koblibs/docker-compose.nats.yml up -d`
- Both dev compose files mark `kob_dev` as external and don't define their own NATS
- Whichever project starts first creates NATS; the second reuses it

## Configuration

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `NATS_URL` | `nats://${NATS_TOKEN}@nats:4222` | NATS server URL (includes auth token) |
| `NATS_TOKEN` | (none) | NATS auth token (required) |
| `NATS_PORT` | `4222` | Host port for NATS (docker-compose) |
| `NATS_MONITOR_PORT` | `8222` | NATS monitoring port |

### NATS Server Configuration

The default NATS configuration in docker-compose:

```yaml
nats:
  image: nats:2.12-alpine
  command: ["--jetstream", "--store_dir=/data", "-m", "8222", "--auth", "${NATS_TOKEN}"]
  volumes:
    - nats_data:/data
  ports:
    - "${NATS_MONITOR_PORT:-8222}:8222"
```

## Securing Cross-Host Communication

When running Kobbler and Kobload on different servers, secure the NATS connection.

### Option 1: Token Authentication (Simple)

The easiest approach - a shared secret token.

**On the Kobbler server** (where NATS runs), create `config/nats.conf`:

```conf
jetstream {
  store_dir: /data
}

authorization {
  token: "your-secure-token-here"
}
```

Update your docker-compose to use this config:

```yaml
nats:
  image: nats:2.12-alpine
  command: ["-c", "/etc/nats/nats.conf"]
  volumes:
    - nats_data:/data
    - ./config/nats.conf:/etc/nats/nats.conf:ro
  ports:
    - "4222:4222"
```

**On the Kobload server**, configure the connection with the token:

```bash
# In .env or docker-compose environment
NATS_URL=nats://your-secure-token-here@kobbler-host:4222
```

Generate a secure token:
```bash
openssl rand -base64 32
```

### Option 2: Token Auth + TLS (Recommended for Internet)

For connections over the internet, add TLS encryption.

**Generate certificates** (or use Let's Encrypt):

```bash
# Self-signed for internal use
openssl req -x509 -nodes -days 365 -newkey rsa:2048 \
  -keyout config/certs/nats-server.key \
  -out config/certs/nats-server.crt \
  -subj "/CN=kobbler-host"
```

**NATS config** (`config/nats.conf`):

```conf
jetstream {
  store_dir: /data
}

authorization {
  token: "your-secure-token-here"
}

tls {
  cert_file: /certs/nats-server.crt
  key_file: /certs/nats-server.key
}
```

**Docker compose**:

```yaml
nats:
  image: nats:2.12-alpine
  command: ["-c", "/etc/nats/nats.conf"]
  volumes:
    - nats_data:/data
    - ./config/nats.conf:/etc/nats/nats.conf:ro
    - ./config/certs:/certs:ro
  ports:
    - "4222:4222"
```

**Kobload connection** (note `tls://` scheme):

```bash
# For self-signed certs, you may need to disable verification
NATS_URL=tls://your-secure-token-here@kobbler-host:4222
```

### Option 3: Firewall Only (Trusted Network)

If both servers are on a trusted private network (e.g., same VPC, Tailscale):

1. Run NATS without auth (default)
2. Use firewall rules to restrict port 4222 to known IPs

```bash
# Example: allow only Kobload server IP
ufw allow from 192.0.2.30 to any port 4222
```

### Testing the Connection

From the Kobload server:

```bash
# Install NATS CLI
# https://github.com/nats-io/natscli

# Test connection
nats server ping --server=nats://token@kobbler-host:4222

# Check JetStream
nats stream ls --server=nats://token@kobbler-host:4222
```

## JetStream Streams

The job system creates two streams automatically on startup:

| Stream | Purpose | Retention |
|--------|---------|-----------|
| `JOBS` | Work queue for job processing | WorkQueue (each message processed once) |
| `EVENTS` | Pub/sub for notifications | Limits (24h retention) |

### Job Subjects

Jobs are published to `jobs.{service}.{job_type}`:

| Subject | Service | Description |
|---------|---------|-------------|
| `jobs.kobbler.directory_scan` | Kobbler | Scan watch directory for new files |
| `jobs.kobbler.metadata_write` | Kobbler | Write metadata to book files |
| `jobs.kobbler.metadata_extraction` | Kobbler | Extract metadata from files |
| `jobs.kobbler.sync_provider` | Kobbler | Sync reading state to providers |
| `jobs.kobload.want_request` | Kobload | Create/update a want's watch pattern and search for it |
| `jobs.kobload.watch_pattern_delete` | Kobload | Delete watch pattern |

### Event Subjects

Events are published to `events.{event_type}`:

| Subject | Publisher | Description |
|---------|-----------|-------------|
| `events.download.completed` | Kobload | Download finished, ready for ingest |
| `events.download.failed` | Kobload | Download failed |
| `events.library.book_added` | Kobbler | New book added to library |

## Monitoring

### Health Check

Both services expose NATS connection status in their health endpoints:

```bash
curl http://localhost:6766/health  # Kobbler
curl http://localhost:7080/health  # Kobload
```

### NATS Monitoring (Dev)

In dev mode, NATS monitoring is available at port 8222:

```bash
# Stream info
curl http://localhost:8222/jsz

# Connection info
curl http://localhost:8222/connz
```

### Logs

NATS connection events are logged at INFO level:

```
INFO nats_jobs::client: Connecting to NATS at nats://nats:4222
INFO nats_jobs::client: Connected to NATS
INFO nats_jobs::client: JOBS stream ready: 0 messages
INFO nats_jobs::client: EVENTS stream ready: 0 messages
```

## Troubleshooting

### "Connection refused" errors

1. Check NATS is running: `docker ps | grep nats`
2. Verify `NATS_URL` is correct
3. For cross-host connections, ensure port 4222 is accessible

### Jobs not processing

1. Check worker is registered: look for `Worker started, listening on jobs.kobbler.>` in logs
2. Verify stream exists: check NATS monitoring at `/jsz`
3. Check for handler errors in logs

### Cross-service events not received

1. Verify both services use the same NATS instance
2. Check subscription logs: `Subscribed to download events from Kobload`
3. Ensure event publisher is connected: look for `Connected to NATS` in publisher logs
