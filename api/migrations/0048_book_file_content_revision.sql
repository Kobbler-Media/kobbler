ALTER TABLE book_files ADD COLUMN content_revision UUID NOT NULL DEFAULT gen_random_uuid();
