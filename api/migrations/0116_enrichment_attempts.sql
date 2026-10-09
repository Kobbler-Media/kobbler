-- Track enrichment job attempts so we don't infinitely re-enqueue
-- entities that legitimately produce no results at a given config.
CREATE TABLE enrichment_attempts (
    entity_type TEXT NOT NULL,
    entity_id UUID NOT NULL,
    job_type TEXT NOT NULL,
    config_tag TEXT NOT NULL,
    attempted_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (entity_type, entity_id, job_type, config_tag)
);
