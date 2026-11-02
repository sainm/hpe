ALTER TABLE hpa_control.oidc_config
    ADD COLUMN IF NOT EXISTS end_session_endpoint TEXT NOT NULL DEFAULT '',
    ADD COLUMN IF NOT EXISTS post_logout_redirect_uri TEXT NOT NULL DEFAULT '';

ALTER TABLE hpa_control.sessions
    ADD COLUMN IF NOT EXISTS refresh_token_ciphertext TEXT,
    ADD COLUMN IF NOT EXISTS refresh_token_version INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS refresh_token_expires_at BIGINT,
    ADD COLUMN IF NOT EXISTS id_token_ciphertext TEXT,
    ADD COLUMN IF NOT EXISTS access_token_expires_at BIGINT,
    ADD COLUMN IF NOT EXISTS last_refresh_at BIGINT;

CREATE TABLE IF NOT EXISTS hpa_control.logout_flows (
    state_hash TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES hpa_control.tenants(id) ON DELETE CASCADE,
    created_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS logout_flows_expiry
    ON hpa_control.logout_flows (tenant_id, expires_at);

ALTER TABLE hpa_control.logout_flows ENABLE ROW LEVEL SECURITY;
ALTER TABLE hpa_control.logout_flows FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS tenant_isolation ON hpa_control.logout_flows;
CREATE POLICY tenant_isolation ON hpa_control.logout_flows
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

GRANT SELECT, INSERT, UPDATE, DELETE ON hpa_control.logout_flows
    TO hpa_tenant_runtime;
GRANT SELECT, INSERT, UPDATE, DELETE ON hpa_control.logout_flows
    TO hpa_platform_admin;
GRANT SELECT ON hpa_control.logout_flows TO hpa_operator, hpa_readonly;

ALTER TABLE hpa_control.logout_flows OWNER TO hpa_migrator;
