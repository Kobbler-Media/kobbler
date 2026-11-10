# TTS: the `tts-breeze` service

Kobbler's text-to-speech engine and the HTTP contract its clients (kobbler,
koblem, kobapp) speak.

Service: `tts-breeze`, host port **8803**, container port 8203.
Engine: BreezeTTS 2 (T5Gemma2 text encoder → Qwen3 backbone → 16-codebook depth
decoder → Qwen3-TTS 12.5 Hz audio tokenizer), 24 kHz mono.

Building the image and converting the model: see
[docker/tts-breeze/README.md](../docker/tts-breeze/README.md). The Breeze-TTS-2
weights are research / non-commercial licensed.

---

## 1. Things that will bite you

**1. Leave `long` on.** `long` defaults to `true`, which chunks the text
internally so nothing truncates. With `long: false`:

| input | what happens |
|---|---|
| ≤ ~164 words | full coverage |
| beyond that | **silently truncated** at `max_new_frames` (default 750 = 60 s) or the model's own stop, whichever comes first |
| `max_new_frames` raised until prompt tokens + frames exceed `n_ctx` | **hard error**, `backbone: context overflow` |

Raising the cap does not buy longer output — it turns a silent truncation into
an error.

**2. Without a voice, every request is a different speaker.** `S0` samples a
fresh one each time. Within one `long` request the voice carries across chunks;
across requests it does not. Pass `voice` if more than one request makes up a
passage.

**3. `speed` renders server-side.** The audio comes back at that rate and the
alignment timings already describe the rendered audio. Never re-apply the rate
in the player. See [`speed`](#speed).

---

## 2. `POST /v1/audio/speech`

```jsonc
{
  "model": "breeze-tts-2",
  "input": "text to speak",          // required
  "voice": "narrator",               // optional; a saved voice id
  "response_format": "wav",          // wav | pcm | mp3
  "bitrate_kbps": 64,                // mp3 only
  "seed": 11,
  "instructions": "a gruff sea captain, slow and gravelly",
  "speaker": "S0",                   // S0..S9
  "speed": 1.5,                      // 0.25-4.0; see below
  "temperature": 0.9, "top_k": 50, "top_p": 1.0, "repetition_penalty": 1.1,
  "max_new_frames": 750              // 12.5 Hz FRAMES, not tokens
}
```

`instruction`, `instructions` and `voice_description` are all accepted.
`max_new_tokens` / `max_audio_tokens` are accepted as aliases for
`max_new_frames`, but the unit is still **frames at 12.5 Hz** — 2048 means 2048
frames and will overflow `n_ctx`.

### Streaming

Two `stream_format` values, both OpenAI's. Anything else is a **400**.

| `stream_format` | body | alignment | use |
|---|---|---|---|
| `"sse"` | `speech.audio.delta` events, base64 PCM s16le 24 kHz | **yes** | read-along |
| `"audio"` | raw chunks: `pcm` bytes, or `wav` with a placeholder-length header | no | plain playback |
| omitted | one buffered response | no | short clips |

```jsonc
{ "stream_format": "sse", "response_format": "pcm",
  "stream_first_batch_size": 1 }     // or chunk_frames; 1 = lowest time-to-first-audio
```
SSE events: `speech.audio.delta`, `speech.audio.done`, and when alignment is on
`speech.audio.alignment.partial` / `.final` / `.error`.
`stream_batch_size` and `chunk_frames` are accepted for the same knob (default 6,
roughly 480 ms of audio).

`stream_format: "audio"` accepts `response_format` of `pcm` or `wav`; **`mp3`
returns 400** (request mp3 without streaming). `stream: true`, or an explicit
`stream_batch_size` with no `stream_format`, also means `"audio"`.

**Use `pcm`, not `wav`, on the audio transport.** A streaming WAV header cannot
know its length, so it carries the `0xFFFFFFFF` placeholder. ffmpeg decodes it
fine, but a client that trusts the declared length sees a ~25-hour file with a
broken seek bar.

**Truncation on the audio transport is only a trailer** (`Trailer: X-Truncated`),
because it is not known until generation ends. Browser `fetch()` cannot read
trailers. If you need a reliable truncation signal, use `"sse"`.

**Always stream.** Peak VRAM does not grow with output length when streaming;
buffered, it does.

### Long-form

```jsonc
{ "long": true, "chunk_words": 120, "long_ref_frames": 600 }
```
On by default. Chunks the text and feeds each chunk's generated codes back in as
the next chunk's reference, so the voice carries without a saved voice. Works
buffered and streamed; streams per codec block, not per chunk.

- `chunk_words` — default 120, or 64 when aligning (the aligner is only accurate
  on short audio).
- `long_ref_frames` — reference carried between chunks, default and max 600
  frames. Prompt tokens, reference frames and generated frames all share `n_ctx`.
- `buffer` — accepted for compatibility; caps the reference at `buffer × 250`
  frames.

### Forced alignment

Only on the SSE path. A non-SSE request with `align: true` returns **400**.

```jsonc
{ "stream_format": "sse", "response_format": "pcm",
  "align": true, "align_stream": "partial",   // or "final-only" (default)
  "align_device": "gpu" }                      // "gpu" (default) | "cpu" | "off"
```
Word events: `{ text, word_index, t0_ms, t1_ms, confidence }`.

**A `.partial` carries only the resolved prefix, not the whole passage.** Its
`words` array grows as the voice advances and the `.final` carries all of them.
`word_index` is absolute and stable in both: **overwrite by `word_index`, never
append.** The prefix never shrinks between partials, but a word's timings are
re-estimated each pass and do change, which overwriting absorbs.

Median onset error is under 30 ms on prose; isolated word lists are less
accurate. `align_device: "cpu"` gives the same result with no VRAM at roughly
4× the wall clock.

Aligner tuning env vars: `QWEN3_FA_PARTIAL_TRIM=0` sends every word on every
partial; `QWEN3_FA_PARTIAL_MIN_CONF` (default `0.05`) is the confidence floor
below which a trailing run is treated as not yet spoken.

### Truncation

With `long` on, nothing truncates. With `long: false`, generation stops at
`max_new_frames` whether or not the text finished, and the result is a valid,
short, mid-sentence clip. It is signalled:

| path | signal |
|---|---|
| SSE | `"truncated": true` on `speech.audio.done` |
| buffered | `X-Truncated: 1` response header |
| `stream_format: "audio"` | `X-Truncated` **trailer** — see above |
| server log | one `breeze: TRUNCATED at max_new_frames=...` line |

Treat a truncated response as a failure and re-request with `long: true`.

### `speed`

Server-side, range **0.25–4.0** (OpenAI's); anything outside is a **400**.
Duration changes, pitch does not (WSOLA time-stretch). Works on all three
response paths. `speed: 1.0` bypasses the stretcher. Quality holds over roughly
0.5–2.0; beyond that the stretch becomes audible.

**Alignment timestamps are already rescaled**: `t0_ms`, `t1_ms`, `audio_seen_ms`
and `audio_total_ms` describe the audio you receive. Do not divide by `speed`.
If you change rate in the player instead (`playbackRate` + `preservesPitch`),
send `speed: 1.0` so it is not applied twice.

Client rules, which kobbler and koblem follow:

| step | rule |
|---|---|
| request | omit `speed` at natural rate; clamp into 0.25–4.0 |
| on 400 | retry once with `speed` dropped — a rejected rate must still produce audio |
| response | report what was rendered, not what was requested |

koblem reports the outcome in the **`x-speed-applied`** response header; kobbler
puts the same value on `paragraph_start.speed` (and `sentence.speed` on the
per-sentence path) of its NDJSON stream.

### `POST /v1/audio/alignment`

Aligns supplied audio against known text, no synthesis.
Multipart: `file` (wav) + `text`. Returns the same word array.

---

## 3. Voices

| route | purpose |
|---|---|
| `GET /v1/audio/voices` | list |
| `POST /v1/audio/voices` | create — multipart `name` + `audio_sample` + `ref_text` |
| `DELETE /v1/audio/voices/{id}` | remove |
| `GET /v1/audio/voices/{id}/sample.wav` | the reference clip |
| `GET /v1/audio/voices/{id}/ref_text` | the reference transcript |

The list body carries two shapes listing the same ids: a flat
`{"<model_id>": ["default", ...]}` and a `"voices"` array of
`{id, builtin, frames, codebooks, has_ref_text, has_sample}`. `default`
(`builtin: true`) means "no reference voice", i.e. the model's own narrator.

A voice is reference codes + transcript. Passing `voice` re-applies it per
request, which is what holds identity across chunks.

---

## 4. Other routes

`GET /health`, `GET /v1/models`, `GET /v1/gpu/status`,
`POST /v1/admin/{load,unload}`, `GET /v1/audio/languages`.

Breeze is English/Chinese only and infers the language from the text.
`/v1/audio/languages` returns `en` and `zh` with **`id: null`**; there is no
`language` request field.

Vocal events are plain text. The model card documents four per language:
English `(laugh)` `(cough)` `(clears throat)` `(sigh)`; Chinese `[笑]` `[咳嗽]`
`[清嗓子]` `[叹气]`. Others may work; nothing guarantees it.

---

## 5. Deployment

| env | default | notes |
|---|---|---|
| `BREEZE_MODEL` | `breeze-tts-2-v2-Q4_K_M.gguf` | file under `/models` |
| `BREEZE_FA_MODEL` | `/models/qwen3-forced-aligner-0.6b-q4_k.gguf` | **must be the q4_k file**; `""` disables alignment |
| `BREEZE_N_CTX` | 2048 | shared by prompt tokens + frames |
| `BREEZE_TEXT_ENC_CPU` | 1 | keeps the 926 MiB text encoder in host RAM; ~2 % slower, bit-identical audio |
| `BREEZE_IDLE_UNLOAD_SECONDS` | 300 | idle worker is killed so VRAM returns to 0 |

**VRAM.** With the defaults (Q4_K_M, text encoder on CPU) a streamed render
peaks around 2.3 GB regardless of length; add ~0.6 GB for the aligner. Buffered
long renders grow past 4 GB. A 4 GB NVIDIA card is enough if you stream.
