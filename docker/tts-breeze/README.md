# tts-breeze

`breeze-server` — BreezeTTS 2 on ggml, the `breeze-server` CMake target of
[hbd-qwen3-tts.cpp](https://github.com/dbrain/hbd-qwen3-tts.cpp). The build
clones `BREEZE_REPO` at `BREEZE_REF`; override both in `.env` to build another
tree.

Breeze-TTS-2 is research / non-commercial licensed (RESONIA, INC.). Check the
licence before using it for anything but personal evaluation.

Run it with the GPU overlay:

    docker compose -f docker-compose.yml -f docker-compose.gpu.yml up -d tts-breeze

## Weights

One GGUF holds all four sub-models (text encoder, backbone, depth decoder, audio
tokenizer). Nothing is baked into the image — the file is bind-mounted from
`BREEZE_MODELS_DIR` at `/models`.

No GGUF is published, so convert
[BreezeBlue/Breeze-TTS-2](https://huggingface.co/BreezeBlue/Breeze-TTS-2) yourself
from a checkout of `BREEZE_REPO` built with CMake (the converter needs `numpy`,
`safetensors`, `tqdm` and `gguf`). gguf-py cannot write K-quants, so the default
`Q4_K_M` file takes two steps:

    python scripts/convert_breeze_to_gguf.py \
        --input  <HF checkpoint dir> \
        --output /models/breeze-tts-2-v2-f16.gguf \
        --type f16

    build/gguf_quantize /models/breeze-tts-2-v2-f16.gguf \
        /models/breeze-tts-2-v2-Q4_K_M.gguf Q4_K_M

The HF checkpoint directory must include `audio_tokenizer/model.safetensors` —
a separate ~682 MB file the top-level download misses. The converter fails
loudly without it. `hf download BreezeBlue/Breeze-TTS-2 --local-dir <dir>` fetches
everything. Put the result in `BREEZE_MODELS_DIR`, or stop after the first step
and set `BREEZE_MODEL` to the f16 (or `--type q8_0`) file.

Read-along alignment also wants `qwen3-forced-aligner-0.6b-q4_k.gguf` in the same
directory. It is loaded on the first aligned request; without it, set
`BREEZE_FA_MODEL=` to turn alignment off.

Fewer bits is not automatically faster: Q6_K and Q3_K depth both benchmark
slower than Q8_0 despite being smaller.

## Env

| var | default | what |
|---|---|---|
| `BREEZE_MODEL` | `breeze-tts-2-v2-Q4_K_M.gguf` | file under the bind-mounted `/models` |
| `BREEZE_N_CTX` | 2048 | backbone context in tokens+frames |
| `BREEZE_TEXT_ENC_CPU` | 1 | keep the 926 MiB text encoder in host RAM (~1.7% slower, bit-identical audio) |
| `BREEZE_WORKER_ISOLATION` | 1 | fork the GPU child; 0 = in-process |
| `BREEZE_IDLE_UNLOAD_SECONDS` | 300 | SIGKILL the idle worker so VRAM reclaims to 0 |
| `BREEZE_FA_MODEL` | the 0.6b aligner | `""` disables read-along alignment |
| `BREEZE_VOICES_DIR` | `/app/voices` | the runtime voice archive |
| `WORKER_DEFAULT_GPU` | — | GPU UUID for un-targeted requests |

## Request shape

OpenAI-compatible `/v1/audio/speech` (wav | pcm | SSE with interleaved
`speech.audio.alignment.*` events) plus `/v1/audio/voices` CRUD with multipart
cloning. Breeze-specific extras on `/v1/audio/speech`:

- `instruction` (or `voice_description`) — natural-language voice design.
  Empty means the plain template.
- `speaker` — `S0`..`S9`, the checkpoint's speaker tokens. Default `S0`.
- `chunk_frames` — codec frames before the first SSE delta. 6 is roughly 480 ms
  of audio and is what sets time-to-first-audio; the chunk then doubles up to 48
  so a long paragraph does not re-decode the whole prefix on every emit.
- `max_new_frames` — 12.5 Hz frames, not tokens. `max_new_tokens` /
  `max_audio_tokens` are accepted as aliases.
