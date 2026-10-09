# syntax=docker/dockerfile:1.7
#
# siglip2 — SigLIP-2 image/text embeddings on ggml. Holds the
# so400m-patch16-naflex encoder + multilingual tokenizer + sigmoid scoring in
# ~1.5-1.7 GB VRAM at Q8_0, and reclaims to 0 MiB on idle-unload via
# worker-isolation SIGKILL.
#
#   stage 1 (builder): CUDA devel image, ccache, cmake build of the
#       `siglip2-server` binary with CUDA + flash-attn + multi-arch PTX.
#
#   stage 2 (runtime): CUDA runtime image + binary + curl. The entrypoint
#       downloads the GGUF + tokenizer from SIGLIP2_HF_REPO on first start into
#       the siglip2-models volume, then execs siglip2-server.
#
# Source delivery: clone SIGLIP2_REPO @ SIGLIP2_REF, or set SIGLIP2_SRC to a
# local siglip2.cpp checkout and the additional_contexts probe in the compose
# overlay routes that working tree into the COPY below instead.

# ─── build stage ──────────────────────────────────────────────────────────────
FROM nvidia/cuda:12.9.1-devel-ubuntu24.04 AS builder

ENV DEBIAN_FRONTEND=noninteractive

RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt/lists,sharing=locked \
    rm -f /etc/apt/apt.conf.d/docker-clean \
    && apt-get update -o Acquire::Retries=3 \
    && apt-get install -y --no-install-recommends \
        build-essential cmake git ca-certificates curl ccache pkg-config

# Multi-arch PTX so one image runs across GPU generations. Each arch costs
# ~15-20 MiB of resident PTX/cubin per CUDA context, so trim this to just your
# card (86 = Ampere, 89 = Ada, 120 = Blackwell) if VRAM is tight.
ARG SIGLIP2_CUDA_ARCHS="86;89;120"

ARG SIGLIP2_REPO=https://github.com/dbrain/hbd-siglip2.cpp.git
ARG SIGLIP2_REF=6bd2ceb

# Two-mode source delivery:
#   1. Production / CI / fresh hosts: clone SIGLIP2_REPO @ SIGLIP2_REF
#      (resolved via the ARGs above).
#   2. Dev iteration: compose passes
#      `--build-context siglip2_src=/path/to/siglip2.cpp`. The COPY below
#      pulls that working tree (with submodule files in place since the host
#      checkout is populated) and we skip the clone. Switch detected by the
#      presence of CMakeLists.txt in the staged context — compose's default
#      siglip2_src is `docker-image://scratch` which copies an empty FS.
WORKDIR /src
COPY --from=siglip2_src . /tmp/siglip2_src_probe/
RUN if [ -f /tmp/siglip2_src_probe/CMakeLists.txt ]; then \
        echo "==> using local build context (dev iteration)"; \
        mv /tmp/siglip2_src_probe siglip2.cpp; \
    else \
        echo "==> cloning ${SIGLIP2_REPO} @ ${SIGLIP2_REF}"; \
        rm -rf /tmp/siglip2_src_probe \
        && git clone --filter=blob:none --also-filter-submodules \
             --recurse-submodules ${SIGLIP2_REPO} siglip2.cpp \
        && cd siglip2.cpp \
        && git checkout ${SIGLIP2_REF} \
        && git submodule sync --recursive \
        && git submodule update --init --recursive; \
    fi

ENV CCACHE_DIR=/root/.ccache CCACHE_MAXSIZE=20G

# GGML_CUDA_FA=ON     — flash-attention CUDA kernels (encoder benefits)
# GGML_CUDA_GRAPHS=ON — kernel-launch coalescing; the bypass-scheduler
#                      graph cache in TextEncoder/VisionEncoder relies on
#                      stable tensor pointers + this flag to warm up the
#                      CUDA-graphs path after 2 calls per cached shape.
RUN --mount=type=cache,target=/root/.ccache \
    cmake -S /src/siglip2.cpp -B /src/siglip2.cpp/build \
        -DCMAKE_BUILD_TYPE=Release \
        -DCMAKE_CXX_COMPILER_LAUNCHER=ccache \
        -DCMAKE_CUDA_COMPILER_LAUNCHER=ccache \
        -DSIGLIP2_SERVER=ON \
        -DGGML_CUDA=ON \
        -DGGML_CUDA_FA=ON \
        -DGGML_CUDA_GRAPHS=ON \
        -DCMAKE_CUDA_ARCHITECTURES="${SIGLIP2_CUDA_ARCHS}" \
    && cmake --build /src/siglip2.cpp/build --target siglip2-server -j"$(nproc)" \
    && ccache -s


# ─── runtime stage ────────────────────────────────────────────────────────────
FROM nvidia/cuda:12.9.1-runtime-ubuntu24.04

ENV DEBIAN_FRONTEND=noninteractive

# nvidia/cuda:runtime carries libcudart + libcublas + libcublasLt + libcurand.
# Extras:
#   libgomp1        — ggml uses OpenMP; libgomp.so.1 isn't in cuda:runtime
#   curl            — healthcheck + first-run GGUF download from HF
#   ca-certificates — HTTPS to huggingface.co
RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt/lists,sharing=locked \
    rm -f /etc/apt/apt.conf.d/docker-clean \
    && apt-get update -o Acquire::Retries=3 \
    && apt-get install -y --no-install-recommends \
        curl libgomp1 ca-certificates

COPY --from=builder /src/siglip2.cpp/build/siglip2-server /usr/local/bin/siglip2-server
COPY entrypoint.sh /usr/local/bin/siglip2-entrypoint
RUN chmod +x /usr/local/bin/siglip2-entrypoint

# Defaults sized for sharing a single consumer GPU with other workloads.
#   - q8_0 — cosine >= 0.9995 against the reference HF model on real images.
#     Override at run time: SIGLIP2_QUANT=q4_k_m / q5_k_m / f16.
#   - LAZY_LOAD=1 — no VRAM until the first request.
#   - IDLE_UNLOAD_SECONDS=300 — free the GPU after 5 min idle.
#   - SIGLIP2_WORKER_ISOLATION=1 — the fork worker holds the CUDA primary
#     context; /unload + idle-unload SIGKILL it so VRAM reclaims to 0 MiB
#     instead of leaving the cuBLAS workspace + cubin cache resident.
#
# SIGLIP2_HF_REPO must host GGUF conversions named
# siglip2-so400m-naflex-<quant>.gguf plus tokenizer.model (see entrypoint.sh).
ENV SIGLIP2_QUANT=q8_0 \
    SIGLIP2_HF_REPO=dbrains/SigLIP2-So400m-Patch16-NaFlex-GGUF \
    LAZY_LOAD=1 \
    IDLE_UNLOAD_SECONDS=300 \
    SIGLIP2_WORKER_ISOLATION=1 \
    HOST=0.0.0.0 \
    PORT=8890 \
    MODEL_PATH=/models/siglip2-so400m-naflex-q8_0.gguf \
    TOKENIZER_PATH=/models/tokenizer.model

EXPOSE 8890

HEALTHCHECK --interval=30s --timeout=10s --start-period=180s \
    CMD curl -sf http://localhost:8890/health || exit 1

ENTRYPOINT ["/usr/local/bin/siglip2-entrypoint"]
