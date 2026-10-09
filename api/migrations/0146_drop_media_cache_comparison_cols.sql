-- Drop the V1-vs-V2 comparison scaffolding from media_cache_runs.
--
-- These columns were added by 0145 to hold side-by-side diffing data for a
-- temporary "agent on the server" validation. They don't belong in permanent
-- schema — comparison is done externally via SQL, not baked into the app.
-- The media_cache_runs table itself stays: its pipeline_version, triggered_by,
-- branch_results and timing are still load-bearing for the backfill walker.

ALTER TABLE media_cache_runs DROP COLUMN skip_detect_v2_results;
ALTER TABLE media_cache_runs DROP COLUMN legacy_results;
ALTER TABLE media_cache_runs DROP COLUMN diff_summary;
