# Kobbler (Alpha)

Kobbler is a self-hosted media library server for ebooks, audiobooks, movies and TV, with a web UI,
a built-in EPUB reader, and a Kobo sync proxy that makes your own books appear on a Kobo e-reader
alongside store purchases. It runs as a handful of Docker containers on one machine and needs no
external accounts or cloud services, with one optional exception: pairing a client app (kobapp,
kobup) by PIN goes through an external relay, `https://pin.biboop.com`, by default. The relay only
carries your server URL and a device token for the lifetime of one PIN. Set
`KOBBLER_PIN_SERVICE_URL` in `.env` to use your own — see [docs/pin-login.md](docs/pin-login.md)
for the trust model and how to self-host it.

Everything beyond that core — text-to-speech, speech-to-text, vision/skip-detection, an LLM chat
assistant, and the companion projects Kobload and koblem — is **optional and off by default**.

> **Alpha.** The API, the config file and the database schema still move. Back up before upgrading.

## Prerequisites

- Linux host with Docker and the `docker compose` plugin
- ~2 GB RAM free and disk for your library
- Nothing else. No GPU, no API keys, no accounts.
- [koblibs](https://github.com/Kobbler-Media/koblibs) cloned as a **sibling directory** of this
  repo. No published image exists yet, so `docker compose up` always builds from source, and the
  build needs the shared koblibs crates and the `@koblibs/ui` npm package from that checkout:

  ```bash
  git clone https://github.com/Kobbler-Media/kobbler.git
  git clone https://github.com/Kobbler-Media/koblibs.git
  # you should end up with ./kobbler and ./koblibs next to each other
  ```

  If you keep koblibs somewhere else, set `KOBLIBS_DIR` in `.env`.

For building from source (contributors only) see [CONTRIBUTING.md](CONTRIBUTING.md).

## Quickstart

```bash
cd kobbler

# 1. Create the config file. (If you skip this the API writes one from its
#    defaults, but the template carries the comments explaining every key.)
cp config/kobbler.toml.example config/kobbler.toml

# 2. Create .env and replace the `changeme` secrets
#    (NATS_TOKEN, MEILI_MASTER_KEY — `openssl rand -hex 32` each).
cp .env.example .env

# 3. Build and start. First run also builds the base image api/Dockerfile
#    needs (Calibre, FFmpeg, chromaprint) — ~5GB, several minutes depending on
#    your connection, cached after that. `make up` builds it automatically if
#    missing; if you'd rather do it yourself, `make tools` first, same thing —
#    skipping straight to `docker compose up --build` without it fails with a
#    confusing "pull access denied" error that has nothing to do with the
#    registry.
make up
```

Then find the generated admin password in the logs:

```bash
docker compose logs api | grep "Admin account"
```

Open the web UI at <http://localhost:5173> and sign in as `admin@localhost`.

The containers use `restart: unless-stopped`, so they come back after a reboot.

Postgres only reads `POSTGRES_PASSWORD` when it first initializes an empty
database. If you change it in `.env` after the first `docker compose up`, the
`db` volume still has the old one and `api` starts crash-looping with
`password authentication failed for user "kobbler"`. Fix by matching `.env`
back to the original password, or `docker compose down -v` to wipe the volume
and reinit from the new one (this deletes your data — only do this before you
have any).

### Before you expose it

Edit `config/kobbler.toml` and set `public_proxy_url` to the address Kobo devices will reach —
Kobo sync URLs are built from it and `http://localhost:6767` only works on the host itself:

```toml
public_proxy_url = "http://192.168.1.100:6767"
```

You can also change this (and everything else in the file) from **Settings > Configuration** in the
web UI; the UI writes back to the same file.

## Ports

| Port | Service | Description |
|------|---------|-------------|
| 5173 | Web UI  | nginx, proxies `/api` to the API internally |
| 6766 | API     | Direct API access (the Calibre plugin uses this) |
| 6767 | Proxy   | Kobo device sync endpoint |

Override with `KOBBLER_WEB_PORT`, `KOBBLER_API_PORT` and `KOBBLER_PROXY_PORT` in `.env`. Every
other environment knob is documented in `.env.example`.

## Search

Search works out of the box: the bundled stack runs Meilisearch for typo-tolerant ranking.

To run without Meilisearch, search falls back to SQL matching (no typo tolerance or relevance
ranking, nothing else changes). Set `meili_url = ""` in `config/kobbler.toml`, then — because
`docker-compose.yml` always passes `MEILI_URL` to the API — add a `docker-compose.override.yml`:

```yaml
services:
  api:
    environment:
      MEILI_URL: ""
    depends_on: !override
      db:
        condition: service_healthy
      nats:
        condition: service_started
  meilisearch:
    profiles: [disabled]
```

The API logs `search is running in SQL-fallback mode` at startup. Semantic search needs
Meilisearch.

## What you get out of the box

- Library of books, audiobooks, movies and TV with metadata, covers, series and people
- Web UI: browse, search, read EPUBs in the browser, play video and audio
- Kobo sync proxy — `merge` mode puts your books next to Kobo store purchases on the device
- Kindle delivery by email
- Calibre plugin ("Send to Kobbler") with automatic KEPUB conversion
- Watch directories and media-root scanning for automatic import
- Metadata lookup from OpenLibrary, Google Books and Hardcover
- Wanted list, IPTV and HDHomeRun live TV, subtitles, backup/restore

## Optional extras (none required)

Each of these is off in the shipped config and stays off until you turn it on and start the
matching service. Kobbler works completely without all of them.

| Feature | Config key | Needs |
|---|---|---|
| Text to speech | `tts_enabled` | a TTS sidecar container |
| Transcription | `transcript_mode` | an STT sidecar container |
| Intro/credit skip detection | `skip_detect_mode` | the vision sidecar |
| Chat assistant | `chat_enabled` | any OpenAI-compatible LLM endpoint you supply |
| Enrichment | `enrichment_enabled` | the above services |
| Semantic search | `semantic_search_enabled` | Meilisearch and an embedding service (`embed-vision`, or koblem) |
| Shared GPU routing | `route_gpu_via_koblem` | koblem (see below) |
| Automated acquisition | `kobload_*` | [Kobload](https://github.com/Kobbler-Media/kobload), a separate project |

[koblem](https://github.com/Kobbler-Media/koblem) is an optional, experimental GPU-services
sidecar tuned for one specific hardware setup. Kobbler runs fully without it, and its settings are
hidden in the UI unless you set `KOBLEM_UI_ENABLED=1` in `.env`.

The GPU sidecars ship as an overlay you opt into explicitly:

```bash
docker compose -f docker-compose.yml -f docker-compose.gpu.yml up -d
```

That needs an NVIDIA GPU and the NVIDIA container toolkit, plus the extra settings at the bottom of
`.env.example`. Plain `docker compose up -d` never touches it. Starting a sidecar doesn't switch its
feature on: point kobbler at it (`stt_service_url`, `tts_service_url`, `vision_service_url`, with
the example values in `config/kobbler.toml.example`) and enable the feature in Settings. All of them
are empty by default, and kobbler leaves each feature off until its URL is set.

`embed-vision` and `embed-text` (EmbeddingGemma-2 embeddings for skip detection, live TV ad
detection and semantic search) run the stock `ghcr.io/ggml-org/llama.cpp:server-cuda` image and
download their GGUFs on first start. `stt-parakeet-cpp` and `tts-breeze` build from public forks of
CrispASR (`PARAKEET_REPO`) and `qwen3-tts.cpp` (`BREEZE_REPO`). The
Breeze model itself has to be converted by hand — see
[docker/tts-breeze/README.md](docker/tts-breeze/README.md).

## Configuration

`config/kobbler.toml` is the single config file. It is deliberately **not** tracked in git: the
Settings UI rewrites it on every save, so it is instance state, not source. The template with every
key and its default is `config/kobbler.toml.example`.

Two ways to change settings:

- **Settings > Configuration** in the web UI (saved immediately, survives restarts)
- Edit `config/kobbler.toml` on the host, then `docker compose restart api`

Frequently-touched keys:

| Setting | Default | Description |
|---------|---------|-------------|
| `public_proxy_url` | `http://localhost:6767` | Public URL for Kobo device sync |
| `kobo_mode` | `merge` | `merge`, `passthrough` or `local` |
| `watcher_enabled` | `false` | Watch directories for auto-import |
| `metadata_providers_enabled` | `true` | External metadata lookups |
| `proxy_logging_enabled` | `true` | Log Kobo proxy requests for debugging |

### Kobo modes

- **merge** (default) — your books appear alongside Kobo store purchases; full store access kept
- **local** — local library only, never contacts Kobo servers
- **passthrough** — pure proxy, useful for debugging

## Device setup

Create a device in **Settings > Devices** to get its sync URL, then on the Kobo edit
`.kobo/Kobo/Kobo eReader.conf`:

```ini
[OneStoreServices]
api_endpoint=<sync_url_from_devices_page>
```

Restart the Kobo and sync.

## Calibre plugin

```bash
./scripts/build_plugin_zip.sh
```

In Calibre: *Preferences > Plugins > Load plugin from file*, then configure:

- Kobbler Base URL: `http://your-server:6766`
- Ingest Token: mint one in **Settings > Ingest Tokens**

Select a book and choose "Send to Kobbler". Missing KEPUB formats are converted automatically.

## Storage

By default Kobbler uses Docker named volumes:

- `kobbler_data` — book files, covers and cache
- `kobbler_pg` — PostgreSQL

Point `KOBBLER_MEDIA_ROOT` in `.env` at your existing media directory; it is mounted at `/media` in
the API container, so library and watch paths you configure in the UI are `/media/...` paths. See
[docs/deployment-storage.md](docs/deployment-storage.md).

## Documentation

- [Quickstart & API examples](docs/quickstart.md)
- [Reverse proxy setup](docs/deployment-reverse-proxy.md) (Nginx Proxy Manager, Traefik, Caddy)
- [Deployment & storage](docs/deployment-storage.md)
- [NAS health monitoring](docs/nas-health-monitoring.md) (CIFS/SMB media shares)
- [PIN app pairing](docs/pin-login.md) (the pairing relay and how to self-host it)
- [Calibre plugin](plugins/calibre/send_to_kobbler/README.md)
- [NATS deployment](docs/nats-deployment.md) (only for multi-host setups)
- [Text to speech: tts-breeze API](docs/tts-breeze.md) and [building it](docker/tts-breeze/README.md)
- [Speech to text: stt-parakeet-cpp](docker/stt-parakeet-cpp/README.md)

## Contributing

Bug reports and pull requests are welcome — see [CONTRIBUTING.md](CONTRIBUTING.md) for how to build,
test and submit changes.

## License

AGPL-3.0. See [LICENSE](LICENSE).
