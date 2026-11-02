ALTER TABLE hpa_control.mcp_tasks
    ADD COLUMN IF NOT EXISTS owner_node_id TEXT;

CREATE TABLE IF NOT EXISTS hpa_control.mcp_task_updates (
    tenant_id TEXT NOT NULL REFERENCES hpa_control.tenants(id) ON DELETE CASCADE,
    id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    owner_node_id TEXT NOT NULL,
    token_id TEXT NOT NULL,
    action TEXT NOT NULL CHECK (action IN ('cancel')),
    payload_json JSONB NOT NULL,
    status TEXT NOT NULL CHECK (
        status IN ('pending', 'claimed', 'applied', 'failed')
    ),
    attempts INTEGER NOT NULL DEFAULT 0,
    available_at TIMESTAMPTZ NOT NULL,
    lease_until TIMESTAMPTZ,
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, id),
    FOREIGN KEY (tenant_id, task_id)
        REFERENCES hpa_control.mcp_tasks(tenant_id, id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX IF NOT EXISTS mcp_task_updates_active
    ON hpa_control.mcp_task_updates (tenant_id, task_id, action)
    WHERE status IN ('pending', 'claimed');
CREATE INDEX IF NOT EXISTS mcp_task_updates_claim
    ON hpa_control.mcp_task_updates (
        tenant_id, owner_node_id, status, available_at, lease_until
    );

ALTER TABLE hpa_control.mcp_task_updates ENABLE ROW LEVEL SECURITY;
ALTER TABLE hpa_control.mcp_task_updates FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS tenant_isolation ON hpa_control.mcp_task_updates;
CREATE POLICY tenant_isolation ON hpa_control.mcp_task_updates
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

GRANT SELECT, INSERT, UPDATE, DELETE ON hpa_control.mcp_task_updates
    TO hpa_tenant_runtime, hpa_platform_admin;
GRANT SELECT ON hpa_control.mcp_task_updates TO hpa_operator, hpa_readonly;

ALTER TABLE hpa_control.mcp_task_updates OWNER TO hpa_migrator;
