CREATE TABLE IF NOT EXISTS hpa_tenant.agent_tool_calls (
    tenant_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    call_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    tool_name TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    status TEXT NOT NULL CHECK (
        status IN ('in_progress', 'succeeded', 'failed', 'denied')
    ),
    response_json JSONB,
    error TEXT,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, session_id, call_id),
    UNIQUE (tenant_id, session_id, idempotency_key),
    FOREIGN KEY (tenant_id, session_id)
        REFERENCES hpa_tenant.agent_sessions(tenant_id, id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS agent_tool_calls_status
    ON hpa_tenant.agent_tool_calls (tenant_id, status, updated_at);

ALTER TABLE hpa_tenant.agent_tool_calls ENABLE ROW LEVEL SECURITY;
ALTER TABLE hpa_tenant.agent_tool_calls FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS tenant_isolation ON hpa_tenant.agent_tool_calls;
CREATE POLICY tenant_isolation ON hpa_tenant.agent_tool_calls
    USING (
        current_user IN ('hpa_platform_admin', 'hpa_operator')
        OR tenant_id = hpa_internal.current_tenant()
    )
    WITH CHECK (
        current_user IN ('hpa_platform_admin', 'hpa_tenant_runtime')
        AND (
            current_user = 'hpa_platform_admin'
            OR tenant_id = hpa_internal.current_tenant()
        )
    );

GRANT SELECT, INSERT, UPDATE, DELETE ON hpa_tenant.agent_tool_calls
    TO hpa_tenant_runtime, hpa_platform_admin;
GRANT SELECT ON hpa_tenant.agent_tool_calls TO hpa_operator, hpa_readonly;

ALTER TABLE hpa_tenant.agent_tool_calls OWNER TO hpa_migrator;
