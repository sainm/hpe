ALTER TABLE hpa_tenant.plugin_publisher_keys
    ADD CONSTRAINT tenant_fk
    FOREIGN KEY (tenant_id)
    REFERENCES hpa_control.tenants(id) ON DELETE CASCADE;

ALTER TABLE hpa_tenant.marketplace_source_keys
    ADD CONSTRAINT tenant_fk
    FOREIGN KEY (tenant_id)
    REFERENCES hpa_control.tenants(id) ON DELETE CASCADE;

ALTER TABLE hpa_tenant.plugin_publisher_keys OWNER TO hpa_migrator;
ALTER TABLE hpa_tenant.marketplace_source_keys OWNER TO hpa_migrator;
