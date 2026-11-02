CREATE SCHEMA IF NOT EXISTS hpa_control;
CREATE SCHEMA IF NOT EXISTS hpa_tenant;
CREATE SCHEMA IF NOT EXISTS hpa_internal;

CREATE TABLE IF NOT EXISTS hpa_control.tenants (
    id TEXT PRIMARY KEY CHECK (id ~ '^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$'),
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'suspended', 'deleted')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS hpa_control.quotas (
    tenant_id TEXT PRIMARY KEY REFERENCES hpa_control.tenants(id) ON DELETE CASCADE,
    max_workflows BIGINT NOT NULL CHECK (max_workflows >= 0),
    max_executions_per_day BIGINT NOT NULL CHECK (max_executions_per_day >= 0),
    max_skills BIGINT NOT NULL CHECK (max_skills >= 0),
    max_agents BIGINT NOT NULL CHECK (max_agents >= 0),
    max_plugins BIGINT NOT NULL CHECK (max_plugins >= 0),
    max_connections BIGINT NOT NULL CHECK (max_connections >= 0),
    max_concurrent_runs BIGINT NOT NULL CHECK (max_concurrent_runs >= 0)
);

CREATE TABLE IF NOT EXISTS hpa_control.tokens (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES hpa_control.tenants(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    role TEXT NOT NULL,
    platform_admin BOOLEAN NOT NULL DEFAULT FALSE,
    token_hash TEXT NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS tokens_tenant
    ON hpa_control.tokens (tenant_id, created_at DESC);

CREATE TABLE IF NOT EXISTS hpa_control.members (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES hpa_control.tenants(id) ON DELETE CASCADE,
    subject TEXT NOT NULL,
    display_name TEXT NOT NULL,
    email TEXT,
    role TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('active', 'disabled')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, subject)
);

CREATE INDEX IF NOT EXISTS members_tenant
    ON hpa_control.members (tenant_id, created_at DESC);

CREATE TABLE IF NOT EXISTS hpa_control.audit (
    id TEXT PRIMARY KEY,
    at TIMESTAMPTZ NOT NULL DEFAULT now(),
    tenant_id TEXT REFERENCES hpa_control.tenants(id) ON DELETE SET NULL,
    actor_id TEXT,
    actor_type TEXT NOT NULL,
    action TEXT NOT NULL,
    resource TEXT NOT NULL,
    decision TEXT NOT NULL CHECK (decision IN ('allow', 'deny')),
    reason TEXT,
    detail_json JSONB NOT NULL DEFAULT '{}'::jsonb
);

CREATE INDEX IF NOT EXISTS audit_at
    ON hpa_control.audit (at DESC);
CREATE INDEX IF NOT EXISTS audit_tenant_at
    ON hpa_control.audit (tenant_id, at DESC);

CREATE TABLE IF NOT EXISTS hpa_control.backups (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES hpa_control.tenants(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    size BIGINT NOT NULL CHECK (size >= 0),
    sha256 TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS backups_tenant
    ON hpa_control.backups (tenant_id, created_at DESC);

CREATE TABLE IF NOT EXISTS hpa_control.workers (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES hpa_control.tenants(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('active', 'disabled')),
    capabilities_json JSONB NOT NULL DEFAULT '["workflow"]'::jsonb,
    max_concurrency INTEGER NOT NULL DEFAULT 1 CHECK (max_concurrency BETWEEN 1 AND 64),
    runtime_version TEXT,
    last_heartbeat_at BIGINT NOT NULL,
    registered_at BIGINT NOT NULL,
    metadata_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    UNIQUE (tenant_id, id)
);

CREATE INDEX IF NOT EXISTS workers_tenant
    ON hpa_control.workers (tenant_id, registered_at DESC);

CREATE TABLE IF NOT EXISTS hpa_control.worker_tokens (
    id TEXT PRIMARY KEY,
    worker_id TEXT NOT NULL REFERENCES hpa_control.workers(id) ON DELETE CASCADE,
    tenant_id TEXT NOT NULL REFERENCES hpa_control.tenants(id) ON DELETE CASCADE,
    token_hash TEXT NOT NULL UNIQUE,
    created_at BIGINT NOT NULL,
    revoked_at BIGINT,
    UNIQUE (worker_id, tenant_id),
    FOREIGN KEY (tenant_id, worker_id)
        REFERENCES hpa_control.workers(tenant_id, id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS hpa_control.worker_jobs (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES hpa_control.tenants(id) ON DELETE CASCADE,
    worker_id TEXT REFERENCES hpa_control.workers(id) ON DELETE SET NULL,
    execution_id TEXT NOT NULL,
    workflow_id TEXT NOT NULL,
    workflow_version TEXT NOT NULL,
    workflow_json JSONB NOT NULL,
    state_json JSONB NOT NULL,
    status TEXT NOT NULL CHECK (
        status IN ('queued', 'leased', 'running', 'waiting_approval', 'succeeded', 'failed', 'cancelled')
    ),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    lease_id TEXT,
    lease_expires_at BIGINT,
    runtime_version TEXT NOT NULL,
    required_capabilities_json JSONB NOT NULL DEFAULT '["workflow"]'::jsonb,
    resumed BOOLEAN NOT NULL DEFAULT FALSE,
    cancel_requested BOOLEAN NOT NULL DEFAULT FALSE,
    error TEXT,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    UNIQUE (tenant_id, execution_id),
    FOREIGN KEY (tenant_id, worker_id)
        REFERENCES hpa_control.workers(tenant_id, id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS worker_jobs_tenant_status
    ON hpa_control.worker_jobs (tenant_id, status, created_at);
CREATE INDEX IF NOT EXISTS worker_jobs_lease
    ON hpa_control.worker_jobs (lease_expires_at)
    WHERE status IN ('leased', 'running');
CREATE UNIQUE INDEX IF NOT EXISTS worker_jobs_lease_id
    ON hpa_control.worker_jobs (lease_id)
    WHERE lease_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS hpa_control.oidc_config (
    id SMALLINT PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    issuer TEXT NOT NULL,
    audience TEXT NOT NULL,
    jwks_url TEXT NOT NULL,
    network_json JSONB NOT NULL DEFAULT '[]'::jsonb,
    client_id TEXT NOT NULL DEFAULT '',
    authorization_endpoint TEXT NOT NULL DEFAULT '',
    token_endpoint TEXT NOT NULL DEFAULT '',
    redirect_uri TEXT NOT NULL DEFAULT '',
    scopes TEXT NOT NULL DEFAULT 'openid email profile',
    admin_subjects_json JSONB NOT NULL DEFAULT '[]'::jsonb,
    cookie_secure BOOLEAN NOT NULL DEFAULT FALSE,
    session_ttl_seconds INTEGER NOT NULL DEFAULT 28800,
    updated_at BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS hpa_control.auth_flows (
    state_hash TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES hpa_control.tenants(id) ON DELETE CASCADE,
    code_verifier TEXT NOT NULL,
    nonce TEXT NOT NULL,
    redirect TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS hpa_control.sessions (
    id TEXT PRIMARY KEY,
    token_hash TEXT NOT NULL UNIQUE,
    csrf_token TEXT NOT NULL,
    tenant_id TEXT NOT NULL REFERENCES hpa_control.tenants(id) ON DELETE CASCADE,
    subject TEXT NOT NULL,
    email TEXT,
    display_name TEXT,
    role TEXT NOT NULL,
    platform_admin BOOLEAN NOT NULL DEFAULT FALSE,
    created_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL,
    revoked_at BIGINT,
    last_seen_at BIGINT
);

CREATE INDEX IF NOT EXISTS sessions_tenant
    ON hpa_control.sessions (tenant_id, created_at DESC);

CREATE TABLE IF NOT EXISTS hpa_control.oauth_clients (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    redirect_uris JSONB NOT NULL,
    grant_types JSONB NOT NULL,
    scopes TEXT NOT NULL,
    auth_method TEXT NOT NULL,
    secret_hash TEXT,
    origin TEXT NOT NULL,
    tenant_id TEXT REFERENCES hpa_control.tenants(id) ON DELETE CASCADE,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    disabled_at BIGINT,
    last_used_at BIGINT
);

CREATE TABLE IF NOT EXISTS hpa_control.oauth_signing_keys (
    kid TEXT PRIMARY KEY,
    alg TEXT NOT NULL,
    secret TEXT NOT NULL,
    public_jwk JSONB NOT NULL,
    created_at BIGINT NOT NULL,
    retired_at BIGINT,
    deleted_at BIGINT
);

CREATE TABLE IF NOT EXISTS hpa_control.oauth_requests (
    id TEXT PRIMARY KEY,
    client_id TEXT NOT NULL REFERENCES hpa_control.oauth_clients(id) ON DELETE CASCADE,
    redirect_uri TEXT NOT NULL,
    scope TEXT NOT NULL,
    state TEXT,
    resource TEXT,
    code_challenge TEXT NOT NULL,
    tenant_id TEXT NOT NULL REFERENCES hpa_control.tenants(id) ON DELETE CASCADE,
    subject TEXT NOT NULL,
    role TEXT NOT NULL,
    platform_admin BOOLEAN NOT NULL,
    created_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL,
    consumed_at BIGINT,
    decision TEXT
);

CREATE TABLE IF NOT EXISTS hpa_control.oauth_codes (
    code_hash TEXT PRIMARY KEY,
    request_id TEXT NOT NULL REFERENCES hpa_control.oauth_requests(id) ON DELETE CASCADE,
    client_id TEXT NOT NULL REFERENCES hpa_control.oauth_clients(id) ON DELETE CASCADE,
    redirect_uri TEXT NOT NULL,
    scope TEXT NOT NULL,
    code_challenge TEXT NOT NULL,
    resource TEXT,
    tenant_id TEXT NOT NULL REFERENCES hpa_control.tenants(id) ON DELETE CASCADE,
    subject TEXT NOT NULL,
    role TEXT NOT NULL,
    platform_admin BOOLEAN NOT NULL,
    created_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL,
    consumed_at BIGINT
);

CREATE TABLE IF NOT EXISTS hpa_control.oauth_tokens (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    jti TEXT,
    family_id TEXT NOT NULL,
    client_id TEXT NOT NULL REFERENCES hpa_control.oauth_clients(id) ON DELETE CASCADE,
    tenant_id TEXT NOT NULL REFERENCES hpa_control.tenants(id) ON DELETE CASCADE,
    subject TEXT NOT NULL,
    role TEXT NOT NULL,
    platform_admin BOOLEAN NOT NULL,
    scope TEXT NOT NULL,
    audience TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL,
    revoked_at BIGINT,
    revoked_reason TEXT,
    rotated_to TEXT,
    used_at BIGINT
);

CREATE INDEX IF NOT EXISTS oauth_tokens_client
    ON hpa_control.oauth_tokens (client_id, created_at DESC);
CREATE UNIQUE INDEX IF NOT EXISTS oauth_tokens_jti
    ON hpa_control.oauth_tokens (jti)
    WHERE jti IS NOT NULL;

CREATE TABLE IF NOT EXISTS hpa_control.oauth_consents (
    client_id TEXT NOT NULL REFERENCES hpa_control.oauth_clients(id) ON DELETE CASCADE,
    tenant_id TEXT NOT NULL REFERENCES hpa_control.tenants(id) ON DELETE CASCADE,
    subject TEXT NOT NULL,
    scope TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (client_id, tenant_id, subject)
);

INSERT INTO hpa_control.tenants (id, name, status)
VALUES ('local', 'Local', 'active')
ON CONFLICT (id) DO NOTHING;

INSERT INTO hpa_control.quotas (
    tenant_id,
    max_workflows,
    max_executions_per_day,
    max_skills,
    max_agents,
    max_plugins,
    max_connections,
    max_concurrent_runs
)
VALUES ('local', 100, 1000, 100, 100, 50, 100, 8)
ON CONFLICT (tenant_id) DO NOTHING;

