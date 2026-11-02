ALTER TABLE hpa_tenant.marketplace_remote_packages
    ADD COLUMN IF NOT EXISTS provenance_json JSONB,
    ADD COLUMN IF NOT EXISTS oci_json JSONB;
