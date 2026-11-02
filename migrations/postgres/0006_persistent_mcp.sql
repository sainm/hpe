CREATE TABLE hpa_control.mcp_sessions (
    tenant_id TEXT NOT NULL REFERENCES hpa_control.tenants(id) ON DELETE CASCADE,
    id TEXT NOT NULL,
    token_id TEXT NOT NULL,
    subject TEXT,
    created_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    last_seen_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, id),
    CHECK (expires_at > created_at)
);

CREATE INDEX mcp_sessions_expiry
    ON hpa_control.mcp_sessions (tenant_id, expires_at)
    WHERE revoked_at IS NULL;

CREATE TABLE hpa_control.mcp_subscriptions (
    tenant_id TEXT NOT NULL,
    id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    requested_json JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, id),
    FOREIGN KEY (tenant_id, session_id)
        REFERENCES hpa_control.mcp_sessions(tenant_id, id) ON DELETE CASCADE,
    CHECK (expires_at > created_at)
);

CREATE INDEX mcp_subscriptions_session
    ON hpa_control.mcp_subscriptions (tenant_id, session_id, created_at DESC);

CREATE TABLE hpa_control.mcp_notifications (
    tenant_id TEXT NOT NULL REFERENCES hpa_control.tenants(id) ON DELETE CASCADE,
    sequence BIGINT GENERATED ALWAYS AS IDENTITY,
    kind TEXT NOT NULL CHECK (kind IN ('tools', 'resources', 'prompts', 'tasks')),
    payload_json JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, sequence)
);

CREATE INDEX mcp_notifications_tenant_sequence
    ON hpa_control.mcp_notifications (tenant_id, sequence);

CREATE TABLE hpa_control.mcp_tasks (
    tenant_id TEXT NOT NULL REFERENCES hpa_control.tenants(id) ON DELETE CASCADE,
    id TEXT NOT NULL,
    session_id TEXT,
    token_id TEXT NOT NULL,
    method TEXT NOT NULL,
    status TEXT NOT NULL CHECK (
        status IN (
            'working', 'input_required', 'completed', 'failed',
            'cancelled', 'expired'
        )
    ),
    input_json JSONB NOT NULL,
    output_json JSONB,
    error_json JSONB,
    approval_json JSONB,
    execution_id TEXT,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ
);

ALTER TABLE hpa_control.mcp_tasks
    ADD PRIMARY KEY (tenant_id, id);
ALTER TABLE hpa_control.mcp_tasks
    ADD FOREIGN KEY (tenant_id, session_id)
    REFERENCES hpa_control.mcp_sessions(tenant_id, id)
    ON DELETE SET NULL (session_id);

CREATE INDEX mcp_tasks_status
    ON hpa_control.mcp_tasks (tenant_id, status, updated_at);

ALTER TABLE hpa_control.mcp_sessions ENABLE ROW LEVEL SECURITY;
ALTER TABLE hpa_control.mcp_sessions FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON hpa_control.mcp_sessions
    USING (
        current_user IN ('hpa_platform_admin', 'hpa_operator')
        OR tenant_id = hpa_internal.current_tenant()
    )
    WITH CHECK (
        current_user = 'hpa_platform_admin'
        OR tenant_id = hpa_internal.current_tenant()
    );

ALTER TABLE hpa_control.mcp_subscriptions ENABLE ROW LEVEL SECURITY;
ALTER TABLE hpa_control.mcp_subscriptions FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON hpa_control.mcp_subscriptions
    USING (
        current_user IN ('hpa_platform_admin', 'hpa_operator')
        OR tenant_id = hpa_internal.current_tenant()
    )
    WITH CHECK (
        current_user = 'hpa_platform_admin'
        OR tenant_id = hpa_internal.current_tenant()
    );

ALTER TABLE hpa_control.mcp_notifications ENABLE ROW LEVEL SECURITY;
ALTER TABLE hpa_control.mcp_notifications FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON hpa_control.mcp_notifications
    USING (
        current_user IN ('hpa_platform_admin', 'hpa_operator')
        OR tenant_id = hpa_internal.current_tenant()
    )
    WITH CHECK (
        current_user = 'hpa_platform_admin'
        OR tenant_id = hpa_internal.current_tenant()
    );

ALTER TABLE hpa_control.mcp_tasks ENABLE ROW LEVEL SECURITY;
ALTER TABLE hpa_control.mcp_tasks FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON hpa_control.mcp_tasks
    USING (
        current_user IN ('hpa_platform_admin', 'hpa_operator')
        OR tenant_id = hpa_internal.current_tenant()
    )
    WITH CHECK (
        current_user = 'hpa_platform_admin'
        OR tenant_id = hpa_internal.current_tenant()
    );

GRANT SELECT, INSERT, UPDATE, DELETE ON
    hpa_control.mcp_sessions,
    hpa_control.mcp_subscriptions,
    hpa_control.mcp_notifications,
    hpa_control.mcp_tasks
    TO hpa_tenant_runtime;
GRANT SELECT ON
    hpa_control.mcp_sessions,
    hpa_control.mcp_subscriptions,
    hpa_control.mcp_notifications,
    hpa_control.mcp_tasks
    TO hpa_operator, hpa_readonly;

ALTER TABLE hpa_control.mcp_sessions OWNER TO hpa_migrator;
ALTER TABLE hpa_control.mcp_subscriptions OWNER TO hpa_migrator;
ALTER TABLE hpa_control.mcp_notifications OWNER TO hpa_migrator;
ALTER TABLE hpa_control.mcp_tasks OWNER TO hpa_migrator;
