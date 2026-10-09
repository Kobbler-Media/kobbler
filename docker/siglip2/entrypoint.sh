#!/bin/sh
# siglip2-entrypoint — first-run GGUF + tokenizer fetch, then exec the server.
#
# On first start the /models volume is empty; we curl the selected quant +
# tokenizer.model directly from HF (no python/hf-cli dep — siglip2.cpp's
# binary is self-contained, no point bloating the runtime image). After the
# first start the named volume holds the cache; subsequent boots skip the
# download and exec the server directly.
#
# Quant selection: SIGLIP2_QUANT (default q8_0). Override at compose time:
#   environment:
#     SIGLIP2_QUANT: q4_k_m
# Switching quants doesn't auto-evict the old file; wipe the volume if you
# want disk back: `docker volume rm kobbler_siglip2-models`.
#
# SIGLIP2_HF_REPO must host GGUFs named siglip2-so400m-naflex-<quant>.gguf
# alongside tokenizer.model.

set -eu

MODELS_DIR="${MODELS_DIR:-/models}"
HF_REPO="${SIGLIP2_HF_REPO:-dbrains/SigLIP2-So400m-Patch16-NaFlex-GGUF}"
QUANT="${SIGLIP2_QUANT:-q8_0}"

GGUF_FILE="siglip2-so400m-naflex-${QUANT}.gguf"
GGUF_PATH="${MODELS_DIR}/${GGUF_FILE}"
TOK_PATH="${MODELS_DIR}/tokenizer.model"

mkdir -p "${MODELS_DIR}"

fetch() {
    src="$1"
    dst="$2"
    if [ -f "$dst" ]; then
        echo "[siglip2] cache hit: $dst"
        return 0
    fi
    url="https://huggingface.co/${HF_REPO}/resolve/main/${src}"
    echo "[siglip2] fetching ${url}"
    # -fL: follow redirects, fail on HTTP error. -C -: resume partial download.
    # --retry: a few retries for transient issues.
    curl -fL --retry 5 --retry-delay 2 -C - -o "${dst}.partial" "${url}"
    mv "${dst}.partial" "${dst}"
}

fetch "${GGUF_FILE}"   "${GGUF_PATH}"
fetch "tokenizer.model" "${TOK_PATH}"

# Override the env defaults so the binary uses what we just staged.
export MODEL_PATH="${GGUF_PATH}"
export TOKENIZER_PATH="${TOK_PATH}"

echo "[siglip2] starting server on ${HOST:-0.0.0.0}:${PORT:-8890}"
exec /usr/local/bin/siglip2-server "$@"
