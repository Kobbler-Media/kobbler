-- A web-reader/app position (stored as epubcfi in location_value) translated
-- to the KoboSpan a Kobo would report for the same text, so a Kobo resumes
-- where the app or web reader left off instead of only at the same percent.
ALTER TABLE reading_bookmark
    ADD COLUMN kobo_location_value TEXT,
    ADD COLUMN kobo_location_source TEXT;
