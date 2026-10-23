-- Links to a metadata provider (Hardcover) that reconcile could not settle on its own:
-- the provider deleted an entity we still hold user data for, or merged two entities
-- we both hold user data for. Entity ids are polymorphic, so there is no FK; reconcile
-- prunes rows whose entity is gone.
CREATE TABLE provider_link_issues (
    entity_type TEXT NOT NULL CHECK (entity_type IN ('author', 'series', 'book')),
    entity_id UUID NOT NULL,
    provider TEXT NOT NULL,
    external_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('deleted', 'merge_conflict')),
    canonical_external_id TEXT,
    conflicting_entity_id UUID,
    detected_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (entity_type, entity_id, provider)
);

-- A book carries something a user or the library put there, so following the provider
-- must not delete it: files, a want in flight, reading progress, shelves, devices, edits.
-- 'pending' (discovered, undecided) and 'skipped' carry nothing.
CREATE FUNCTION book_has_user_data(p_book_id UUID) RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT EXISTS (
        SELECT 1 FROM books b
        WHERE b.id = p_book_id
          AND (
              b.want_status NOT IN ('none', 'pending', 'skipped')
              OR b.user_edited_at IS NOT NULL
              OR EXISTS (SELECT 1 FROM book_files WHERE book_id = b.id)
              OR EXISTS (SELECT 1 FROM audiobook_files WHERE book_id = b.id)
              OR EXISTS (SELECT 1 FROM media_root_files
                         WHERE entity_type = 'book' AND entity_id = b.id)
              OR EXISTS (SELECT 1 FROM book_ingest_source WHERE book_id = b.id)
              OR EXISTS (SELECT 1 FROM reading_states WHERE book_id = b.id)
              OR EXISTS (SELECT 1 FROM user_book_data WHERE book_id = b.id)
              OR EXISTS (SELECT 1 FROM shelf_items WHERE book_id = b.id)
              OR EXISTS (SELECT 1 FROM entitlements WHERE book_id = b.id)
              OR EXISTS (SELECT 1 FROM kindle_deliveries WHERE book_id = b.id)
              OR EXISTS (SELECT 1 FROM audiobook_bookmarks WHERE book_id = b.id)
              OR EXISTS (SELECT 1 FROM cross_format_sync WHERE book_id = b.id)
          )
    )
$$;
