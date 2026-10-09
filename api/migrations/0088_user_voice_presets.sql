CREATE TABLE user_voice_presets (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    voice TEXT,
    clip_filename TEXT,
    speed DOUBLE PRECISION NOT NULL DEFAULT 1.0,
    exaggeration DOUBLE PRECISION,
    cfg_weight DOUBLE PRECISION,
    temperature DOUBLE PRECISION,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(user_id, name)
);

ALTER TABLE users ADD COLUMN active_voice_preset_id UUID REFERENCES user_voice_presets(id) ON DELETE SET NULL;
