-- Make Hardcover the top priority metadata provider
-- Lower priority number = higher priority

UPDATE metadata_providers SET priority = 10 WHERE provider_type = 'hardcover';
UPDATE metadata_providers SET priority = 20 WHERE provider_type = 'googlebooks';
UPDATE metadata_providers SET priority = 30 WHERE provider_type = 'openlibrary';
