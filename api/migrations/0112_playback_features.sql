-- Rich playback features: seek thumbnails, video chapters, intro/credits skip,
-- volume normalization (LUFS), audiobook bookmarks, audiobook intro/outro.

-- Seek thumbnails: track generation status per video file
ALTER TABLE movie_files ADD COLUMN thumbnails_generated BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE tv_episode_files ADD COLUMN thumbnails_generated BOOLEAN NOT NULL DEFAULT FALSE;

-- Video chapters (polymorphic, same pattern as audio_tracks/subtitle_tracks)
CREATE TABLE video_chapters (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    movie_file_id UUID REFERENCES movie_files(id) ON DELETE CASCADE,
    episode_file_id UUID REFERENCES tv_episode_files(id) ON DELETE CASCADE,
    chapter_index INTEGER NOT NULL,
    title TEXT,
    start_seconds DOUBLE PRECISION NOT NULL,
    end_seconds DOUBLE PRECISION NOT NULL,
    CONSTRAINT video_chapters_one_parent CHECK (
        (movie_file_id IS NOT NULL) != (episode_file_id IS NOT NULL)
    )
);
CREATE UNIQUE INDEX idx_video_chapters_movie ON video_chapters(movie_file_id, chapter_index)
    WHERE movie_file_id IS NOT NULL;
CREATE UNIQUE INDEX idx_video_chapters_episode ON video_chapters(episode_file_id, chapter_index)
    WHERE episode_file_id IS NOT NULL;

-- TV intro/credits skip segments (episode-only)
CREATE TABLE episode_skip_segments (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    episode_file_id UUID NOT NULL REFERENCES tv_episode_files(id) ON DELETE CASCADE,
    segment_type TEXT NOT NULL CHECK (segment_type IN ('intro', 'credits')),
    start_seconds DOUBLE PRECISION NOT NULL,
    end_seconds DOUBLE PRECISION NOT NULL,
    source TEXT NOT NULL DEFAULT 'chapter',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE UNIQUE INDEX idx_skip_segments_file_type ON episode_skip_segments(episode_file_id, segment_type);

-- LUFS loudness measurement for volume normalization (video + audio)
ALTER TABLE movie_files ADD COLUMN lufs DOUBLE PRECISION;
ALTER TABLE tv_episode_files ADD COLUMN lufs DOUBLE PRECISION;
ALTER TABLE audiobook_files ADD COLUMN lufs DOUBLE PRECISION;

-- Audiobook intro/outro detection boundaries
ALTER TABLE audiobook_files ADD COLUMN intro_end_seconds DOUBLE PRECISION;
ALTER TABLE audiobook_files ADD COLUMN outro_start_seconds DOUBLE PRECISION;
ALTER TABLE audiobook_files ADD COLUMN intro_outro_source TEXT;

-- Audiobook user bookmarks
CREATE TABLE audiobook_bookmarks (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_id UUID NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    audiobook_file_id UUID REFERENCES audiobook_files(id) ON DELETE SET NULL,
    position_seconds DOUBLE PRECISION NOT NULL,
    note TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_audiobook_bookmarks_user_book ON audiobook_bookmarks(user_id, book_id);
