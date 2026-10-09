CREATE TABLE chat_turn_logs (
    id UUID PRIMARY KEY,
    session_id UUID,
    user_id UUID REFERENCES users(id) ON DELETE SET NULL,
    transport TEXT NOT NULL,
    user_message TEXT,

    succeeded BOOLEAN NOT NULL DEFAULT FALSE,
    turn_error TEXT,

    extract_ms INTEGER,
    extract_output JSONB,
    extract_error TEXT,

    resolve_ms INTEGER,
    resolve_query TEXT,
    resolved_ref TEXT,
    resolve_candidate_count INTEGER,
    resolve_candidates JSONB,
    resolve_error TEXT,

    action_ms INTEGER,
    action_name TEXT,
    action_args JSONB,
    action_result JSONB,
    action_error TEXT,

    llm_provider TEXT,
    llm_model TEXT,
    llm_ms INTEGER,
    prompt_tokens INTEGER,
    completion_tokens INTEGER,
    rounds INTEGER NOT NULL DEFAULT 0,

    tool_calls JSONB NOT NULL DEFAULT '[]'::jsonb,
    tool_call_count INTEGER NOT NULL DEFAULT 0,

    assistant_text TEXT,
    total_ms INTEGER,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_chat_turn_logs_created ON chat_turn_logs (created_at DESC);
CREATE INDEX idx_chat_turn_logs_user ON chat_turn_logs (user_id, created_at DESC);
CREATE INDEX idx_chat_turn_logs_session ON chat_turn_logs (session_id, created_at DESC);
CREATE INDEX idx_chat_turn_logs_transport ON chat_turn_logs (transport, created_at DESC);
CREATE INDEX idx_chat_turn_logs_failed ON chat_turn_logs (created_at DESC) WHERE NOT succeeded;
CREATE INDEX idx_chat_turn_logs_extract_error ON chat_turn_logs (created_at DESC) WHERE extract_error IS NOT NULL;
CREATE INDEX idx_chat_turn_logs_resolve_error ON chat_turn_logs (created_at DESC) WHERE resolve_error IS NOT NULL;
CREATE INDEX idx_chat_turn_logs_action_error ON chat_turn_logs (created_at DESC) WHERE action_error IS NOT NULL;

-- chat_sessions is created by 0157, owned by another workstream; the FK is added
-- only once that migration has landed so 0158 applies standalone either way.
DO $$
BEGIN
    IF to_regclass('public.chat_sessions') IS NOT NULL THEN
        ALTER TABLE chat_turn_logs
            ADD CONSTRAINT chat_turn_logs_session_id_fkey
            FOREIGN KEY (session_id) REFERENCES chat_sessions(id) ON DELETE SET NULL;
    END IF;
END $$;
