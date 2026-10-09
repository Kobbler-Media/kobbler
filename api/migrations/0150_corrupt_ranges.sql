-- Per-segment transcode-on-demand: track time ranges where the SW decoder
-- hit error-concealment / missing references / invalid bitstream so the
-- streaming layer can spawn overlay transcode generations that paper over
-- corruption that would otherwise crash the browser HW decoder.
--
-- Format: array of {start: float, end: float, frame_count: int,
--                   concealment: bool, missing_reference: bool,
--                   invalid_bitstream: bool, decode_slices_failed: bool,
--                   other: bool}
ALTER TABLE tv_episode_files
    ADD COLUMN corrupt_ranges JSONB NOT NULL DEFAULT '[]'::jsonb;

ALTER TABLE movie_files
    ADD COLUMN corrupt_ranges JSONB NOT NULL DEFAULT '[]'::jsonb;
