-- Where the picture sits inside the frame when bars are baked into the encode, as
-- fractions of the frame. NULL area with a checked_at means no bars; NULL checked_at
-- means not yet looked at.
ALTER TABLE movie_files
    ADD COLUMN picture_x DOUBLE PRECISION,
    ADD COLUMN picture_y DOUBLE PRECISION,
    ADD COLUMN picture_width DOUBLE PRECISION,
    ADD COLUMN picture_height DOUBLE PRECISION,
    ADD COLUMN picture_checked_at TIMESTAMPTZ;

ALTER TABLE tv_episode_files
    ADD COLUMN picture_x DOUBLE PRECISION,
    ADD COLUMN picture_y DOUBLE PRECISION,
    ADD COLUMN picture_width DOUBLE PRECISION,
    ADD COLUMN picture_height DOUBLE PRECISION,
    ADD COLUMN picture_checked_at TIMESTAMPTZ;
