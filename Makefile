.PHONY: all quick web up deps deps-web deps-plugin fmt fmt-api fmt-web fmt-plugin lint lint-api lint-web lint-plugin check check-api check-web check-plugin sqlx-check sqlx-prepare sqlx-migrate build build-api build-web test test-api test-web test-db-up test-db-down docker-push player tools sweep

# Prerequisites for a fresh clone:
#   - Rust stable + `cargo install sqlx-cli` (for the sqlx-check / sqlx-prepare targets)
#   - Node 20+ and npm (web)
#   - Docker + `docker compose` (test database)
#   - Python 3 with `ruff` in a local .venv (the Calibre plugin lint) — `make deps-plugin`
# `make deps` sets up everything this repo can set up for you.
all: sweep fmt lint check build test

# Quick build check - skips slow backend tests
quick: sweep fmt lint check build

# One-time setup for a fresh clone.
deps: deps-web deps-plugin

deps-web:
	npm --prefix web install

deps-plugin:
	python3 -m venv .venv
	.venv/bin/pip install --upgrade pip ruff

# Web-only checks - for frontend-only changes
web: fmt-web lint-web check-web build-web

fmt: fmt-api fmt-web fmt-plugin
lint: lint-api lint-web lint-plugin
check: check-api check-web check-plugin sqlx-check
build: build-api build-web

fmt-api:
	cargo fmt --manifest-path api/Cargo.toml

lint-api:
	SQLX_OFFLINE=true cargo clippy --manifest-path api/Cargo.toml --all-targets --all-features -- -D warnings

check-api:
	SQLX_OFFLINE=true cargo check --manifest-path api/Cargo.toml

# Verify .sqlx cache matches code. If stale, run: make sqlx-prepare
# `-- --all-targets` so test-only query macros (api/tests/) are included; without
# it, `lint-api` (clippy --all-targets) surfaces them as SQLX_OFFLINE cache misses.
# `--all` so koblibs' nats-jobs (a dep, not a workspace member) is force-recompiled
# and re-emits its job_status queries. Without it those 9 entries appear or vanish
# depending only on whether cargo happened to rebuild the dep, and get "pruned as
# unreachable" then restored forever.
sqlx-check: test-db-up sqlx-migrate
	DATABASE_URL=postgres://kobbler:kobbler@localhost:5433/kobbler_test cargo sqlx prepare --workspace --all --check -- --all-targets

# Regenerate .sqlx cache from current code + DB schema
sqlx-prepare: test-db-up sqlx-migrate
	DATABASE_URL=postgres://kobbler:kobbler@localhost:5433/kobbler_test cargo sqlx prepare --workspace --all -- --all-targets

# Apply migrations to the test database
sqlx-migrate: test-db-up
	DATABASE_URL=postgres://kobbler:kobbler@localhost:5433/kobbler_test cargo sqlx migrate run --source api/migrations

# No RUSTFLAGS anywhere here: .cargo/config.toml sets the tokio cfg for every cargo run,
# and a step with flags of its own is a separate build of the whole dependency tree.
# Warnings are denied by lint-api, whose clippy pass covers rustc's warnings too.
build-api:
	SQLX_OFFLINE=true cargo build --manifest-path api/Cargo.toml

fmt-web:
	npm --prefix web run format

lint-web:
	npm --prefix web run lint

check-web:
	npm --prefix web run check

build-web:
	npm --prefix web run build

# Uses the local .venv created by `make deps-plugin`; falls back to a ruff on PATH.
RUFF := $(shell test -x .venv/bin/ruff && echo .venv/bin/ruff || command -v ruff)

fmt-plugin:
	@test -n "$(RUFF)" || { echo "ruff not found - run 'make deps-plugin'"; exit 1; }
	$(RUFF) format plugins/calibre/send_to_kobbler

lint-plugin:
	@test -n "$(RUFF)" || { echo "ruff not found - run 'make deps-plugin'"; exit 1; }
	$(RUFF) check plugins/calibre/send_to_kobbler

check-plugin: lint-plugin

test: test-api test-web

test-web:
	npm --prefix web test

test-db-up:
	docker compose -f docker-compose.test.yml up -d --wait

test-db-down:
	docker compose -f docker-compose.test.yml down

test-api: test-db-up
	@RUN_ID="$$(date +%s)_$$$$" && \
	DATABASE_URL_TEST=postgres://kobbler:kobbler@localhost:5433/kobbler_test \
	NATS_URL_TEST=nats://localhost:4223 \
	TEST_RUN_ID="$$RUN_ID" \
	CARGO_INCREMENTAL=0 \
	cargo test --manifest-path api/Cargo.toml -- --test-threads=4

# Maintainer target: publishes the release images. Needs push rights on the
# registry (`docker login ghcr.io`). Same variable docker-compose.yml uses, so
# a fork can set KOBBLER_REGISTRY once in .env and it applies to both.
KOBBLER_REGISTRY ?= ghcr.io/kobbler-media
ifdef REGISTRY
export KOBBLER_REGISTRY := $(REGISTRY)
endif
KOBBLER_TAG ?= latest
ifdef TAG
export KOBBLER_TAG := $(TAG)
endif

# kobapp's Compose web build, served by the web image at /player/.
KOBAPP_DIR ?= ../kobapp
player:
	cd $(KOBAPP_DIR) && ./gradlew :composeApp:wasmJsBrowserDistribution
	rm -rf web/static/player
	cp -r $(KOBAPP_DIR)/composeApp/build/dist/wasmJs/productionExecutable web/static/player

docker-push: player
	@docker image inspect kobbler-tools:latest >/dev/null 2>&1 || $(MAKE) tools
	docker compose build api web
	docker compose push api web
	@echo ""
	@echo "=== Pushed digests (compare with server: docker inspect <image> --format '{{.RepoDigests}}') ==="
	@docker inspect $(KOBBLER_REGISTRY)/kobbler:$(KOBBLER_TAG) --format '  api: {{index .RepoDigests 0}}' 2>/dev/null || true
	@docker inspect $(KOBBLER_REGISTRY)/kobbler-web:$(KOBBLER_TAG) --format '  web: {{index .RepoDigests 0}}' 2>/dev/null || true
	@echo ""
	@echo "Pruning old build artifacts..."
	docker image prune -f --filter "dangling=true"

# Skipped when cargo-sweep isn't installed so a fresh clone can still run `make`.
# Above what one full `make` leaves (~15 GB): a lower cap deletes artifacts the next run
# only builds again.
sweep:
	@command -v cargo-sweep >/dev/null 2>&1 && cargo sweep --maxsize 25GB . || \
		echo "sweep: cargo-sweep not installed, skipping (cargo install cargo-sweep)"

tools:
	docker build -f docker/tools.Dockerfile -t kobbler-tools:latest .

# api/Dockerfile is FROM kobbler-tools:latest, which docker compose has no way
# to build on its own — skip `make tools` and a compose build (here or in
# docker-push) fails
# with a "pull access denied" error that reads like a registry auth problem.
# This target builds it first if missing, then starts the stack.
up:
	@docker image inspect kobbler-tools:latest >/dev/null 2>&1 || $(MAKE) tools
	docker compose up -d --build
