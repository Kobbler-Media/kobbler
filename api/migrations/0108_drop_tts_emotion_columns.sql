-- Chatterbox Turbo ignores exaggeration and cfg_weight params, so remove them.
ALTER TABLE user_voice_presets DROP COLUMN IF EXISTS exaggeration;
ALTER TABLE user_voice_presets DROP COLUMN IF EXISTS cfg_weight;
