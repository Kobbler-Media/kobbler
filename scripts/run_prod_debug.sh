#!/usr/bin/env bash
set -euo pipefail

export RUST_LOG="${RUST_LOG:-debug,tower_http=debug}"
export KOBBLER_HTTP_TRACE="${KOBBLER_HTTP_TRACE:-1}"
export KOBBLER_HTTP_TRACE_SCOPE="${KOBBLER_HTTP_TRACE_SCOPE:-kobo}"

exec "$(dirname "$0")/run_prod.sh"
