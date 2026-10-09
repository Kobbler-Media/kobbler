# syntax=docker/dockerfile:1.7
#
# BreezeTTS 2 text-to-speech, built from a qwen3-tts.cpp source tree (the
# `breeze-server` CMake target). Set BREEZE_REPO/BREEZE_REF to point at one.
#
# Breeze-TTS-2 is research / non-commercial licensed (RESONIA, INC.). Check the
# licence before using this for anything but personal evaluation. Weights are
# never baked into the image: one GGUF holds all four models (text encoder,
# backbone, depth decoder, audio tokenizer) and is bind-mounted at /models.
#
# The runtime stage needs the libav shared objects, not just the -dev packages:
# breeze-server encodes mp3 through ffmpeg (response_format=mp3, Kobbler's
# preview default) and dies at exec without libavcodec.so.60.
#
# BREEZE_REF is used as the git-clone cache key, so changing it forces a rebuild.

# ─── build stage ──────────────────────────────────────────────────────────────
FROM nvidia/cuda:13.3.0-devel-ubuntu24.04 AS builder

ENV DEBIAN_FRONTEND=noninteractive

# libav*-dev is needed for cmake to CONFIGURE the project (the qwen3-tts-server
# target in the same CMakeLists find_package's it) even though breeze-server
# does not link it.
RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt/lists,sharing=locked \
    rm -f /etc/apt/apt.conf.d/docker-clean \
    && apt-get update -o Acquire::Retries=3 \
    && apt-get install -y --no-install-recommends \
        build-essential cmake git ca-certificates curl ccache \
        pkg-config \
        libavcodec-dev libavformat-dev libavutil-dev

# No public upstream ships the breeze-server target; supply your own
# qwen3-tts.cpp checkout via BREEZE_REPO (and pin BREEZE_REF to a commit).
ARG BREEZE_REPO=https://github.com/dbrain/hbd-qwen3-tts.cpp.git
ARG BREEZE_REF=d022511
# CUDA compute capabilities to emit: 86 = Ampere, 89 = Ada, 120 = Blackwell.
# Trim to just your card to save ~15-20 MiB of resident PTX per omitted arch.
ARG BREEZE_CUDA_ARCHS="86;89;120"

WORKDIR /src
RUN git clone --filter=blob:none --also-filter-submodules \
        --recurse-submodules ${BREEZE_REPO} qwen3-tts.cpp \
    && cd qwen3-tts.cpp \
    && git checkout ${BREEZE_REF} \
    && git submodule sync --recursive \
    && git submodule update --init --recursive

ENV CCACHE_DIR=/root/.ccache CCACHE_MAXSIZE=20G
RUN --mount=type=cache,target=/root/.ccache \
    cmake -S /src/qwen3-tts.cpp -B /src/qwen3-tts.cpp/build \
        -DCMAKE_BUILD_TYPE=Release \
        -DCMAKE_CXX_COMPILER_LAUNCHER=ccache \
        -DCMAKE_CUDA_COMPILER_LAUNCHER=ccache \
        -DQWEN3_TTS_SERVER=ON \
        -DGGML_CUDA=ON \
        -DGGML_CUDA_FA=ON \
        -DGGML_CUDA_GRAPHS=ON \
        -DCMAKE_CUDA_ARCHITECTURES="${BREEZE_CUDA_ARCHS}" \
    && cmake --build /src/qwen3-tts.cpp/build --target breeze-server -j"$(nproc)" \
    && ccache -s

# ─── runtime stage ────────────────────────────────────────────────────────────
FROM nvidia/cuda:13.3.0-runtime-ubuntu24.04

ENV DEBIAN_FRONTEND=noninteractive

RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt/lists,sharing=locked \
    rm -f /etc/apt/apt.conf.d/docker-clean \
    && apt-get update -o Acquire::Retries=3 \
    && apt-get install -y --no-install-recommends \
        curl libgomp1 ca-certificates \
        libavcodec60 libavformat60 libavutil58

COPY --from=builder /src/qwen3-tts.cpp/build/breeze-server /usr/local/bin/breeze-server

EXPOSE 8203

# Worker isolation: the forked child owns the engine; idle-unload and
# /v1/admin/unload SIGKILL it → CUDA context torn down → VRAM true-0. The
# koblem TTS gate POSTs /v1/admin/unload to swap engines.
ENV BREEZE_WORKER_ISOLATION=1 \
    BREEZE_IDLE_UNLOAD_SECONDS=300 \
    BREEZE_VOICES_DIR=/app/voices

# Read-along forced alignment (own CUDA context, own idle-unload, ~0.6-0.8 GB
# while a paragraph streams). Set BREEZE_FA_MODEL="" to disable — aligned-stream
# requests then degrade to plain SSE audio with no highlight.
# BREEZE_TEXT_ENC_CPU=1 by default: the text encoder is 926 MiB (40% of the
# weights) and runs once per request for ~1% of the runtime, so keeping it in
# host RAM is the cheapest VRAM on the card. Costs ~1.7% RTF, bit-identical
# audio. Set it to 0 if the card is dedicated.
ENV BREEZE_TEXT_ENC_CPU=1 \
    BREEZE_FA_MODEL=/models/qwen3-forced-aligner-0.6b-q4_k.gguf \
    BREEZE_ALIGNER_IDLE_UNLOAD_SECONDS=20 \
    CRISPASR_KV_QUANT=q8_0

# Building the GGUF: gguf-py cannot write K-quants, so it is a two-step
# conversion — convert to f16, then requantise with the in-tree tool.
#   scripts/convert_breeze_to_gguf.py --input <HF dir> --type f16 \
#       --output /models/breeze-tts-2-v2-f16.gguf
#   build/gguf_quantize /models/breeze-tts-2-v2-f16.gguf \
#       /models/breeze-tts-2-v2-Q4_K_M.gguf Q4_K_M
# The HF dir MUST include audio_tokenizer/model.safetensors — a separate 682 MB
# file the top-level download misses. The converter fails loudly without it.
#
# Q4_K_M is the default: fewer bits does not mean faster here, Q6_K and Q3_K
# depth both benchmark slower than Q8_0 despite being smaller.
HEALTHCHECK --interval=30s --timeout=10s --start-period=60s \
    CMD curl -sf http://localhost:8203/health || exit 1

ENTRYPOINT ["/bin/sh", "-c", "exec /usr/local/bin/breeze-server \
    --model \"/models/${BREEZE_MODEL:-breeze-tts-2-v2-Q4_K_M.gguf}\" \
    --n-ctx \"${BREEZE_N_CTX:-2048}\" \
    ${BREEZE_FA_MODEL:+--aligner-model \"$BREEZE_FA_MODEL\"} \
    -H 0.0.0.0 -p 8203 ${BREEZE_EXTRA_ARGS:-}"]
