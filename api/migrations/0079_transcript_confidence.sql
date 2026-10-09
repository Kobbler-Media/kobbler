-- Add confidence metadata columns from faster-whisper to transcript segments
ALTER TABLE audiobook_transcripts
    ADD COLUMN avg_logprob DOUBLE PRECISION,
    ADD COLUMN no_speech_prob DOUBLE PRECISION,
    ADD COLUMN compression_ratio DOUBLE PRECISION;
