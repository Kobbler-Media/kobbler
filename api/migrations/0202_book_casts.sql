CREATE TABLE book_casts (
    book_id UUID PRIMARY KEY REFERENCES books(id) ON DELETE CASCADE,
    book_file_id UUID NOT NULL REFERENCES book_files(id) ON DELETE CASCADE,
    owner_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    state TEXT NOT NULL CHECK (state IN ('chapters', 'bible', 'voices', 'casting', 'rendering', 'done', 'failed')),
    error TEXT,
    audiobook_file_id UUID REFERENCES audiobook_files(id) ON DELETE SET NULL,
    analysis_started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE book_cast_chapters (
    book_id UUID NOT NULL REFERENCES book_casts(book_id) ON DELETE CASCADE,
    chapter_index INTEGER NOT NULL,
    title TEXT,
    first_segment INTEGER NOT NULL,
    last_segment INTEGER NOT NULL,
    notes JSONB NOT NULL,
    model TEXT NOT NULL,
    finished_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (book_id, chapter_index)
);

CREATE TABLE book_cast_characters (
    id UUID PRIMARY KEY,
    book_id UUID NOT NULL REFERENCES book_casts(book_id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    aliases TEXT[] NOT NULL DEFAULT '{}',
    bible JSONB NOT NULL DEFAULT '{}',
    line_count INTEGER NOT NULL DEFAULT 0,
    pooled BOOLEAN NOT NULL DEFAULT FALSE,
    generic BOOLEAN NOT NULL DEFAULT FALSE,
    narrator BOOLEAN NOT NULL DEFAULT FALSE,
    -- What the admin changed by hand ({"generic": .., "bible": {..}}), laid
    -- back over the analysis when it is run again.
    edits JSONB NOT NULL DEFAULT '{}',
    UNIQUE (book_id, name)
);
CREATE UNIQUE INDEX idx_book_cast_characters_narrator ON book_cast_characters(book_id) WHERE narrator;

-- `speaker` is the name the chapter pass gave; `character_id` is set once the
-- bible pass has resolved that name to a character. Both NULL = narration.
-- A generic character's lines (an unnamed soldier, a crowd) are narration too.
CREATE TABLE book_cast_lines (
    book_id UUID NOT NULL REFERENCES book_casts(book_id) ON DELETE CASCADE,
    segment_index INTEGER NOT NULL,
    span_start INTEGER NOT NULL,
    span_end INTEGER NOT NULL,
    chapter_index INTEGER NOT NULL,
    speaker TEXT,
    character_id UUID REFERENCES book_cast_characters(id) ON DELETE SET NULL,
    cue TEXT,
    source TEXT NOT NULL CHECK (source IN ('gemma', 'beefy', 'user')),
    PRIMARY KEY (book_id, segment_index, span_start)
);
CREATE INDEX idx_book_cast_lines_character ON book_cast_lines(character_id);

CREATE TABLE book_cast_voices (
    character_id UUID NOT NULL REFERENCES book_cast_characters(id) ON DELETE CASCADE,
    from_chapter INTEGER NOT NULL DEFAULT 0,
    preset_id UUID REFERENCES user_voice_presets(id) ON DELETE SET NULL,
    design_preset_id UUID REFERENCES user_voice_presets(id) ON DELETE SET NULL,
    approved_at TIMESTAMPTZ,
    PRIMARY KEY (character_id, from_chapter)
);

-- A chapter's audio is a file under the storage root; a row without its file
-- (a restored backup) is rendered again, as is one whose fingerprint (the
-- voices and attribution it was read with) no longer matches the cast.
CREATE TABLE book_cast_renders (
    book_id UUID NOT NULL REFERENCES book_casts(book_id) ON DELETE CASCADE,
    chapter_index INTEGER NOT NULL,
    duration_ms BIGINT NOT NULL,
    fingerprint JSONB NOT NULL,
    render_ms BIGINT NOT NULL,
    rendered_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (book_id, chapter_index)
);

-- A preset seeded for a cast's voice design belongs to that cast: it is kept
-- off the owner's own voice list and goes when the cast is started over.
ALTER TABLE user_voice_presets
    ADD COLUMN cast_book_id UUID REFERENCES book_casts(book_id) ON DELETE CASCADE;
CREATE INDEX idx_user_voice_presets_cast_book ON user_voice_presets(cast_book_id)
    WHERE cast_book_id IS NOT NULL;
