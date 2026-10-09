-- Store the Hardcover user_book_read ID so we can update instead of insert
ALTER TABLE user_book_data ADD COLUMN IF NOT EXISTS hardcover_read_id BIGINT;
COMMENT ON COLUMN user_book_data.hardcover_read_id IS 'Hardcover user_book_read ID for update calls';
