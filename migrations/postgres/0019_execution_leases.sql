CREATE TABLE hpa_tenant.execution_leases (
    tenant_id TEXT NOT NULL,
    execution_id TEXT NOT NULL,
    holder_id TEXT NOT NULL,
    fencing_token BIGINT NOT NULL CHECK (fencing_token > 0),
    acquired_at TIMESTAMPTZ NOT NULL,
    renewed_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, execution_id),
    FOREIGN KEY (tenant_id, execution_id)
        REFERENCES hpa_tenant.executions(tenant_id, id) ON DELETE CASCADE,
    CHECK (expires_at > renewed_at)
);

CREATE INDEX execution_leases_expiry
    ON hpa_tenant.execution_leases (tenant_id, expires_at);

ALTER TABLE hpa_tenant.execution_leases ENABLE ROW LEVEL SECURITY;
ALTER TABLE hpa_tenant.execution_leases FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON hpa_tenant.execution_leases
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

GRANT SELECT, INSERT, UPDATE, DELETE ON hpa_tenant.execution_leases
    TO hpa_tenant_runtime, hpa_platform_admin;
GRANT SELECT ON hpa_tenant.execution_leases TO hpa_operator, hpa_readonly;

ALTER TABLE hpa_tenant.execution_leases OWNER TO hpa_migrator;
