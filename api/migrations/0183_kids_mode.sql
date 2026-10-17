-- Kids profiles see titles rated teen-and-under (US certifications, as TMDB and OMDB
-- report them) or ones an admin has allowed by hand. kids_override is never written by
-- metadata, so it survives every refresh; NULL defers to the rating, and an unrated
-- title with no override stays hidden.
ALTER TABLE users ADD COLUMN is_kids BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE movies ADD COLUMN kids_override BOOLEAN;
ALTER TABLE tv_shows ADD COLUMN kids_override BOOLEAN;

CREATE FUNCTION kids_rating_allowed(rating TEXT) RETURNS BOOLEAN
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT upper(btrim(rating)) IN
        ('G', 'PG', 'PG-13', 'TV-Y', 'TV-Y7', 'TV-Y7-FV', 'TV-G', 'TV-PG', 'TV-14')
$$;

CREATE FUNCTION kids_allowed(kids_override BOOLEAN, rating TEXT) RETURNS BOOLEAN
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT COALESCE(kids_override, kids_rating_allowed(rating), FALSE)
$$;
