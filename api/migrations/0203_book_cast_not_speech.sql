-- A quote pass 1 judged nobody says aloud (a sign, a note, scare quotes) is
-- kept so it can be vetted, and narrated in place as written.
ALTER TABLE book_cast_lines
    ADD COLUMN not_speech BOOLEAN NOT NULL DEFAULT FALSE,
    ADD CONSTRAINT book_cast_lines_not_speech_unspoken CHECK (NOT not_speech OR character_id IS NULL);
