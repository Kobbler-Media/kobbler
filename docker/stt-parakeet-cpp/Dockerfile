# syntax=docker/dockerfile:1.7
#
# stt-parakeet-cpp — Parakeet-TDT speech-to-text on ggml, built from CrispASR.
# Holds the encoder + TDT decoder graph in ~600 MB of VRAM at Q4_K (~711 MB at
# Q8_0, ~1.26 GB at F16), with no Python/NeMo/Torch runtime.
#
#   stage 1 (builder): CUDA devel image, ccache, cmake build of the `crispasr`
#       CLI binary with CUDA + flash-attn + multi-arch PTX. Its --server mode is
#       what the container runs.
#
#   stage 2 (runtime): CUDA runtime image + binary + ffmpeg shared libs.
#       PARAKEET_LAZY_LOAD + PARAKEET_IDLE_UNLOAD_SECONDS handle load/unload and
#       idle behaviour inside the binary.
#
# PARAKEET_REF pins the source commit so a new upstream commit doesn't silently
# reshape a rebuild; bump it deliberately.

# ─── build stage ──────────────────────────────────────────────────────────────
FROM nvidia/cuda:12.9.1-devel-ubuntu24.04 AS builder

ENV DEBIAN_FRONTEND=noninteractive

RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt/lists,sharing=locked \
    rm -f /etc/apt/apt.conf.d/docker-clean \
    && apt-get update -o Acquire::Retries=3 \
    && apt-get install -y --no-install-recommends \
        build-essential cmake git ca-certificates curl ccache \
        pkg-config \
        libavformat-dev libavcodec-dev libavutil-dev libswresample-dev

# Multi-arch PTX so one image runs across GPU generations. Each arch costs
# ~15-20 MiB of resident PTX/cubin per CUDA context, so trim this to just your
# card (86 = Ampere, 89 = Ada, 120 = Blackwell) if VRAM is tight.
ARG PARAKEET_CUDA_ARCHS="86;89;120"

# Force the MMQ (INT8 MMA tensor-core) path for quantized matmuls regardless of
# M. The default CUDA heuristic is `mmq if ne11 < 64 else dequant -> cuBLAS
# half-GEMM`, so at parakeet's encoder width every q4_k matmul falls through to
# cuBLAS. Set to ON to benchmark the alternative.
ARG PARAKEET_FORCE_MMQ="OFF"

ARG PARAKEET_REPO=https://github.com/dbrain/hbd-CrispASR.git
ARG PARAKEET_REF=457252dd
WORKDIR /src
RUN git clone --filter=blob:none --also-filter-submodules \
        --recurse-submodules ${PARAKEET_REPO} parakeet.cpp \
    && cd parakeet.cpp \
    && git checkout ${PARAKEET_REF} \
    && git submodule sync --recursive \
    && git submodule update --init --recursive

ENV CCACHE_DIR=/root/.ccache CCACHE_MAXSIZE=20G

# GGML_CUDA_FA=ON       — flash attention CUDA kernels (encoder benefits)
# GGML_CUDA_GRAPHS=ON   — kernel-launch coalescing (small win for parakeet
#                         decoder; compile in, runtime can disable)
# GGML_CUDA_NO_VMM=ON   — skip CUDA VMM (cuMemMap/cuMemCreate). On the dev
#                         nvidia/cuda:12.9.1-devel image CMake's CUDA::cuda_driver
#                         target doesn't pick up the libcuda stub, so the link
#                         step fails with undefined references to cuMem*. We
#                         don't need VMM for a 0.6B model — physical VRAM is
#                         abundant relative to model size — so disabling it
#                         skips the libcuda dep entirely. If a larger backend
#                         (granite/voxtral, multi-GB) ever needs VMM here we'll
#                         add `-L /usr/local/cuda/lib64/stubs -lcuda` instead.
# GGML_CUDA_NCCL=OFF    — single-GPU server, NCCL is multi-GPU collectives.
RUN --mount=type=cache,target=/root/.ccache \
    cmake -S /src/parakeet.cpp -B /src/parakeet.cpp/build \
        -DCMAKE_BUILD_TYPE=Release \
        -DCMAKE_CXX_COMPILER_LAUNCHER=ccache \
        -DCMAKE_CUDA_COMPILER_LAUNCHER=ccache \
        -DGGML_CUDA=ON \
        -DGGML_CUDA_FA=ON \
        -DGGML_CUDA_GRAPHS=ON \
        -DGGML_CUDA_NO_VMM=ON \
        -DGGML_CUDA_NCCL=OFF \
        -DGGML_CUDA_FORCE_MMQ="${PARAKEET_FORCE_MMQ}" \
        -DCMAKE_CUDA_ARCHITECTURES="${PARAKEET_CUDA_ARCHS}" \
        -DCRISPASR_BUILD_TESTS=OFF \
        -DCRISPASR_FFMPEG=ON \
    && cmake --build /src/parakeet.cpp/build --target crispasr-cli -j"$(nproc)" \
    && ccache -s

# Stage the binary + its shared libs into a clean prefix the runtime stage
# can copy in one shot. CrispASR + ggml build as a fan of shared libraries
# (libcrispasr.so, libggml.so, libggml-base.so, libggml-cpu.so, libggml-cuda.so),
# so the runtime needs all of them on its rpath / ldconfig cache.
RUN cmake --install /src/parakeet.cpp/build --prefix /opt/crispasr \
        --component "" >/dev/null 2>&1 || true \
    && mkdir -p /opt/crispasr/bin /opt/crispasr/lib \
    && cp /src/parakeet.cpp/build/bin/crispasr /opt/crispasr/bin/ \
    && find /src/parakeet.cpp/build -name 'libcrispasr*.so*' -o -name 'libggml*.so*' \
        | xargs -I {} cp -P {} /opt/crispasr/lib/ \
    && ls -la /opt/crispasr/bin /opt/crispasr/lib


# ─── runtime stage ────────────────────────────────────────────────────────────
FROM nvidia/cuda:12.9.1-runtime-ubuntu24.04

ENV DEBIAN_FRONTEND=noninteractive

# libgomp1     — ggml is built with OpenMP; libgomp.so.1 not in cuda:runtime
# libsndfile1   — read_audio_data() WAV path
# ffmpeg        — pulls in libavcodec / libavformat / libavutil / libswresample
#                 shared libs (the apt pkg has them as deps). With
#                 CRISPASR_FFMPEG=ON the binary now decodes mp3/m4a/aac in
#                 process via libav* — matches python's ffmpeg-cli decode
#                 byte-for-byte (incl. LAME encoder-delay stripping), so word
#                 timestamps are sub-millisecond consistent across stacks
#                 (a drmp3 decode path leaves 1216 leading padding samples =
#                 76 ms in the PCM buffer, drifting read-along relative to
#                 ffmpeg-decoded transcripts).
# curl          — healthcheck AND first-run GGUF download (CrispASR shells out
#                 to `curl -fL` for HF fetches when CRISPASR_USE_LIBCURL is unset,
#                 which is the build default — see src/crispasr_cache.cpp:393).
# ca-certificates — HTTPS for huggingface.co
RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt/lists,sharing=locked \
    rm -f /etc/apt/apt.conf.d/docker-clean \
    && apt-get update -o Acquire::Retries=3 \
    && apt-get install -y --no-install-recommends \
        curl libsndfile1 libgomp1 ca-certificates \
        ffmpeg

COPY --from=builder /opt/crispasr/bin/crispasr /usr/local/bin/crispasr
COPY --from=builder /opt/crispasr/lib/ /usr/local/lib/
RUN ldconfig

ENV HF_HOME=/root/.cache/huggingface

EXPOSE 8023

# Defaults sized for sharing a single consumer GPU with other workloads:
#   - q4_k — smallest VRAM footprint. Quant IDs are matched case-sensitively
#     against HF filenames (cstr/parakeet-tdt-0.6b-v3-GGUF uses lowercase), so
#     always pass PARAKEET_MODEL_QUANT lowercase: q4_k / q5_0 / q8_0 / f16.
#   - LAZY_LOAD=1 — no VRAM until the first request.
#   - IDLE_UNLOAD=300s — free the GPU after 5 min idle; long enough to absorb
#     short bursts without re-paying ~1s of cold-start mel + encoder warmup.
ENV PARAKEET_MODEL_QUANT=q4_k \
    PARAKEET_LAZY_LOAD=1 \
    PARAKEET_IDLE_UNLOAD_SECONDS=300

HEALTHCHECK --interval=30s --timeout=10s --start-period=120s \
    CMD curl -sf http://localhost:8023/health || exit 1

# `-m parakeet --backend parakeet --auto-download` resolves to the
# parakeet-tdt-0.6b-v3 entry in src/crispasr_model_registry.cpp; the
# registry's default URL is cstr/parakeet-tdt-0.6b-v3-GGUF/...-q4_k.gguf,
# and --model-quant rewrites the filename suffix in place. First boot
# downloads ~470 MB (q4_k) into HF_HOME; subsequent boots hit the cache.
ENTRYPOINT ["/bin/sh", "-c", "exec /usr/local/bin/crispasr \
    --server \
    -m parakeet \
    --backend parakeet \
    --model-quant \"${PARAKEET_MODEL_QUANT}\" \
    --auto-download \
    --host 0.0.0.0 --port 8023 \
    -t \"$(nproc)\" \
    ${PARAKEET_EXTRA_ARGS:-}"]
