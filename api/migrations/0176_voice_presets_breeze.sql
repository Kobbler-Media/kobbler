-- Move voice presets onto the single TTS engine (BreezeTTS 2).
--
-- `max_audio_tokens` was qwen3's cap in AUDIO TOKENS, where 2048 meant ~164 s.
-- The new engine accepts the same knob but reads it as 12.5 Hz FRAMES, so a
-- carried-over 2048 is not "a long render" — it overflows n_ctx and fails the
-- request outright. The column is renamed to say what it now means, and any
-- stored value is dropped rather than reinterpreted: there is no conversion
-- that makes an old number correct, and NULL simply defers to the engine.
ALTER TABLE user_voice_presets RENAME COLUMN max_audio_tokens TO max_new_frames;
UPDATE user_voice_presets SET max_new_frames = NULL WHERE max_new_frames IS NOT NULL;

-- The engine is en/zh and infers language from the text; there is no `language`
-- request field for a stored value to feed.
ALTER TABLE user_voice_presets DROP COLUMN language;

-- Built-in narrator token ("S0".."S9"). NULL = the engine's own default.
ALTER TABLE user_voice_presets ADD COLUMN speaker TEXT;
