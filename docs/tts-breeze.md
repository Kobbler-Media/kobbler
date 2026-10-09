# TTS: the `tts-breeze` service

The single TTS engine. Replaces `tts-qwen3` and `higgs-tts`; this document is the
contract for every client (kobbler, koblem, kobapp).

Service: `tts-breeze`, host port **8803**, container port 8203.
Engine: BreezeTTS 2 (T5Gemma2 text encoder → Qwen3 backbone → 16-codebook depth
decoder → Qwen3-TTS 12.5 Hz audio tokenizer), 24 kHz mono.

---

## 1. The three things that will bite you

**1. Text must be chunked, or use `long: true`.** Three regimes, all measured:

| input | what happens |
|---|---|
| ≤ ~164 words | full coverage |
| beyond that | **silently truncated** at `max_new_frames` (default 750 = 60 s) or the model's own stop around 890–1100 frames, whichever comes first. 792 words returned 60.0 s — about 21 % of the text, no error. |
| raise `max_new_frames` far enough that prompt tokens + frames exceed `n_ctx` | **hard error**, `backbone: context overflow`. 792 words (891 prompt tokens) with `max_new_frames: 4000` fails outright. |

So raising the cap does not buy you longer output — it converts a silent
truncation into a crash. **Use `long: true`**, which chunks internally and keeps
every chunk's prompt small.

**2. Without a voice, every request is a different speaker.** `[S0]` samples a
fresh one each time. Median F0 across three consecutive chunks: **114 Hz spread**
with no voice, **5 Hz** with a saved voice. So either pass `voice`, or use
`long: true` (below), or accept a narrator who changes sex mid-paragraph.

**3. `speed` renders server-side, and a rate the engine can't do is a 400.**
Not a no-op knob and not a player setting: the audio comes back at the rate, and
the aligner ran against that audio, so timings are already in the rendered
timebase. Two rules follow. Never re-apply the rate in the player — that would
double it. And never send `speed` unconditionally: `1.0` is accepted, but the
field is omitted at natural rate so a future validation change can't 400 every
render. A rejected rate must degrade to a 1.0 re-render, not to an error — see
[§2 `speed`](#speed).

---

## 2. `POST /v1/audio/speech`

```jsonc
{
  "model": "breeze-tts-2",
  "input": "text to speak",          // required
  "voice": "gaiman",                 // optional; a saved voice id
  "response_format": "wav",          // wav | pcm | mp3
  "bitrate_kbps": 64,                // mp3 only
  "seed": 11,
  "instructions": "a gruff sea captain, slow and gravelly",
  "speaker": "S0",                   // S0..S9
  "speed": 1.5,                      // real re-render at that rate; see below
  "temperature": 0.9, "top_k": 50, "top_p": 1.0, "repetition_penalty": 1.1,
  "max_new_frames": 750              // 12.5 Hz FRAMES, not tokens (see §1)
}
```

`instruction`, `instructions` and `voice_description` are all accepted.
`max_new_tokens` / `max_audio_tokens` are accepted as aliases for
`max_new_frames` — **but the unit is frames at 12.5 Hz, not qwen3's audio
tokens.** qwen3's `max_audio_tokens: 2048` meant ~164 s; the same number here
means 2048 frames and will overflow `n_ctx`.

### Streaming

Two `stream_format` values, both OpenAI's. Anything else is a **400**.

| `stream_format` | body | alignment | use |
|---|---|---|---|
| `"sse"` | `speech.audio.delta` events, base64 PCM s16le 24 kHz | **yes** | read-along |
| `"audio"` | raw chunks: `pcm` bytes, or `wav` with a placeholder-length header | no | plain playback |
| omitted | one buffered response | no | short clips |

```jsonc
{ "stream_format": "sse", "response_format": "pcm",
  "stream_first_batch_size": 1 }     // or chunk_frames; 1 = lowest TTFA
```
SSE events: `speech.audio.delta`, `speech.audio.done`, and when alignment is on
`speech.audio.alignment.partial` / `.final` / `.error`.
`stream_batch_size` and `chunk_frames` are accepted for the same knob.
TTFA: 203 ms at the default, **89 ms** at `stream_first_batch_size: 1`.

`stream_format: "audio"` accepts `response_format` of `pcm` or `wav`; **`mp3`
returns 400** (it needs a streaming encoder the server does not have — request
mp3 without streaming). `stream: true` is an older spelling of `"audio"`, and an
explicit `stream_batch_size` with no `stream_format` also means `"audio"`, both
matching `tts-qwen3`.

**Use `pcm`, not `wav`, on the audio transport.** A streaming WAV header cannot
know its length, so it carries the conventional `0xFFFFFFFF` placeholder. ffmpeg
reads to EOF and decodes it correctly, but any client that *trusts* the declared
length sees 2³¹ frames — Python's `wave` reports exactly that — which presents as
a ~25-hour file with a broken seek bar.

**Truncation on this transport is only a trailer.** There is no
`speech.audio.done` here and `X-Truncated` is a buffered-path header, so the flag
ships as a trailing header (`Trailer: X-Truncated`) because truncation is not
known until generation ends. Trailers are correct HTTP but thinly supported —
browser `fetch()` cannot read them. **If you need a reliable truncation signal,
use `"sse"`.**

**Always stream.** Peak VRAM does not grow with output length when streaming;
buffered it does (2243 MiB vs 4145 MiB on a 4.5-minute read).

koblem's `resolve_transport` therefore defaults `audio` to `pcm`; ask for `wav` only if you know your player ignores the declared length.

### Long-form — the audiobook path

```jsonc
{ "long": true, "chunk_words": 120, "buffer": 1 }
```
Chunks the text and feeds each chunk's **generated codes back in as the next
chunk's reference**, so the voice carries without a saved voice. Works buffered
and streamed; streams per codec block, not per chunk, so TTFA stays low.

| 52 s in one request | F0 drift | WER |
|---|---|---|
| naive chunking, 3 requests | 48.4 Hz | 0.06 |
| `long: true` | **6.0 Hz** | **0.01** |

Buffered RTF 0.2498; streamed RTF 0.2514, TTFA 49 ms.
`buffer` is higgs's chunk-count knob and is accepted, but Breeze caps by frames
(`long_ref_frames`, default 250 = 20 s, max 600): prompt tokens, reference frames
and generated frames all share `n_ctx` (2048). The cap costs nothing — the same
text renders 282 frames with no reference and 307 with a 20 s one.

**Use `long: true` for anything over ~150 words.** It replaces client-side
chunking entirely.

### Forced alignment

Only on the SSE path. A buffered request with `align: true` returns **400**.

```jsonc
{ "stream_format": "sse", "response_format": "pcm",
  "align": true, "align_stream": "partial" }   // or "final-only"
```
Word events: `{ text, word_index, t0_ms, t1_ms, confidence }`.

**A `.partial` carries only the resolved prefix, not the whole passage.** The
aligner is handed the full word list every pass and will happily return a timing
for every word, so the not-yet-spoken tail comes back as a synthetic ~40 ms/word
ramp at confidence ~0.02. The server trims that off, so a partial's `words` array
grows as the voice advances (6 words at 0.5 s, 49 at 7 s, 468 at 147 s) and the
`.final` carries all of them. `word_index` is absolute and stable in both, so the
client rule is unchanged: **overwrite by `word_index`, never append.** The prefix
is monotonic -- it never shrinks between partials -- but a word's *timings* are
re-estimated each pass and do change, which is exactly what overwriting absorbs.

Accuracy vs torchaudio MMS_FA over 690 prose words: **median absolute onset error
27 ms, 76 % within 50 ms, 99.9 % within one word.** Live partial path 28 ms.
Page/sentence boundaries land inside the real pause 100 % of the time.
Costs **+13 % RTF** and **634 MiB** on top of the TTS model.
Caveat: isolated word *lists* (not prose) are worse; `QWEN3_FA_AUDIO_REFINE=6`
restores the old behaviour for that material.
`QWEN3_FA_PARTIAL_TRIM=0` disables the prefix trim (restores "every word on every
partial"); `QWEN3_FA_PARTIAL_MIN_CONF` (default `0.05`) is the floor below which a
trailing run is treated as ramp.

### Truncation

With `long` on (the default) text is chunked and nothing truncates. With
`long: false`, generation stops at `max_new_frames` (default 750 = **60.00 s**)
whether or not the text finished, and the result is a valid, short, mid-sentence
clip. That is now signalled, not silent:

| path | signal |
|---|---|
| SSE | `"truncated": true` on `speech.audio.done` |
| buffered | `X-Truncated: 1` response header |
| `stream_format: "audio"` | `X-Truncated` **trailer** — see the streaming caveat |
| server log | one `breeze: TRUNCATED at max_new_frames=...` line |

Treat a truncated response as a failure and re-request with `long: true`.

### `speed`

Implemented server-side, range **0.25-4.0** (OpenAI's). Duration changes, pitch
does not -- the audio is time-stretched with WSOLA, not resampled. Works on all
three response paths, streamed included.

**Alignment timestamps are already rescaled**: `t0_ms`, `t1_ms`, `audio_seen_ms`
and `audio_total_ms` all describe the audio you receive, so a read-along needs no
correction. Do not divide by `speed` yourself.

`speed: 1.0` bypasses the stretcher entirely, so the default path is unaffected.

Measured on the 5060 Ti, 250 words of prose, fixed seed, scored against torchaudio
MMS_FA re-aligned on the **stretched** audio:

| speed | audio | ratio | onset err med | p90 | wall | peak VRAM |
|---|---|---|---|---|---|---|
| 0.75 | 116.85 s | 0.7500 | 36 ms | 110 ms | — | — |
| 1.00 | 87.64 s | 1.0000 | 29 ms | 94 ms | 5.98 s | 2859 MiB |
| 1.50 | 58.43 s | 1.5000 | 20 ms | 65 ms | 5.96 s | 2863 MiB |
| 2.00 | 43.82 s | 2.0000 | 25 ms | 59 ms | — | — |

Duration is exact and accuracy is unchanged, so the rescaling is correct.
Wall-clock is identical across speeds and VRAM differs by 4 MiB — the stretcher
is effectively free; RTF only moves because it divides by output duration.

**Re-dividing by `speed` client-side was measured at 9.7–19.4 s of median onset
error** instead of 20–36 ms. Doing it in the player instead (`playbackRate` +
`preservesPitch`) remains valid and is instant — but then the server must not do
it too, so send `speed: 1.0`.

Quality holds well over roughly 0.5–2.0; beyond that WSOLA becomes audible, which
is a property of time-stretching rather than a defect.

It was a flat 400 for any non-1.0 value at one point in the migration, so every
client here still treats a rejected rate as a fallback rather than a failure:

| step | rule |
|---|---|
| request | omit `speed` at natural rate; clamp into 0.25-4.0 rather than letting it 400 |
| on 400 | retry ONCE with `speed` dropped — a rejected rate must still produce audio |
| response | report what was RENDERED, never what was requested |

koblem owns that fallback for everything routed through it and reports the
outcome in the **`x-speed-applied`** response header; kobbler puts the same value
on `paragraph_start.speed` (and `sentence.speed` on the per-sentence path),
because a streamed NDJSON envelope has no header to hang it on. A UI that shows
"2x" over audio that is plainly 1x is the failure this exists to prevent.

### `POST /v1/audio/alignment`

Aligns supplied audio against known text, no synthesis.
Multipart: `file` (wav) + `text`. Returns the same word array.

---

## 3. Voices

| route | purpose |
|---|---|
| `GET /v1/audio/voices` | list — two shapes in one body, see below |
| `POST /v1/audio/voices` | create — multipart `name` + `audio_sample` + `ref_text` |
| `DELETE /v1/audio/voices/{id}` | remove |
| `GET /v1/audio/voices/{id}/sample.wav` | the reference clip |
| `GET /v1/audio/voices/{id}/ref_text` | the reference transcript |

`GET /v1/audio/voices` answers with both the flat qwen3-tts shape
(`{"<model_id>": ["default", ...]}`) and a richer `"voices"` array of
`{id, builtin, frames, codebooks, has_ref_text, has_sample}`. **The two list the
same ids.** `default` appears in both with `builtin: true` — it is a real
selectable value meaning "no reference voice", i.e. the model's own narrator.

The list body carries **both shapes**: qwen3's `{"<model_id>": ["default", ...]}`
and the richer `{"model_id", "voices":[{id, frames, codebooks, ...}]}`. A client
written against either works.

A voice is reference codes + transcript. Passing `voice` re-applies it per
request, which is what holds identity across chunks.

---

## 4. Other routes

`GET /health`, `GET /v1/models`, `GET /v1/gpu/status`,
`POST /v1/admin/{load,unload}`, `GET /v1/audio/languages`.

`/v1/audio/languages` returns `en` and `zh` with **`id: null`** — Breeze is
bilingual and infers language from the text; there is no numeric `language_id`
and no `language` request field. **This is the one real capability loss against
qwen3-tts**, which advertised 10 languages.

Vocal events are plain text, not tokens, so there is no closed list. The model
card documents four per language: English `(laugh)` `(cough)` `(clears throat)`
`(sigh)`; Chinese `[笑]` `[咳嗽]` `[清嗓子]` `[叹气]`. Others may work; nothing
guarantees it.

---

## 5. Deployment

| env | default | why |
|---|---|---|
| `BREEZE_MODEL` | `breeze-tts-2-v2-Q4_K_M.gguf` | RTF 0.245, 2947 MiB (5060 Ti) |
| `BREEZE_FA_MODEL` | `qwen3-forced-aligner-0.6b-q4_k.gguf` | **must be the q4_k file** |
| `BREEZE_N_CTX` | 2048 | shared by prompt tokens + frames |
| `BREEZE_TEXT_ENC_CPU` | unset | `=1` frees 926 MiB for +1.7 % RTF, bit-identical audio |
| `QWEN3_FA_CPU` | unset | `=1` frees 599 MiB but costs +95 % RTF; prefer the GPU aligner |

`qwen3-forced-aligner-0.6b-q8_0.gguf` is an **audio.cpp-format file, not ours** —
it now fails the load with a clear message instead of SIGSEGV'ing the worker.

**VRAM. Always stream.** Peak device memory, RTX 3060, Q4_K_M, sampled
externally for the life of the run:

| input | path | audio | RTF | peak |
|---|---|---|---|---|
| 132 words | buffered | 48.9 s | 0.349 | 4309 MiB |
| 132 words | **streamed** | 48.9 s | 0.356 | **3143 MiB** |
| 792 words | `long`, buffered | 268.8 s | 0.348 | 4145 MiB |
| 792 words | `long`, **streamed** | 268.8 s | 0.353 | **3161 MiB** |
| 792 words | `long`, streamed, `BREEZE_TEXT_ENC_CPU=1` | 290.5 s | 0.360 | **2243 MiB** |

Streaming is worth ~1 GB and costs ~2 % RTF; the text encoder on CPU is worth
another ~0.9 GB for another ~2 %. Together: **2243 MiB for four and a half
minutes of continuous audio**, and peak does not grow with length because the
streamed scratch is flat. Buffered scratch scales with output, which is what the
4145 MiB row is.

Add ~634 MiB for the GPU aligner if read-along alignment is on. That still fits
beside a 9 GB LLM on a 12 GB card.

**Recommendation: stream everything.** There is no case where buffering a long
render is the better choice.

---

## 6. Migration notes for existing clients

- `PARAGRAPH_CHAR_CAP = 2000` was sized for qwen3's ~164 s budget. Either drop it
  to ~900 **or** (preferred) stop chunking and send `long: true`.
- Any stored `max_audio_tokens` on a voice preset is now **frames** — a saved
  2048 will overflow `n_ctx`. Clamp or clear it.
- `speed` stays in the request body, but omit it at natural rate, handle the 400
  by re-rendering at 1.0, and stop applying rate in the player — the audio and
  the word timings both arrive already at speed.
- `language` has no effect; remove the field or restrict the UI to en/zh.
- Alignment `word_index` is per request. With `long: true` one request covers the
  whole passage, so the old per-paragraph `word_offset` globalising is no longer
  needed — and was a source of cascading desync when a paragraph truncated.
