# Kobbler (Alpha)

Kobbler is a self-hosted media library server for ebooks, audiobooks, movies and TV, with a web UI,
a built-in EPUB reader, and a Kobo sync proxy that makes your own books appear on a Kobo e-reader
alongside store purchases. It runs as a handful of Docker containers on one machine and needs no
external accounts or cloud services, with one optional exception: pairing a client app (kobapp,
kobup) uses a small stateless pairing relay by default — see
[docs/pin-login.md](docs/pin-login.md) for its trust model and how to self-host it instead.

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

Search works out of the box with no extra setup. The bundled stack runs Meilisearch
for typo-tolerant ranking; leave `meili_url` empty in `config/kobbler.toml` and the
API falls back to SQL matching instead. Nothing else changes — you just lose typo
tolerance and relevance ranking.

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
| Shared GPU routing | `route_gpu_via_koblem` | [koblem](https://github.com/Kobbler-Media/koblem), a separate project |
| Automated acquisition | `kobload_*` | [Kobload](https://github.com/Kobbler-Media/kobload), a separate project |

The GPU sidecars ship as an overlay you opt into explicitly:

```bash
docker compose -f docker-compose.yml -f docker-compose.gpu.yml up -d
```

That needs an NVIDIA GPU and the NVIDIA container toolkit, plus the extra settings at the bottom of
`.env.example`. Plain `docker compose up -d` never touches it.

Of the three sidecars, only `stt-parakeet-cpp` builds from a public upstream by default
(`PARAKEET_REPO`). `tts-breeze` and `siglip2` have no public `*_REPO` default — their ggml ports
aren't published anywhere yet — so building them needs your own `qwen3-tts.cpp` / `siglip2.cpp`
checkout, pointed at via `*_REPO` or the local-working-tree `SIGLIP2_SRC`-style override.

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
- [Calibre plugin](plugins/calibre/send_to_kobbler/README.md)
- [NATS deployment](docs/nats-deployment.md) (only for multi-host setups)

## Contributing

Bug reports and pull requests are welcome — see [CONTRIBUTING.md](CONTRIBUTING.md) for how to build,
test and submit changes.

## License

AGPL-3.0. See [LICENSE](LICENSE).
