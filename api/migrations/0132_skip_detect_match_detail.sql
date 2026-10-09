ALTER TABLE skip_detect_candidates
    ADD COLUMN credit_text_score DOUBLE PRECISION,
    ADD COLUMN match_detail JSONB;
