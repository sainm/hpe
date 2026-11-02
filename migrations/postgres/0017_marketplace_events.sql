CREATE TABLE IF NOT EXISTS hpa_tenant.marketplace_event_stream (
    tenant_id TEXT NOT NULL,
    sequence BIGINT GENERATED ALWAYS AS IDENTITY,
    id TEXT NOT NULL,
    at TIMESTAMPTZ NOT NULL,
    plugin_id TEXT,
    version TEXT,
    kind TEXT NOT NULL,
    detail_json JSONB NOT NULL,
    PRIMARY KEY (tenant_id, sequence),
    UNIQUE (tenant_id, id),
    FOREIGN KEY (tenant_id)
        REFERENCES hpa_control.tenants(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS marketplace_event_stream_at
    ON hpa_tenant.marketplace_event_stream (tenant_id, at);

ALTER TABLE hpa_tenant.marketplace_event_stream ENABLE ROW LEVEL SECURITY;
ALTER TABLE hpa_tenant.marketplace_event_stream FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS tenant_isolation ON hpa_tenant.marketplace_event_stream;
CREATE POLICY tenant_isolation ON hpa_tenant.marketplace_event_stream
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

GRANT SELECT, INSERT, UPDATE, DELETE ON hpa_tenant.marketplace_event_stream
    TO hpa_tenant_runtime, hpa_platform_admin;
GRANT SELECT ON hpa_tenant.marketplace_event_stream TO hpa_operator, hpa_readonly;

ALTER TABLE hpa_tenant.marketplace_event_stream OWNER TO hpa_migrator;
