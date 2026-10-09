# Contributing to Kobbler

Pull requests are welcome — bug fixes, docs, tests, and features alike. If you are planning
something large, open an issue first so we can agree on the shape before you write it.

## Prerequisites

- Rust stable (via [rustup](https://rustup.rs)) and `cargo install sqlx-cli`
- Node 20+ and npm
- Docker and the `docker compose` plugin — the test database runs in Docker
- Python 3 (only for the Calibre plugin lint)

Kobbler depends on shared crates from the **koblibs** repository (`nats-jobs`, `kob-ffmpeg`,
`kob-audio-norm`, `kob-gpu-gate`, `kob-ebook`) and on the `@koblibs/ui` npm package. Clone koblibs
as a sibling directory:

```
your-workspace/
  kobbler/
  koblibs/
```

`api/Cargo.toml` depends on each koblibs crate via a sibling `path = "../../koblibs/<crate>"`
entry, and `web/package.json` resolves `@koblibs/ui` the same way (`file:../../koblibs`) — both
need the sibling checkout above to resolve at all. Docker builds instead take koblibs as an
`additional_contexts` build context, pointed at `${KOBLIBS_DIR:-../koblibs}`; set `KOBLIBS_DIR` if
you keep the checkout somewhere other than next to this repo.

## Setup

```bash
git clone https://github.com/Kobbler-Media/kobbler.git
cd kobbler
make deps      # npm install + a .venv with ruff
```

## Build and test

```bash
make quick   # fmt, lint, type-check, build — use this while working
make         # everything above plus the test suite — run before opening a PR
make web     # frontend only
```

`make` is the gate. It must be green before a PR is ready.

Individual pieces:

| Target | What it does |
|---|---|
| `make fmt` | `cargo fmt`, prettier, ruff format |
| `make lint` | clippy (warnings are errors), eslint, ruff |
| `make check` | `cargo check`, `svelte-check`, sqlx offline-cache check |
| `make test` | backend integration tests (starts a Postgres + NATS via Docker) |
| `make sqlx-prepare` | regenerate `.sqlx/` after changing a query or migration |

Warnings are errors everywhere. `svelte-check` must report zero warnings.

### sqlx offline cache

Queries are checked at compile time. If you add or change a `query!`/`query_as!` macro or a
migration, run `make sqlx-prepare` and commit the updated `.sqlx/` directory. A *failed*
`sqlx-prepare` empties `.sqlx/` before it compiles — fix the build and re-run rather than
committing the wreckage.

## Tests

Backend tests live in `api/tests/` and run against a real Postgres, started for you by
`make test-db-up`. Cover the happy path, the error path, and auth.

**Bug fixes are test-first.** Write a test that reproduces the bug and watch it fail, then fix it,
then watch it pass. Cover every entry point into the broken logic, not just the one in the report.
A fix without a regression test will be asked for one.

Keep tests fast: share fixtures, use table-driven cases for variations, avoid repeating heavy setup.

## Code style

- **No comments by default.** Names and types are the documentation. Comment only a non-obvious
  *why* — a constraint, a workaround, an invariant, a coupling a reader cannot see from here.
  If code needs a comment to be readable, refactor it instead.
- No backwards-compatibility shims or commented-out code. Delete what you replace; git remembers.
- Keep files small and single-purpose. Check for an existing helper before adding a new one.
- Database writes go through `api/src/repositories/`. Background work goes through
  `api/src/jobs/handlers/`, not `tokio::spawn`. HTTP clients come from `crate::http::new_client()`.
- Schema changes touch the frontend types in `web/src/lib/types.ts` and `client.ts` together.
- New tables need an entry in `BACKUP_TABLES` in `api/src/backup.rs`.
- New pages need registering in four places: `Sidebar.svelte`, `MobileMenu.svelte`,
  `get_static_pages()` in `search_index.rs`, and `CommandPalette/registry.ts`.

`AGENTS.md` holds the same conventions in condensed form for coding agents. Machine-specific notes
belong in `CLAUDE.local.md` / `AGENTS.local.md`, which are gitignored.

## Pull requests

- One logical change per PR; keep the diff reviewable.
- `make` green, including tests.
- Describe what changed and why. Link the issue if there is one.
- Note any breaking change to a public API or to the config file explicitly.

## Configuration changes

`config/kobbler.toml` is untracked instance state — never commit it. When you add a config key, add
it to `config/kobbler.toml.example` with a comment and a default that matches the code default in
`api/src/file_config.rs`. The template must describe a working standalone install: anything needing
an optional service defaults to off.

## Licensing

Kobbler is licensed under the **GNU Affero General Public License v3.0** ([LICENSE](LICENSE)). By
submitting a pull request you agree that your contribution is licensed under AGPL-3.0 on the same
terms as the rest of the project. Do not paste in code you do not have the right to relicense.
