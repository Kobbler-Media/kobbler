# stt-parakeet-cpp

Parakeet-TDT speech-to-text on ggml, built from
[hbd-CrispASR](https://github.com/dbrain/hbd-CrispASR), a fork of
[CrispASR](https://github.com/CrispStrobe/CrispASR) that adds the lazy-load,
idle-unload and worker-isolation server behaviour. Clones `PARAKEET_REPO` at
`PARAKEET_REF`, builds the `crispasr` CLI with CUDA + flash-attn + multi-arch
PTX, and ships a ~700 MB runtime image with the binary, its shared libs, ffmpeg
and libsndfile.

Run it with the GPU overlay:

    docker compose -f docker-compose.yml -f docker-compose.gpu.yml up -d stt-parakeet-cpp

Kobbler reaches it at `http://stt-parakeet-cpp:8023` — the `stt_service_url`
default in `config/kobbler.toml.example`.

## Models

The image downloads its own GGUF on first request via `--auto-download`,
resolving `parakeet-tdt-0.6b-v3` against the model registry and caching it in
the `parakeet-cpp-models` volume (`HF_HOME`). Nothing to stage by hand.

First boot per quant pulls ~470 MB (q4_k), ~711 MB (q8_0) or ~1.26 GB (f16).
Quant IDs are matched case-sensitively against the HF filenames, so always pass
`PARAKEET_MODEL_QUANT` in lowercase: `q4_k` / `q5_0` / `q8_0` / `f16`.

## Env

| var | default | what |
|---|---|---|
| `PARAKEET_MODEL_QUANT` | `q8_0` (compose) | which GGUF quant to fetch |
| `PARAKEET_LAZY_LOAD` | 1 | defer the model load until the first request |
| `PARAKEET_IDLE_UNLOAD_SECONDS` | 300 | free the GPU after 5 min idle |
| `PARAKEET_WORKER_ISOLATION` | 1 | run the model in a subprocess so `/unload` reclaims all VRAM |
| `PARAKEET_EXTRA_ARGS` | — | appended to the server command line |

## Build args

| arg | default | what |
|---|---|---|
| `PARAKEET_REPO` | `https://github.com/dbrain/hbd-CrispASR.git` | source repo |
| `PARAKEET_REF` | `457252dd` | commit to build |
| `PARAKEET_CUDA_ARCHS` | `86;89;120` | trim to your card; each omitted arch saves ~15-20 MiB of idle VRAM |
| `PARAKEET_FORCE_MMQ` | `OFF` | force the INT8 MMA path for quantized matmuls |
