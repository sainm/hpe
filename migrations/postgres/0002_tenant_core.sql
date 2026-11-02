CREATE TABLE IF NOT EXISTS hpa_tenant.workflows (
    tenant_id TEXT NOT NULL REFERENCES hpa_control.tenants(id) ON DELETE CASCADE,
    id TEXT NOT NULL,
    name TEXT NOT NULL,
    version TEXT NOT NULL,
    description TEXT,
    published BOOLEAN NOT NULL DEFAULT FALSE,
    ir_json JSONB NOT NULL,
    yaml TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, id)
);

CREATE TABLE IF NOT EXISTS hpa_tenant.workflow_versions (
    tenant_id TEXT NOT NULL,
    workflow_id TEXT NOT NULL,
    version TEXT NOT NULL,
    registry_version TEXT NOT NULL,
    ir_json JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, workflow_id, version),
    FOREIGN KEY (tenant_id, workflow_id)
        REFERENCES hpa_tenant.workflows(tenant_id, id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS hpa_tenant.executions (
    tenant_id TEXT NOT NULL REFERENCES hpa_control.tenants(id) ON DELETE CASCADE,
    id TEXT NOT NULL,
    workflow_id TEXT NOT NULL,
    workflow_version TEXT NOT NULL,
    status TEXT NOT NULL,
    state_json JSONB NOT NULL,
    workflow_json JSONB NOT NULL,
    output_json JSONB,
    error_json JSONB,
    started_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, id),
    FOREIGN KEY (tenant_id, workflow_id)
        REFERENCES hpa_tenant.workflows(tenant_id, id) ON DELETE RESTRICT
);

CREATE INDEX IF NOT EXISTS executions_workflow
    ON hpa_tenant.executions (tenant_id, workflow_id, started_at DESC);
CREATE INDEX IF NOT EXISTS executions_status
    ON hpa_tenant.executions (tenant_id, status, started_at);

CREATE TABLE IF NOT EXISTS hpa_tenant.remote_executions (
    tenant_id TEXT NOT NULL,
    execution_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, execution_id),
    FOREIGN KEY (tenant_id, execution_id)
        REFERENCES hpa_tenant.executions(tenant_id, id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS hpa_tenant.events (
    tenant_id TEXT NOT NULL,
    execution_id TEXT NOT NULL,
    sequence BIGINT NOT NULL CHECK (sequence > 0),
    payload_json JSONB NOT NULL,
    PRIMARY KEY (tenant_id, execution_id, sequence),
    FOREIGN KEY (tenant_id, execution_id)
        REFERENCES hpa_tenant.executions(tenant_id, id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS hpa_tenant.artifacts (
    tenant_id TEXT NOT NULL,
    id TEXT NOT NULL,
    execution_id TEXT NOT NULL,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    size BIGINT NOT NULL CHECK (size >= 0),
    content_json JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, id),
    FOREIGN KEY (tenant_id, execution_id)
        REFERENCES hpa_tenant.executions(tenant_id, id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS artifacts_execution
    ON hpa_tenant.artifacts (tenant_id, execution_id);

CREATE TABLE IF NOT EXISTS hpa_tenant.skills (
    tenant_id TEXT NOT NULL REFERENCES hpa_control.tenants(id) ON DELETE CASCADE,
    id TEXT NOT NULL,
    name TEXT NOT NULL,
    latest_version TEXT NOT NULL,
    license TEXT,
    status TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, id)
);

CREATE TABLE IF NOT EXISTS hpa_tenant.skill_versions (
    tenant_id TEXT NOT NULL,
    skill_id TEXT NOT NULL,
    version TEXT NOT NULL,
    name TEXT NOT NULL,
    license TEXT,
    manifest_json JSONB NOT NULL,
    skill_md TEXT NOT NULL,
    report_json JSONB NOT NULL,
    content_hash TEXT NOT NULL,
    status TEXT NOT NULL,
    source_path TEXT,
    imported_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    published_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, skill_id, version),
    FOREIGN KEY (tenant_id, skill_id)
        REFERENCES hpa_tenant.skills(tenant_id, id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS skill_versions_imported
    ON hpa_tenant.skill_versions (tenant_id, imported_at DESC);

CREATE TABLE IF NOT EXISTS hpa_tenant.skill_files (
    tenant_id TEXT NOT NULL,
    skill_id TEXT NOT NULL,
    version TEXT NOT NULL,
    path TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    size BIGINT NOT NULL CHECK (size >= 0),
    content BYTEA NOT NULL,
    PRIMARY KEY (tenant_id, skill_id, version, path),
    FOREIGN KEY (tenant_id, skill_id, version)
        REFERENCES hpa_tenant.skill_versions(tenant_id, skill_id, version) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS hpa_tenant.skill_runs (
    tenant_id TEXT NOT NULL,
    id TEXT NOT NULL,
    skill_id TEXT NOT NULL,
    version TEXT NOT NULL,
    script_id TEXT NOT NULL,
    status TEXT NOT NULL,
    input_json JSONB NOT NULL,
    output_json JSONB,
    stdout TEXT,
    stderr TEXT,
    exit_code INTEGER,
    error_json JSONB,
    sandbox TEXT NOT NULL,
    duration_ms BIGINT NOT NULL CHECK (duration_ms >= 0),
    started_at TIMESTAMPTZ NOT NULL,
    finished_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, id),
    FOREIGN KEY (tenant_id, skill_id, version)
        REFERENCES hpa_tenant.skill_versions(tenant_id, skill_id, version) ON DELETE RESTRICT
);

CREATE INDEX IF NOT EXISTS skill_runs_skill
    ON hpa_tenant.skill_runs (tenant_id, skill_id, started_at DESC);

CREATE TABLE IF NOT EXISTS hpa_tenant.skill_run_artifacts (
    tenant_id TEXT NOT NULL,
    id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    name TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    size BIGINT NOT NULL CHECK (size >= 0),
    mime_type TEXT NOT NULL,
    stored_path TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, id),
    FOREIGN KEY (tenant_id, run_id)
        REFERENCES hpa_tenant.skill_runs(tenant_id, id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS hpa_tenant.mcp_tokens (
    tenant_id TEXT NOT NULL,
    id TEXT NOT NULL,
    name TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    scopes_json JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, id)
);

CREATE TABLE IF NOT EXISTS hpa_tenant.mcp_approvals (
    tenant_id TEXT NOT NULL,
    id TEXT NOT NULL,
    token_id TEXT NOT NULL,
    tool_name TEXT NOT NULL,
    arguments_json JSONB NOT NULL,
    arguments_hash TEXT NOT NULL,
    request_state TEXT NOT NULL UNIQUE,
    risk TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    decided_at TIMESTAMPTZ,
    used_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, id),
    FOREIGN KEY (tenant_id, token_id)
        REFERENCES hpa_tenant.mcp_tokens(tenant_id, id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS hpa_tenant.mcp_audit (
    tenant_id TEXT NOT NULL,
    id TEXT NOT NULL,
    at TIMESTAMPTZ NOT NULL,
    token_id TEXT,
    method TEXT NOT NULL,
    tool_name TEXT,
    status TEXT NOT NULL,
    latency_ms BIGINT NOT NULL CHECK (latency_ms >= 0),
    detail_json JSONB,
    PRIMARY KEY (tenant_id, id)
);

CREATE INDEX IF NOT EXISTS mcp_audit_at
    ON hpa_tenant.mcp_audit (tenant_id, at DESC);

CREATE TABLE IF NOT EXISTS hpa_tenant.mcp_servers (
    tenant_id TEXT NOT NULL,
    id TEXT NOT NULL,
    name TEXT NOT NULL,
    transport TEXT NOT NULL,
    command_json JSONB NOT NULL,
    url TEXT,
    allowed_tools_json JSONB NOT NULL,
    network_json JSONB NOT NULL,
    credential_env TEXT,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    plugin_id TEXT,
    plugin_version TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, id)
);

CREATE TABLE IF NOT EXISTS hpa_tenant.agents (
    tenant_id TEXT NOT NULL,
    id TEXT NOT NULL,
    name TEXT NOT NULL,
    latest_version TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, id)
);

CREATE TABLE IF NOT EXISTS hpa_tenant.agent_versions (
    tenant_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    version TEXT NOT NULL,
    name TEXT NOT NULL,
    description TEXT,
    config_json JSONB NOT NULL,
    content_hash TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    published_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, agent_id, version),
    FOREIGN KEY (tenant_id, agent_id)
        REFERENCES hpa_tenant.agents(tenant_id, id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS hpa_tenant.agent_sessions (
    tenant_id TEXT NOT NULL,
    id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    agent_version TEXT NOT NULL,
    status TEXT NOT NULL,
    input_json JSONB NOT NULL,
    output_json JSONB,
    state_json JSONB NOT NULL,
    error_json JSONB,
    parent_session_id TEXT,
    depth INTEGER NOT NULL CHECK (depth >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, id),
    FOREIGN KEY (tenant_id, agent_id, agent_version)
        REFERENCES hpa_tenant.agent_versions(tenant_id, agent_id, version) ON DELETE RESTRICT
);

CREATE INDEX IF NOT EXISTS agent_sessions_agent
    ON hpa_tenant.agent_sessions (tenant_id, agent_id, created_at DESC);

CREATE TABLE IF NOT EXISTS hpa_tenant.agent_events (
    tenant_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    sequence BIGINT NOT NULL CHECK (sequence > 0),
    at TIMESTAMPTZ NOT NULL,
    kind TEXT NOT NULL,
    data_json JSONB NOT NULL,
    PRIMARY KEY (tenant_id, session_id, sequence),
    FOREIGN KEY (tenant_id, session_id)
        REFERENCES hpa_tenant.agent_sessions(tenant_id, id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS hpa_tenant.agent_approvals (
    tenant_id TEXT NOT NULL,
    id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    tool_name TEXT NOT NULL,
    args_json JSONB NOT NULL,
    risk TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    decided_at TIMESTAMPTZ,
    used_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, id),
    FOREIGN KEY (tenant_id, session_id)
        REFERENCES hpa_tenant.agent_sessions(tenant_id, id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS hpa_tenant.plugin_trusted_keys (
    tenant_id TEXT NOT NULL,
    key_id TEXT NOT NULL,
    public_key TEXT NOT NULL,
    label TEXT NOT NULL,
    trust TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, key_id)
);

CREATE TABLE IF NOT EXISTS hpa_tenant.plugin_packages (
    tenant_id TEXT NOT NULL,
    plugin_id TEXT NOT NULL,
    version TEXT NOT NULL,
    name TEXT NOT NULL,
    description TEXT,
    license TEXT NOT NULL,
    publisher JSONB,
    manifest_json JSONB NOT NULL,
    content_hash TEXT NOT NULL,
    package_path TEXT NOT NULL,
    signature_json JSONB,
    signer_key_id TEXT,
    signer_public_key TEXT,
    trust TEXT NOT NULL,
    source TEXT NOT NULL,
    capabilities_json JSONB NOT NULL,
    sbom_json JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, plugin_id, version)
);

CREATE TABLE IF NOT EXISTS hpa_tenant.plugins (
    tenant_id TEXT NOT NULL,
    id TEXT NOT NULL,
    name TEXT NOT NULL,
    latest_version TEXT NOT NULL,
    status TEXT NOT NULL,
    installed_version TEXT,
    previous_version TEXT,
    trust TEXT NOT NULL,
    license TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, id)
);

CREATE TABLE IF NOT EXISTS hpa_tenant.plugin_capabilities (
    tenant_id TEXT NOT NULL,
    plugin_id TEXT NOT NULL,
    version TEXT NOT NULL,
    kind TEXT NOT NULL,
    capability_id TEXT NOT NULL,
    capability_version TEXT NOT NULL,
    path TEXT NOT NULL,
    status TEXT NOT NULL,
    config_json JSONB NOT NULL,
    permissions_json JSONB NOT NULL,
    PRIMARY KEY (tenant_id, plugin_id, version, kind, capability_id),
    FOREIGN KEY (tenant_id, plugin_id, version)
        REFERENCES hpa_tenant.plugin_packages(tenant_id, plugin_id, version) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS hpa_tenant.plugin_market_listings (
    tenant_id TEXT NOT NULL,
    plugin_id TEXT NOT NULL,
    version TEXT NOT NULL,
    review_status TEXT NOT NULL,
    review_note TEXT,
    submitted_at TIMESTAMPTZ NOT NULL,
    reviewed_at TIMESTAMPTZ,
    withdrawn_at TIMESTAMPTZ,
    downloads BIGINT NOT NULL DEFAULT 0 CHECK (downloads >= 0),
    PRIMARY KEY (tenant_id, plugin_id, version),
    FOREIGN KEY (tenant_id, plugin_id, version)
        REFERENCES hpa_tenant.plugin_packages(tenant_id, plugin_id, version) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS hpa_tenant.plugin_reviews (
    tenant_id TEXT NOT NULL,
    id TEXT NOT NULL,
    plugin_id TEXT NOT NULL,
    version TEXT NOT NULL,
    rating INTEGER NOT NULL CHECK (rating BETWEEN 1 AND 5),
    comment TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, id),
    FOREIGN KEY (tenant_id, plugin_id, version)
        REFERENCES hpa_tenant.plugin_packages(tenant_id, plugin_id, version) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS hpa_tenant.plugin_events (
    tenant_id TEXT NOT NULL,
    id TEXT NOT NULL,
    at TIMESTAMPTZ NOT NULL,
    plugin_id TEXT,
    version TEXT,
    kind TEXT NOT NULL,
    detail_json JSONB NOT NULL,
    PRIMARY KEY (tenant_id, id)
);

CREATE INDEX IF NOT EXISTS plugin_events_at
    ON hpa_tenant.plugin_events (tenant_id, at DESC);

CREATE TABLE IF NOT EXISTS hpa_tenant.connector_definitions (
    tenant_id TEXT NOT NULL,
    connector_id TEXT NOT NULL,
    version TEXT NOT NULL,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    manifest_json JSONB NOT NULL,
    plugin_id TEXT,
    plugin_version TEXT,
    status TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, connector_id, version)
);

CREATE TABLE IF NOT EXISTS hpa_tenant.connections (
    tenant_id TEXT NOT NULL,
    id TEXT NOT NULL,
    name TEXT NOT NULL,
    connector_id TEXT NOT NULL,
    connector_version TEXT NOT NULL,
    config_json JSONB NOT NULL,
    credential_env TEXT,
    status TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, id),
    FOREIGN KEY (tenant_id, connector_id, connector_version)
        REFERENCES hpa_tenant.connector_definitions(tenant_id, connector_id, version) ON DELETE RESTRICT
);

CREATE TABLE IF NOT EXISTS hpa_tenant.connector_audit (
    tenant_id TEXT NOT NULL,
    id TEXT NOT NULL,
    at TIMESTAMPTZ NOT NULL,
    connection_id TEXT NOT NULL,
    operation TEXT NOT NULL,
    status TEXT NOT NULL,
    latency_ms BIGINT NOT NULL CHECK (latency_ms >= 0),
    detail_json JSONB NOT NULL,
    PRIMARY KEY (tenant_id, id)
);

CREATE INDEX IF NOT EXISTS connector_audit_at
    ON hpa_tenant.connector_audit (tenant_id, at DESC);

CREATE TABLE IF NOT EXISTS hpa_tenant.trigger_state (
    tenant_id TEXT NOT NULL,
    workflow_id TEXT NOT NULL,
    trigger_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    config_json JSONB NOT NULL,
    workflow_version TEXT,
    enabled BOOLEAN NOT NULL,
    next_fire_at BIGINT,
    last_fire_at BIGINT,
    last_execution_id TEXT,
    last_status TEXT,
    updated_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, workflow_id, trigger_id),
    FOREIGN KEY (tenant_id, workflow_id)
        REFERENCES hpa_tenant.workflows(tenant_id, id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS trigger_state_due
    ON hpa_tenant.trigger_state (tenant_id, enabled, next_fire_at);

CREATE TABLE IF NOT EXISTS hpa_tenant.trigger_fires (
    tenant_id TEXT NOT NULL,
    id TEXT NOT NULL,
    workflow_id TEXT NOT NULL,
    trigger_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    fired_at TIMESTAMPTZ NOT NULL,
    scheduled_at TIMESTAMPTZ,
    execution_id TEXT,
    status TEXT NOT NULL,
    event_id TEXT,
    detail_json JSONB,
    PRIMARY KEY (tenant_id, id)
);

CREATE UNIQUE INDEX IF NOT EXISTS trigger_fires_event
    ON hpa_tenant.trigger_fires (tenant_id, trigger_id, event_id)
    WHERE event_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS trigger_fires_at
    ON hpa_tenant.trigger_fires (tenant_id, fired_at DESC);

ALTER TABLE hpa_tenant.trigger_fires
    ADD CONSTRAINT trigger_fires_workflow_fk
    FOREIGN KEY (tenant_id, workflow_id)
    REFERENCES hpa_tenant.workflows(tenant_id, id) ON DELETE CASCADE;

ALTER TABLE hpa_tenant.remote_executions
    ADD CONSTRAINT remote_executions_worker_fk
    FOREIGN KEY (tenant_id, worker_id)
    REFERENCES hpa_control.workers(tenant_id, id) ON DELETE RESTRICT;

DO $$
DECLARE
    table_name TEXT;
    tenant_tables TEXT[] := ARRAY[
        'workflow_versions', 'executions', 'remote_executions', 'events',
        'artifacts', 'skills', 'skill_versions', 'skill_files', 'skill_runs',
        'skill_run_artifacts', 'mcp_tokens', 'mcp_approvals', 'mcp_audit',
        'mcp_servers', 'agents', 'agent_versions', 'agent_sessions',
        'agent_events', 'agent_approvals', 'plugin_trusted_keys',
        'plugin_packages', 'plugins', 'plugin_capabilities',
        'plugin_market_listings', 'plugin_reviews', 'plugin_events',
        'connector_definitions', 'connections', 'connector_audit',
        'trigger_state', 'trigger_fires'
    ];
BEGIN
    FOREACH table_name IN ARRAY tenant_tables LOOP
        EXECUTE format(
            'ALTER TABLE hpa_tenant.%I
             ADD CONSTRAINT tenant_fk
             FOREIGN KEY (tenant_id)
             REFERENCES hpa_control.tenants(id) ON DELETE CASCADE',
            table_name
        );
    END LOOP;
END
$$;
