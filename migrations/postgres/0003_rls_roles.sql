DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'hpa_migrator') THEN
        CREATE ROLE hpa_migrator NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOBYPASSRLS;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'hpa_tenant_runtime') THEN
        CREATE ROLE hpa_tenant_runtime NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOBYPASSRLS;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'hpa_worker_runtime') THEN
        CREATE ROLE hpa_worker_runtime NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOBYPASSRLS;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'hpa_platform_admin') THEN
        CREATE ROLE hpa_platform_admin NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOBYPASSRLS;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'hpa_operator') THEN
        CREATE ROLE hpa_operator NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOBYPASSRLS;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'hpa_readonly') THEN
        CREATE ROLE hpa_readonly NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOBYPASSRLS;
    END IF;
END
$$;

ALTER ROLE hpa_migrator NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOBYPASSRLS;
ALTER ROLE hpa_tenant_runtime NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOBYPASSRLS;
ALTER ROLE hpa_worker_runtime NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOBYPASSRLS;
ALTER ROLE hpa_platform_admin NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOBYPASSRLS;
ALTER ROLE hpa_operator NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOBYPASSRLS;
ALTER ROLE hpa_readonly NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOBYPASSRLS;

CREATE OR REPLACE FUNCTION hpa_internal.current_tenant()
RETURNS TEXT
LANGUAGE sql
STABLE
PARALLEL SAFE
SET search_path = pg_catalog
AS $$
    SELECT NULLIF(current_setting('hpa.tenant_id', true), '')
$$;

REVOKE ALL ON SCHEMA hpa_control, hpa_tenant, hpa_internal FROM PUBLIC;
GRANT USAGE ON SCHEMA hpa_control, hpa_tenant, hpa_internal
    TO hpa_tenant_runtime, hpa_worker_runtime, hpa_platform_admin,
       hpa_operator, hpa_readonly;
GRANT USAGE, CREATE ON SCHEMA hpa_control, hpa_tenant, hpa_internal
    TO hpa_migrator;
REVOKE ALL ON FUNCTION hpa_internal.current_tenant() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION hpa_internal.current_tenant()
    TO hpa_tenant_runtime, hpa_worker_runtime, hpa_platform_admin,
       hpa_operator, hpa_readonly;

ALTER TABLE hpa_control.tenants ENABLE ROW LEVEL SECURITY;
ALTER TABLE hpa_control.tenants FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS tenant_isolation ON hpa_control.tenants;
CREATE POLICY tenant_isolation ON hpa_control.tenants
    USING (
        current_user IN ('hpa_platform_admin', 'hpa_operator')
        OR id = hpa_internal.current_tenant()
    )
    WITH CHECK (current_user = 'hpa_platform_admin');

DO $$
DECLARE
    table_name TEXT;
    control_tables TEXT[] := ARRAY[
        'quotas', 'tokens', 'members', 'audit', 'backups', 'workers',
        'worker_tokens', 'worker_jobs', 'sessions', 'auth_flows',
        'oauth_requests', 'oauth_codes', 'oauth_tokens', 'oauth_consents'
    ];
    business_tables TEXT[] := ARRAY[
        'workflows', 'workflow_versions', 'executions', 'remote_executions',
        'events', 'artifacts', 'skills', 'skill_versions', 'skill_files',
        'skill_runs', 'skill_run_artifacts', 'mcp_tokens', 'mcp_approvals',
        'mcp_audit', 'mcp_servers', 'agents', 'agent_versions',
        'agent_sessions', 'agent_events', 'agent_approvals',
        'plugin_trusted_keys', 'plugin_packages', 'plugins',
        'plugin_capabilities', 'plugin_market_listings', 'plugin_reviews',
        'plugin_events', 'connector_definitions', 'connections',
        'connector_audit', 'trigger_state', 'trigger_fires'
    ];
BEGIN
    FOREACH table_name IN ARRAY control_tables LOOP
        EXECUTE format('ALTER TABLE hpa_control.%I ENABLE ROW LEVEL SECURITY', table_name);
        EXECUTE format('ALTER TABLE hpa_control.%I FORCE ROW LEVEL SECURITY', table_name);
        EXECUTE format('DROP POLICY IF EXISTS tenant_isolation ON hpa_control.%I', table_name);
        EXECUTE format(
            'CREATE POLICY tenant_isolation ON hpa_control.%I
             USING (
                 current_user IN (''hpa_platform_admin'', ''hpa_operator'')
                 OR tenant_id = hpa_internal.current_tenant()
             )
             WITH CHECK (
                 current_user = ''hpa_platform_admin''
                 OR tenant_id = hpa_internal.current_tenant()
             )',
            table_name
        );
    END LOOP;

    FOREACH table_name IN ARRAY business_tables LOOP
        EXECUTE format('ALTER TABLE hpa_tenant.%I ENABLE ROW LEVEL SECURITY', table_name);
        EXECUTE format('ALTER TABLE hpa_tenant.%I FORCE ROW LEVEL SECURITY', table_name);
        EXECUTE format('DROP POLICY IF EXISTS tenant_isolation ON hpa_tenant.%I', table_name);
        EXECUTE format(
            'CREATE POLICY tenant_isolation ON hpa_tenant.%I
             USING (
                 current_user IN (''hpa_platform_admin'', ''hpa_operator'')
                 OR tenant_id = hpa_internal.current_tenant()
             )
             WITH CHECK (
                 current_user = ''hpa_platform_admin''
                 OR tenant_id = hpa_internal.current_tenant()
             )',
            table_name
        );
    END LOOP;
END
$$;

ALTER TABLE hpa_control.oauth_clients ENABLE ROW LEVEL SECURITY;
ALTER TABLE hpa_control.oauth_clients FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS tenant_isolation ON hpa_control.oauth_clients;
CREATE POLICY tenant_isolation ON hpa_control.oauth_clients
    USING (
        current_user IN ('hpa_platform_admin', 'hpa_operator')
        OR tenant_id IS NULL
        OR tenant_id = hpa_internal.current_tenant()
    )
    WITH CHECK (
        current_user = 'hpa_platform_admin'
        OR tenant_id = hpa_internal.current_tenant()
    );

GRANT SELECT ON hpa_control.tenants TO hpa_tenant_runtime, hpa_worker_runtime;
GRANT SELECT, INSERT, UPDATE, DELETE ON
    hpa_control.quotas,
    hpa_control.tokens,
    hpa_control.members,
    hpa_control.backups,
    hpa_control.workers,
    hpa_control.worker_tokens,
    hpa_control.worker_jobs,
    hpa_control.sessions,
    hpa_control.auth_flows,
    hpa_control.oauth_requests,
    hpa_control.oauth_codes,
    hpa_control.oauth_tokens,
    hpa_control.oauth_consents,
    hpa_control.oauth_clients
    TO hpa_tenant_runtime;
GRANT SELECT, INSERT ON hpa_control.audit TO hpa_tenant_runtime;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA hpa_tenant
    TO hpa_tenant_runtime;

GRANT SELECT, INSERT, UPDATE ON
    hpa_control.workers,
    hpa_control.worker_jobs
    TO hpa_worker_runtime;
GRANT SELECT, UPDATE ON hpa_control.worker_tokens TO hpa_worker_runtime;
GRANT SELECT ON
    hpa_tenant.workflows,
    hpa_tenant.workflow_versions,
    hpa_tenant.executions
    TO hpa_worker_runtime;
GRANT SELECT, INSERT ON
    hpa_tenant.events,
    hpa_tenant.artifacts
    TO hpa_worker_runtime;

GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA hpa_control, hpa_tenant
    TO hpa_platform_admin;

GRANT SELECT ON
    hpa_control.tenants,
    hpa_control.quotas,
    hpa_control.members,
    hpa_control.audit,
    hpa_control.backups,
    hpa_control.workers,
    hpa_control.worker_jobs,
    hpa_control.oidc_config
    TO hpa_operator, hpa_readonly;
GRANT SELECT (
    id, tenant_id, name, role, platform_admin, created_at,
    last_used_at, expires_at, revoked_at
) ON hpa_control.tokens TO hpa_operator, hpa_readonly;
GRANT SELECT (
    id, tenant_id, name, last_heartbeat_at,
    registered_at, metadata_json
) ON hpa_control.workers TO hpa_operator, hpa_readonly;
GRANT SELECT (
    id, worker_id, tenant_id, created_at, revoked_at
) ON hpa_control.worker_tokens TO hpa_operator, hpa_readonly;
GRANT SELECT (
    id, tenant_id, subject, email, display_name, role, platform_admin,
    created_at, expires_at, revoked_at, last_seen_at
) ON hpa_control.sessions TO hpa_operator, hpa_readonly;
GRANT SELECT (
    id, name, redirect_uris, grant_types, scopes, auth_method, origin,
    tenant_id, created_at, updated_at, disabled_at, last_used_at
) ON hpa_control.oauth_clients TO hpa_operator, hpa_readonly;
GRANT SELECT (
    id, kind, jti, family_id, client_id, tenant_id, subject, role,
    platform_admin, scope, audience, created_at, expires_at, revoked_at,
    revoked_reason, rotated_to, used_at
) ON hpa_control.oauth_tokens TO hpa_operator, hpa_readonly;
GRANT SELECT ON ALL TABLES IN SCHEMA hpa_tenant TO hpa_operator, hpa_readonly;

ALTER DEFAULT PRIVILEGES FOR ROLE hpa_migrator IN SCHEMA hpa_tenant
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO hpa_tenant_runtime;
ALTER DEFAULT PRIVILEGES FOR ROLE hpa_migrator IN SCHEMA hpa_control
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO hpa_platform_admin;
ALTER DEFAULT PRIVILEGES FOR ROLE hpa_migrator IN SCHEMA hpa_tenant
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO hpa_platform_admin;

ALTER SCHEMA hpa_control OWNER TO hpa_migrator;
ALTER SCHEMA hpa_tenant OWNER TO hpa_migrator;
ALTER SCHEMA hpa_internal OWNER TO hpa_migrator;
DO $$
DECLARE
    relation_name TEXT;
BEGIN
    FOR relation_name IN
        SELECT tablename
        FROM pg_tables
        WHERE schemaname IN ('hpa_control', 'hpa_tenant')
    LOOP
        IF EXISTS (
            SELECT 1
            FROM pg_tables
            WHERE schemaname = 'hpa_control' AND tablename = relation_name
        ) THEN
            EXECUTE format(
                'ALTER TABLE hpa_control.%I OWNER TO hpa_migrator',
                relation_name
            );
        ELSE
            EXECUTE format(
                'ALTER TABLE hpa_tenant.%I OWNER TO hpa_migrator',
                relation_name
            );
        END IF;
    END LOOP;
END
$$;
ALTER FUNCTION hpa_internal.current_tenant() OWNER TO hpa_migrator;
