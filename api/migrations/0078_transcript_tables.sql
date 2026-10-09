-- Audiobook transcript segments (from whisper STT)
CREATE TABLE audiobook_transcripts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    audiobook_file_id UUID NOT NULL REFERENCES audiobook_files(id) ON DELETE CASCADE,
    segment_index INTEGER NOT NULL,
    start_seconds DOUBLE PRECISION NOT NULL,
    end_seconds DOUBLE PRECISION NOT NULL,
    text TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (audiobook_file_id, segment_index)
);
CREATE INDEX idx_audiobook_transcripts_file ON audiobook_transcripts(audiobook_file_id);
CREATE INDEX idx_audiobook_transcripts_time ON audiobook_transcripts(audiobook_file_id, start_seconds);

-- Ebook text segments (extracted from EPUB)
CREATE TABLE ebook_segments (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    book_file_id UUID NOT NULL REFERENCES book_files(id) ON DELETE CASCADE,
    segment_index INTEGER NOT NULL,
    cfi TEXT NOT NULL,
    text TEXT NOT NULL,
    chapter_title TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (book_file_id, segment_index)
);
CREATE INDEX idx_ebook_segments_file ON ebook_segments(book_file_id);

-- Cross-format alignment mapping (audiobook timestamp <-> ebook CFI)
CREATE TABLE cross_format_sync (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    book_id UUID NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    audio_file_id UUID NOT NULL REFERENCES audiobook_files(id) ON DELETE CASCADE,
    audio_seconds DOUBLE PRECISION NOT NULL,
    ebook_file_id UUID NOT NULL REFERENCES book_files(id) ON DELETE CASCADE,
    ebook_cfi TEXT NOT NULL,
    confidence DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_cross_format_sync_book ON cross_format_sync(book_id);
CREATE INDEX idx_cross_format_sync_audio ON cross_format_sync(audio_file_id, audio_seconds);
