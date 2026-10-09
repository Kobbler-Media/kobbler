-- Fix duplicate entitlements causing duplicate books in Kobo sync responses.
--
-- Root causes:
-- 1. No unique constraint on (book_id, scope_type, scope_id)
-- 2. Orphaned DEVICE entitlements from deleted devices
-- 3. Re-processing (watcher rescans, retries) creates duplicate rows

-- Step 1: Delete orphaned DEVICE entitlements where the device no longer exists.
DELETE FROM entitlements
WHERE scope_type = 'DEVICE'
  AND scope_id IS NOT NULL
  AND NOT EXISTS (
    SELECT 1 FROM devices WHERE devices.id = entitlements.scope_id
  );

-- Step 2: Deduplicate existing rows.
-- Keep the newest row per (book_id, scope_type, scope_id) combination.
DELETE FROM entitlements
WHERE id IN (
    SELECT id FROM (
        SELECT id,
               ROW_NUMBER() OVER (
                   PARTITION BY book_id, scope_type, scope_id
                   ORDER BY updated_at DESC, created_at DESC, id DESC
               ) AS rn
        FROM entitlements
    ) ranked
    WHERE rn > 1
);

-- Step 3: Add unique constraint to prevent future duplicates.
-- NULLS NOT DISTINCT ensures (book_id, 'ALL_USERS', NULL) is treated as a single combo.
ALTER TABLE entitlements
    ADD CONSTRAINT uq_entitlements_book_scope
    UNIQUE NULLS NOT DISTINCT (book_id, scope_type, scope_id);
