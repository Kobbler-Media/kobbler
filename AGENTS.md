Self-hosted media library (books, movies, TV). Kobo sync, Kindle delivery, web EPUB reader, optional automated acquisition via Kobload. API prefix `/api/v1/admin/` is historical — normal authenticated endpoints.

Machine-specific notes (your build box, your deployment hosts, your paths) do not belong here — put them in `CLAUDE.local.md` / `AGENTS.local.md`, which are gitignored.

## Session artifacts stay out of the tree

This tree is code plus real documentation. Handoff notes, results, prompts, findings and
investigation/status writeups (`HANDOFF-*`, `RESULTS-*`, `PROMPT-*`, `FINDINGS-*`, `PERF-*`,
`DISCUSSION-*`) are scratch — keep them outside the repo. Genuine reference docs (READMEs,
`docs/`, PORT-SPEC/API/RUNBOOK specs) stay in-repo.

## Build

- `make deps` — one-time setup for a fresh clone (npm install, ruff venv).
- `make` — fmt, lint, check, build, tests. Required before done.
- `make quick` — No tests. Use during dev.
- `make web` — Frontend only.
- `make sqlx-prepare` — Regenerate `.sqlx` cache after query/migration changes.

Warnings are errors (`lint-api` denies them). `--cfg tokio_unstable` is required and
`.cargo/config.toml` sets it for every cargo run — don't set `RUSTFLAGS` yourself: any other flag
set is a second full build of every dependency in `target/`.
Clippy `collapsible_if`: use let-chains, not nested `if let`.

`make` is expensive — use `make quick` during dev, `make` once at the end.

## Build requires Docker

`sqlx-check` starts the test DB via Docker. Stale cache → `make sqlx-prepare`.

`.sqlx/` holds 9 `job_status` entries owned by koblibs' `nats-jobs`, a dependency rather than a
workspace member. They are not stale — do not "prune" them. `sqlx-prepare`/`sqlx-check` pass
`--all` so the dep is force-recompiled and re-emits them every run; drop that flag and they vanish
and reappear at random.

A *failed* `sqlx-prepare` leaves `.sqlx` truncated — sqlx-cli empties the directory before it
compiles. Fix the build and re-run; never commit the wreckage.

## `config/kobbler.toml` is untracked instance state

It is live config, not source: compose bind-mounts `./config:/config` and the Settings UI rewrites
it on every save, so a tracked copy would carry each machine's hostnames and keys into the repo.
The template is `config/kobbler.toml.example` — add new keys there, never real values.

If `config/kobbler.toml` is missing, the API writes one from `FileConfig::default()` and boots
(`FileConfig::load_or_create`). On a fresh checkout still run
`cp config/kobbler.toml.example config/kobbler.toml` — the generated file has no comments.

## Nav sync (4 places)

New pages need: `Sidebar.svelte`, `MobileMenu.svelte`, `search_index.rs` → `get_static_pages()`,
`CommandPalette/registry.ts`. Bump `SEARCH_SCHEMA_VERSION` if search fields changed.

## Patterns

- Tests in `api/tests/integration/`, one test binary: add a module and its `mod` line in `main.rs`, never a new top-level `api/tests/*.rs`. Filter with `cargo test --test integration <module>::`. Happy path + error + auth.
- Bug fixes are test-first: a failing test that reproduces the bug, then the fix.
- INSERT/UPDATE/DELETE via `api/src/repositories/`.
- Jobs via `api/src/jobs/handlers/`, not `tokio::spawn`. Use `enqueue_with_context()` for UI visibility.
- HTTP via `crate::http::new_client()`.
- sqlx: prefer `query!`/`query_as!` macros. Runtime `sqlx::query()` only for dynamic SQL.
- Backup: new tables → update `api/src/backup.rs` `BACKUP_TABLES`.
- Schema changes: update frontend types in `web/src/lib/types.ts` and `client.ts` together.
- Frontend: `svelte-check` zero warnings.
- Comments are for non-obvious *why*, not for restating what the code does.

## Layout

- `api/` — Axum backend (`src/routes/`, `src/repositories/`, `src/jobs/`, `migrations/`, `tests/`).
- `kobo-proxy/` — Kobo sync protocol proxy.
- `libs/stk-client/` — shared client crate.
- `web/` — SvelteKit frontend.
- `plugins/calibre/send_to_kobbler/` — Calibre "Send to Kobbler" plugin (Python).
- `docker/`, `docker-compose*.yml` — service definitions, including the optional GPU sidecars.
- `scripts/` — dev/ops helpers.

## Related projects

kobbler (this), kobload, koblibs, koblem. All optional except koblibs, which supplies shared Rust
crates (`nats-jobs`, `kob-ffmpeg`, `kob-audio-norm`, `kob-gpu-gate`, `kob-ebook`) and the
`@koblibs/ui` npm package.
