-- Change kindle_targets.device_id foreign key from SET NULL to CASCADE
-- When a Kobbler device is deleted, its linked Kindle targets should be deleted too.

-- First, clean up any orphaned kindle_targets (device_id is NULL or references deleted device)
DELETE FROM kindle_targets WHERE device_id IS NULL;

-- Now alter the constraint
ALTER TABLE kindle_targets
    DROP CONSTRAINT kindle_targets_device_id_fkey,
    ADD CONSTRAINT kindle_targets_device_id_fkey
        FOREIGN KEY (device_id) REFERENCES devices(id) ON DELETE CASCADE;
