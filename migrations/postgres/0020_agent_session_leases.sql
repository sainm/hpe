ALTER TABLE hpa_tenant.agent_sessions
    ADD COLUMN lease_owner TEXT,
    ADD COLUMN lease_token BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN lease_expires_at TIMESTAMPTZ;

CREATE INDEX IF NOT EXISTS agent_sessions_lease
    ON hpa_tenant.agent_sessions (tenant_id, lease_expires_at)
    WHERE lease_expires_at IS NOT NULL;
