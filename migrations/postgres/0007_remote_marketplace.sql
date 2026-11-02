CREATE TABLE hpa_tenant.plugin_publishers (
    tenant_id TEXT NOT NULL REFERENCES hpa_control.tenants(id) ON DELETE CASCADE,
    id TEXT NOT NULL,
    name TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('active', 'suspended')),
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, id)
);

CREATE TABLE hpa_tenant.plugin_publisher_keys (
    tenant_id TEXT NOT NULL,
    publisher_id TEXT NOT NULL,
    key_id TEXT NOT NULL,
    public_key TEXT NOT NULL,
    label TEXT NOT NULL,
    valid_from TIMESTAMPTZ,
    valid_until TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, publisher_id, key_id),
    FOREIGN KEY (tenant_id, publisher_id)
        REFERENCES hpa_tenant.plugin_publishers(tenant_id, id) ON DELETE CASCADE
);

CREATE TABLE hpa_tenant.marketplace_sources (
    tenant_id TEXT NOT NULL REFERENCES hpa_control.tenants(id) ON DELETE CASCADE,
    id TEXT NOT NULL,
    label TEXT NOT NULL,
    index_uri TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    last_sequence BIGINT NOT NULL DEFAULT 0,
    last_refresh_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, id)
);

CREATE TABLE hpa_tenant.marketplace_source_keys (
    tenant_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    key_id TEXT NOT NULL,
    public_key TEXT NOT NULL,
    label TEXT NOT NULL,
    valid_from TIMESTAMPTZ,
    valid_until TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, source_id, key_id),
    FOREIGN KEY (tenant_id, source_id)
        REFERENCES hpa_tenant.marketplace_sources(tenant_id, id) ON DELETE CASCADE
);

CREATE TABLE hpa_tenant.marketplace_remote_packages (
    tenant_id TEXT NOT NULL REFERENCES hpa_control.tenants(id) ON DELETE CASCADE,
    source_id TEXT NOT NULL,
    plugin_id TEXT NOT NULL,
    version TEXT NOT NULL,
    publisher_id TEXT NOT NULL,
    signer_key_id TEXT NOT NULL,
    package_sha256 TEXT NOT NULL,
    package_path TEXT NOT NULL,
    review_status TEXT NOT NULL,
    index_sequence BIGINT NOT NULL,
    published_at TIMESTAMPTZ,
    withdrawn_at TIMESTAMPTZ,
    revocation_reason TEXT,
    PRIMARY KEY (tenant_id, source_id, plugin_id, version),
    FOREIGN KEY (tenant_id, source_id)
        REFERENCES hpa_tenant.marketplace_sources(tenant_id, id) ON DELETE CASCADE
);

CREATE INDEX marketplace_remote_packages_plugin
    ON hpa_tenant.marketplace_remote_packages (tenant_id, plugin_id, version);

ALTER TABLE hpa_tenant.plugin_publishers ENABLE ROW LEVEL SECURITY;
ALTER TABLE hpa_tenant.plugin_publishers FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON hpa_tenant.plugin_publishers
    USING (
        current_user IN ('hpa_platform_admin', 'hpa_operator')
        OR tenant_id = hpa_internal.current_tenant()
    )
    WITH CHECK (
        current_user = 'hpa_platform_admin'
        OR tenant_id = hpa_internal.current_tenant()
    );

ALTER TABLE hpa_tenant.plugin_publisher_keys ENABLE ROW LEVEL SECURITY;
ALTER TABLE hpa_tenant.plugin_publisher_keys FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON hpa_tenant.plugin_publisher_keys
    USING (
        current_user IN ('hpa_platform_admin', 'hpa_operator')
        OR tenant_id = hpa_internal.current_tenant()
    )
    WITH CHECK (
        current_user = 'hpa_platform_admin'
        OR tenant_id = hpa_internal.current_tenant()
    );

ALTER TABLE hpa_tenant.marketplace_sources ENABLE ROW LEVEL SECURITY;
ALTER TABLE hpa_tenant.marketplace_sources FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON hpa_tenant.marketplace_sources
    USING (
        current_user IN ('hpa_platform_admin', 'hpa_operator')
        OR tenant_id = hpa_internal.current_tenant()
    )
    WITH CHECK (
        current_user = 'hpa_platform_admin'
        OR tenant_id = hpa_internal.current_tenant()
    );

ALTER TABLE hpa_tenant.marketplace_source_keys ENABLE ROW LEVEL SECURITY;
ALTER TABLE hpa_tenant.marketplace_source_keys FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON hpa_tenant.marketplace_source_keys
    USING (
        current_user IN ('hpa_platform_admin', 'hpa_operator')
        OR tenant_id = hpa_internal.current_tenant()
    )
    WITH CHECK (
        current_user = 'hpa_platform_admin'
        OR tenant_id = hpa_internal.current_tenant()
    );

ALTER TABLE hpa_tenant.marketplace_remote_packages ENABLE ROW LEVEL SECURITY;
ALTER TABLE hpa_tenant.marketplace_remote_packages FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON hpa_tenant.marketplace_remote_packages
    USING (
        current_user IN ('hpa_platform_admin', 'hpa_operator')
        OR tenant_id = hpa_internal.current_tenant()
    )
    WITH CHECK (
        current_user = 'hpa_platform_admin'
        OR tenant_id = hpa_internal.current_tenant()
    );

GRANT SELECT, INSERT, UPDATE, DELETE ON
    hpa_tenant.plugin_publishers,
    hpa_tenant.plugin_publisher_keys,
    hpa_tenant.marketplace_sources,
    hpa_tenant.marketplace_source_keys,
    hpa_tenant.marketplace_remote_packages
    TO hpa_tenant_runtime;
GRANT SELECT ON
    hpa_tenant.plugin_publishers,
    hpa_tenant.plugin_publisher_keys,
    hpa_tenant.marketplace_sources,
    hpa_tenant.marketplace_source_keys,
    hpa_tenant.marketplace_remote_packages
    TO hpa_operator, hpa_readonly;

ALTER TABLE hpa_tenant.plugin_publishers OWNER TO hpa_migrator;
ALTER TABLE hpa_tenant.plugin_publisher_keys OWNER TO hpa_migrator;
ALTER TABLE hpa_tenant.marketplace_sources OWNER TO hpa_migrator;
ALTER TABLE hpa_tenant.marketplace_source_keys OWNER TO hpa_migrator;
ALTER TABLE hpa_tenant.marketplace_remote_packages OWNER TO hpa_migrator;
