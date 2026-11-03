CREATE TABLE hpa_tenant.execution_idempotency (
    tenant_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    execution_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, idempotency_key),
    UNIQUE (tenant_id, execution_id),
    FOREIGN KEY (tenant_id, execution_id)
        REFERENCES hpa_tenant.executions(tenant_id, id) ON DELETE CASCADE
);

CREATE TABLE hpa_tenant.execution_cancel_requests (
    tenant_id TEXT NOT NULL,
    execution_id TEXT NOT NULL,
    requested_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, execution_id),
    FOREIGN KEY (tenant_id, execution_id)
        REFERENCES hpa_tenant.executions(tenant_id, id) ON DELETE CASCADE
);

ALTER TABLE hpa_tenant.execution_idempotency ENABLE ROW LEVEL SECURITY;
ALTER TABLE hpa_tenant.execution_idempotency FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON hpa_tenant.execution_idempotency
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

ALTER TABLE hpa_tenant.execution_cancel_requests ENABLE ROW LEVEL SECURITY;
ALTER TABLE hpa_tenant.execution_cancel_requests FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON hpa_tenant.execution_cancel_requests
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

GRANT SELECT, INSERT, UPDATE, DELETE
    ON hpa_tenant.execution_idempotency, hpa_tenant.execution_cancel_requests
    TO hpa_tenant_runtime, hpa_platform_admin;
GRANT SELECT
    ON hpa_tenant.execution_idempotency, hpa_tenant.execution_cancel_requests
    TO hpa_operator, hpa_readonly;

ALTER TABLE hpa_tenant.execution_idempotency OWNER TO hpa_migrator;
ALTER TABLE hpa_tenant.execution_cancel_requests OWNER TO hpa_migrator;
