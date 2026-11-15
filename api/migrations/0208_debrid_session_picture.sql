-- Where the picture sits inside a debrid release's frame, as fractions of it (the shape
-- library files keep in picture_x..picture_height). NULL until measured; the whole frame
-- when there are no bars.
ALTER TABLE debrid_stream_sessions ADD COLUMN picture JSONB;
