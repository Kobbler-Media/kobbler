-- Qwen3 TTS preset fields. All optional (NULL = use service default).
-- ref_text:        ICL transcript of the reference clip (qwen3-only).
-- instructions:    voice-design description ("warm friendly young woman …"),
--                  used with voice="default" instead of a clip (qwen3-only).
-- language:        BCP-47-ish code, e.g. "en". Default applied at synth time.
-- seed:            deterministic-sample seed; chosen via the audition flow.
-- top_p/top_k/repetition_penalty/max_audio_tokens:
--                  sampler knobs (qwen3-only); chatterbox ignores them.
ALTER TABLE user_voice_presets
    ADD COLUMN ref_text TEXT,
    ADD COLUMN instructions TEXT,
    ADD COLUMN language TEXT,
    ADD COLUMN seed BIGINT,
    ADD COLUMN top_p DOUBLE PRECISION,
    ADD COLUMN top_k INTEGER,
    ADD COLUMN repetition_penalty DOUBLE PRECISION,
    ADD COLUMN max_audio_tokens INTEGER;
