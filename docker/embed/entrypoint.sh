#!/bin/sh
# embed-entrypoint — first-run GGUF fetch for the stock llama.cpp server image,
# then exec llama-server with the caller's arguments.
#
# Fetches EMBED_MODEL_FILE (and EMBED_MMPROJ_FILE, when set) from
# https://huggingface.co/${EMBED_HF_REPO} into /models unless already there, so
# a bind-mounted weights directory works too. Nothing is baked into the image.
# Drop the volume to re-download: `docker volume rm kobbler_embed-models`.
set -eu

HF_REPO="${EMBED_HF_REPO:-unsloth/embeddinggemma-2-GGUF}"
HF_REV="${EMBED_HF_REVISION:-main}"
MODELS_DIR="${EMBED_MODELS_DIR:-/models}"

fetch() {
    dst="${MODELS_DIR}/$1"
    if [ -s "$dst" ]; then
        echo "[embed] cache hit: $dst"
        return
    fi
    url="https://huggingface.co/${HF_REPO}/resolve/${HF_REV}/$1"
    echo "[embed] fetching ${url}"
    mkdir -p "$MODELS_DIR"
    curl -fL --retry 3 -o "${dst}.part" "$url"
    mv "${dst}.part" "$dst"
}

fetch "${EMBED_MODEL_FILE:?EMBED_MODEL_FILE is required}"
if [ -n "${EMBED_MMPROJ_FILE:-}" ]; then
    fetch "$EMBED_MMPROJ_FILE"
fi

exec /app/llama-server "$@"
