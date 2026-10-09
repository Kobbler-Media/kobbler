-- Normalize tv_shows.status to canonical lowercase values.
-- TMDB used "Returning Series", "Ended", "Canceled", "In Production", "Planned", "Pilot".
-- TVDB used "Continuing", "Ended", "Upcoming".
-- Canonical: "continuing", "ended", "canceled", "upcoming".

UPDATE tv_shows SET status = 'continuing' WHERE LOWER(status) IN ('returning series', 'continuing');
UPDATE tv_shows SET status = 'ended' WHERE LOWER(status) = 'ended' AND status != 'ended';
UPDATE tv_shows SET status = 'canceled' WHERE LOWER(status) IN ('canceled', 'cancelled');
UPDATE tv_shows SET status = 'upcoming' WHERE LOWER(status) IN ('upcoming', 'in production', 'planned', 'pilot');
