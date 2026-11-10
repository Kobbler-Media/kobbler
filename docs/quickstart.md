# Kobbler Quickstart

Detailed setup guide with API examples. See also the main [README](../README.md).

## 1) Start services

```bash
make up
```

`make up` builds the `kobbler-tools` base image first if it is missing; a bare
`docker compose up --build` without it fails with "pull access denied".

Watch logs for the generated admin password:
```
Admin account created: email=admin@localhost password=<password>
```

### Development with demo data (contributors)

`scripts/run_and_seed.sh` runs the hot-reloading dev stack (`docker-compose.dev.yml`) and seeds
demo data. It needs:

- [koblibs](https://github.com/Kobbler-Media/koblibs) checked out next to this repo — its
  `docker-compose.nats.yml` provides the shared dev NATS and creates the `kob_dev` Docker network
  the dev stack joins; the dev images also build against it
- optionally `cp env.dev.template .env.dev` to override dev settings
- `python` (Python 3) and `rg` (ripgrep) on the host

```bash
./scripts/run_and_seed.sh
```

By default this resets volumes each run. To keep existing data:
```bash
KOBBLER_RESET=0 ./scripts/run_and_seed.sh
```

## 2) Bootstrap user/device/token

```bash
./scripts/bootstrap.py \
  --base-url http://localhost:6766 \
  --admin-email admin@localhost \
  --admin-password <password-from-logs> \
  --user-email user@example.com \
  --user-password testpass
```

This prints:
- Kobo sync URL
- Device token
- Calibre ingest token

## 3) Install Calibre plugin

1. Build the plugin zip from the repo root:
   ```bash
   ./scripts/build_plugin_zip.sh
   ```

   Note: The zip filename must be `send_to_kobbler.zip` so Calibre loads it as `calibre_plugins.send_to_kobbler`. The empty `plugin-import-name-send_to_kobbler.txt` file is required for multi-file plugins.

2. In Calibre: Preferences -> Plugins -> Load plugin from file

3. Configure plugin settings:
   - Kobbler Base URL: `http://localhost:6766`
   - Ingest Token: `<token from bootstrap>`
   - Store KEPUB in library: choose on/off based on preference

Note: If KEPUB is missing, the plugin will convert from EPUB using Calibre's conversion pipeline.

## 4) Send a book from Calibre

Select a book -> "Send to Kobbler".

## 5) Configure Kobo device

Edit `.kobo/Kobo/Kobo eReader.conf`:
```ini
[OneStoreServices]
api_endpoint=<kobo_sync_url_from_bootstrap>
```

Restart the Kobo and run sync.

## 6) Verify

- Kobo should show the book in the library after sync
- Kobbler logs should show sync calls

## API Examples (without Calibre)

### Create or update a book

```bash
curl -X PUT http://localhost:6766/api/v1/books/<book_uuid> \
  -H "Authorization: Bearer <ingest_token>" \
  -H "Content-Type: application/json" \
  -d '{"title":"Example","authors":["Author"],"language":"en"}'
```

### Upload a cover

```bash
curl -X PUT http://localhost:6766/api/v1/books/<book_uuid>/cover \
  -H "Authorization: Bearer <ingest_token>" \
  -H "Content-Type: image/jpeg" \
  --data-binary @cover.jpg
```

### Upload a KEPUB file

```bash
curl -X PUT http://localhost:6766/api/v1/books/<book_uuid>/files/KEPUB \
  -H "Authorization: Bearer <ingest_token>" \
  -H "Content-Type: application/octet-stream" \
  --data-binary @book.kepub.epub
```

### Grant entitlement

```bash
curl -X POST http://localhost:6766/api/v1/entitlements \
  -H "Authorization: Bearer <ingest_token>" \
  -H "Content-Type: application/json" \
  -d '{"book_id":"<book_uuid>"}'
```

### Seed demo data

```bash
python scripts/seed_demo_data.py \
  --base-url http://localhost:6766 \
  --token <ingest_token>
```

This creates placeholder books and covers for UI testing. Clear the DB or remove the books before using a real device sync.

## Troubleshooting

- If sync fails behind a proxy, ensure correct public URL and port
- Kobo requires large headers; proxy buffers may need to be increased
- Ensure the device token has not been rotated
