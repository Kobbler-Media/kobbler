-- Which extraction slots came back null, without the values. Metadata, so a
-- non-owner admin can tune the resolver without reading the conversation.
ALTER TABLE chat_turn_logs ADD COLUMN extract_shape JSONB;
