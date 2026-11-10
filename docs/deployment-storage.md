# Storage Configuration

Kobbler stores book files, covers, and other data in a configurable storage location. This guide covers the options for configuring storage.

## Default: Docker Named Volumes

By default, docker-compose.yml uses named volumes:

```yaml
volumes:
  kobbler_data:/data
  kobbler_pg:/var/lib/postgresql
```

Named volumes are managed by Docker and stored in `/var/lib/docker/volumes/` on the host. They're easy to use but harder to access directly or back up.

## Host Directory Mounts

For easier backup and direct file access, you can mount a host directory instead.

### Basic Setup

Edit `docker-compose.yml` and change the `api` service volumes:

```yaml
services:
  api:
    volumes:
      # Named volume (default)
      # - kobbler_data:/data

      # Host directory (uncomment and adjust path)
      - /var/kobbler/data:/data
```

Create the directory with appropriate permissions:

```bash
sudo mkdir -p /var/kobbler/data
sudo chown -R 1000:1000 /var/kobbler/data  # Or your user's UID:GID
```

### Directory Structure

Kobbler creates its own subdirectories inside the data directory — `books/`,
`covers/`, `voices/`, `proxy_blobs/`, `reader_packs/` and caches. Don't edit
them by hand; use the UI or the backup/restore feature.

### Multiple Instances

If running multiple Kobbler instances, use separate data directories:

```yaml
# Instance 1
volumes:
  - /var/kobbler/instance1:/data

# Instance 2
volumes:
  - /var/kobbler/instance2:/data
```

## Permissions

The API container runs as root by default. If you need to run as a non-root user:

1. Set ownership on the host directory:
```bash
sudo chown -R 1000:1000 /var/kobbler/data
```

2. Add user mapping to the container (if needed):
```yaml
services:
  api:
    user: "1000:1000"
```

## Watch Directories

Watch directories can be configured to auto-import books from host paths. When using host mounts, ensure the watch directory path is accessible inside the container.

Example: To watch `/home/user/books` on the host:

```yaml
services:
  api:
    volumes:
      - /var/kobbler/data:/data
      - /home/user/books:/import:ro  # Read-only mount
```

Then configure the watch directory in the UI to point to `/import`.

## Backup Recommendations

### Database

The PostgreSQL database should be backed up separately:

```bash
# From the kobbler checkout
docker compose exec -T db pg_dump -U kobbler kobbler > backup.sql

# Or with volume mounted
docker run --rm -v kobbler_pg:/data -v $(pwd):/backup alpine \
  tar czf /backup/pg_data.tar.gz -C /data .
```

### Book Files

With host directory mounts, back up directly:

```bash
tar czf kobbler_data_backup.tar.gz /var/kobbler/data
```

With named volumes:

```bash
docker run --rm -v kobbler_data:/data -v $(pwd):/backup alpine \
  tar czf /backup/kobbler_data.tar.gz -C /data .
```

### Full Backup Script

```bash
#!/bin/bash
BACKUP_DIR=/var/backups/kobbler
DATE=$(date +%Y%m%d_%H%M%S)

mkdir -p $BACKUP_DIR

# Database
docker compose exec -T db pg_dump -U kobbler kobbler > $BACKUP_DIR/db_$DATE.sql

# Data files (adjust path for your setup)
tar czf $BACKUP_DIR/data_$DATE.tar.gz /var/kobbler/data

# Keep last 7 days
find $BACKUP_DIR -name "*.sql" -mtime +7 -delete
find $BACKUP_DIR -name "*.tar.gz" -mtime +7 -delete
```

## Migrating from Named Volumes to Host Directory

1. Stop Kobbler:
```bash
docker compose down
```

2. Copy data from volume to host:
```bash
sudo mkdir -p /var/kobbler/data
docker run --rm -v kobbler_data:/source -v /var/kobbler/data:/dest alpine \
  cp -a /source/. /dest/
```

3. Update docker-compose.yml to use the host path

4. Start Kobbler:
```bash
docker compose up -d
```

5. Verify everything works, then optionally remove the old volume:
```bash
docker volume rm kobbler_data
```
