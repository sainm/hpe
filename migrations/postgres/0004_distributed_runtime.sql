DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'hpa_scheduler_runtime') THEN
        CREATE ROLE hpa_scheduler_runtime NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOBYPASSRLS;
    END IF;
END
$$;

ALTER ROLE hpa_scheduler_runtime
    NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOBYPASSRLS;
GRANT USAGE ON SCHEMA hpa_control, hpa_internal TO hpa_scheduler_runtime;

CREATE TABLE hpa_control.runtime_leases (
    lease_key TEXT PRIMARY KEY,
    holder_id TEXT NOT NULL,
    fencing_token BIGINT NOT NULL CHECK (fencing_token > 0),
    acquired_at TIMESTAMPTZ NOT NULL,
    renewed_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    CHECK (expires_at > renewed_at)
);

CREATE INDEX runtime_leases_expiry
    ON hpa_control.runtime_leases (expires_at);

CREATE TABLE hpa_control.outbox (
    tenant_id TEXT NOT NULL REFERENCES hpa_control.tenants(id) ON DELETE CASCADE,
    id TEXT NOT NULL,
    topic TEXT NOT NULL,
    event_type TEXT NOT NULL,
    aggregate_type TEXT NOT NULL,
    aggregate_id TEXT NOT NULL,
    payload_json JSONB NOT NULL,
    headers_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    idempotency_key TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'leased', 'processed', 'dead')),
    available_at TIMESTAMPTZ NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    max_attempts INTEGER NOT NULL DEFAULT 10 CHECK (max_attempts BETWEEN 1 AND 100),
    locked_by TEXT,
    lease_expires_at TIMESTAMPTZ,
    fencing_token BIGINT NOT NULL DEFAULT 0 CHECK (fencing_token >= 0),
    last_error TEXT,
    trace_id TEXT,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    processed_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, id),
    UNIQUE (tenant_id, idempotency_key),
    CHECK (
        (status = 'leased' AND locked_by IS NOT NULL AND lease_expires_at IS NOT NULL)
        OR status <> 'leased'
    )
);

CREATE INDEX outbox_claim
    ON hpa_control.outbox (tenant_id, available_at, created_at)
    WHERE status IN ('pending', 'leased');
CREATE INDEX outbox_aggregate
    ON hpa_control.outbox (tenant_id, aggregate_type, aggregate_id, created_at);

CREATE TABLE hpa_control.idempotency_keys (
    tenant_id TEXT NOT NULL REFERENCES hpa_control.tenants(id) ON DELETE CASCADE,
    scope TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('in_progress', 'succeeded', 'failed')),
    resource_type TEXT,
    resource_id TEXT,
    response_json JSONB,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, scope, idempotency_key)
);

CREATE INDEX idempotency_expiry
    ON hpa_control.idempotency_keys (expires_at)
    WHERE expires_at IS NOT NULL;

ALTER TABLE hpa_control.outbox ENABLE ROW LEVEL SECURITY;
ALTER TABLE hpa_control.outbox FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON hpa_control.outbox
    USING (
        current_user IN ('hpa_platform_admin', 'hpa_operator')
        OR tenant_id = hpa_internal.current_tenant()
    )
    WITH CHECK (
        current_user = 'hpa_platform_admin'
        OR tenant_id = hpa_internal.current_tenant()
    );

ALTER TABLE hpa_control.idempotency_keys ENABLE ROW LEVEL SECURITY;
ALTER TABLE hpa_control.idempotency_keys FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON hpa_control.idempotency_keys
    USING (
        current_user IN ('hpa_platform_admin', 'hpa_operator')
        OR tenant_id = hpa_internal.current_tenant()
    )
    WITH CHECK (
        current_user = 'hpa_platform_admin'
        OR tenant_id = hpa_internal.current_tenant()
    );

GRANT SELECT, INSERT, UPDATE, DELETE
    ON hpa_control.runtime_leases
    TO hpa_scheduler_runtime, hpa_platform_admin;
GRANT SELECT ON hpa_control.runtime_leases
    TO hpa_operator, hpa_readonly;
GRANT SELECT, INSERT, UPDATE, DELETE
    ON hpa_control.outbox, hpa_control.idempotency_keys
    TO hpa_tenant_runtime;
GRANT SELECT ON hpa_control.outbox, hpa_control.idempotency_keys
    TO hpa_operator, hpa_readonly;

CREATE OR REPLACE FUNCTION hpa_internal.claim_outbox(
    p_consumer TEXT,
    p_limit INTEGER DEFAULT 20,
    p_lease_seconds INTEGER DEFAULT 30
)
RETURNS SETOF hpa_control.outbox
LANGUAGE plpgsql
SET search_path = pg_catalog
AS $$
BEGIN
    IF p_consumer IS NULL OR length(trim(p_consumer)) = 0 THEN
        RAISE EXCEPTION 'consumer is required' USING ERRCODE = '22023';
    END IF;
    IF p_limit NOT BETWEEN 1 AND 100 THEN
        RAISE EXCEPTION 'limit must be between 1 and 100' USING ERRCODE = '22023';
    END IF;
    IF p_lease_seconds NOT BETWEEN 1 AND 3600 THEN
        RAISE EXCEPTION 'lease seconds must be between 1 and 3600' USING ERRCODE = '22023';
    END IF;

    RETURN QUERY
    WITH candidate AS (
        SELECT outbox.id
        FROM hpa_control.outbox outbox
        WHERE outbox.status IN ('pending', 'leased')
          AND outbox.available_at <= now()
          AND (
              outbox.status = 'pending'
              OR outbox.lease_expires_at < now()
          )
        ORDER BY outbox.available_at, outbox.created_at, outbox.id
        FOR UPDATE SKIP LOCKED
        LIMIT p_limit
    ),
    claimed AS (
        UPDATE hpa_control.outbox outbox
        SET status = 'leased',
            locked_by = p_consumer,
            lease_expires_at = now() + make_interval(secs => p_lease_seconds),
            fencing_token = outbox.fencing_token + 1,
            attempts = outbox.attempts + 1,
            updated_at = now()
        FROM candidate
        WHERE outbox.id = candidate.id
        RETURNING outbox.*
    )
    SELECT * FROM claimed;
END
$$;

CREATE OR REPLACE FUNCTION hpa_internal.complete_outbox(
    p_id TEXT,
    p_consumer TEXT,
    p_fencing_token BIGINT
)
RETURNS BOOLEAN
LANGUAGE plpgsql
SET search_path = pg_catalog
AS $$
DECLARE
    updated_rows INTEGER;
BEGIN
    UPDATE hpa_control.outbox
    SET status = 'processed',
        locked_by = NULL,
        lease_expires_at = NULL,
        last_error = NULL,
        processed_at = now(),
        updated_at = now()
    WHERE id = p_id
      AND status = 'leased'
      AND locked_by = p_consumer
      AND fencing_token = p_fencing_token;
    GET DIAGNOSTICS updated_rows = ROW_COUNT;
    RETURN updated_rows = 1;
END
$$;

CREATE OR REPLACE FUNCTION hpa_internal.fail_outbox(
    p_id TEXT,
    p_consumer TEXT,
    p_fencing_token BIGINT,
    p_error TEXT,
    p_retry_after_seconds INTEGER DEFAULT 0
)
RETURNS BOOLEAN
LANGUAGE plpgsql
SET search_path = pg_catalog
AS $$
DECLARE
    updated_rows INTEGER;
BEGIN
    IF p_retry_after_seconds NOT BETWEEN 0 AND 86400 THEN
        RAISE EXCEPTION 'retry after seconds must be between 0 and 86400'
            USING ERRCODE = '22023';
    END IF;
    UPDATE hpa_control.outbox
    SET status = CASE WHEN attempts >= max_attempts THEN 'dead' ELSE 'pending' END,
        available_at = now() + make_interval(secs => p_retry_after_seconds),
        locked_by = NULL,
        lease_expires_at = NULL,
        last_error = left(p_error, 4096),
        updated_at = now()
    WHERE id = p_id
      AND status = 'leased'
      AND locked_by = p_consumer
      AND fencing_token = p_fencing_token;
    GET DIAGNOSTICS updated_rows = ROW_COUNT;
    RETURN updated_rows = 1;
END
$$;

REVOKE ALL ON FUNCTION hpa_internal.claim_outbox(TEXT, INTEGER, INTEGER) FROM PUBLIC;
REVOKE ALL ON FUNCTION hpa_internal.complete_outbox(TEXT, TEXT, BIGINT) FROM PUBLIC;
REVOKE ALL ON FUNCTION hpa_internal.fail_outbox(TEXT, TEXT, BIGINT, TEXT, INTEGER)
    FROM PUBLIC;
GRANT EXECUTE ON FUNCTION hpa_internal.claim_outbox(TEXT, INTEGER, INTEGER)
    TO hpa_tenant_runtime, hpa_platform_admin;
GRANT EXECUTE ON FUNCTION hpa_internal.complete_outbox(TEXT, TEXT, BIGINT)
    TO hpa_tenant_runtime, hpa_platform_admin;
GRANT EXECUTE ON FUNCTION hpa_internal.fail_outbox(TEXT, TEXT, BIGINT, TEXT, INTEGER)
    TO hpa_tenant_runtime, hpa_platform_admin;

ALTER TABLE hpa_control.runtime_leases OWNER TO hpa_migrator;
ALTER TABLE hpa_control.outbox OWNER TO hpa_migrator;
ALTER TABLE hpa_control.idempotency_keys OWNER TO hpa_migrator;
ALTER FUNCTION hpa_internal.claim_outbox(TEXT, INTEGER, INTEGER) OWNER TO hpa_migrator;
ALTER FUNCTION hpa_internal.complete_outbox(TEXT, TEXT, BIGINT) OWNER TO hpa_migrator;
ALTER FUNCTION hpa_internal.fail_outbox(TEXT, TEXT, BIGINT, TEXT, INTEGER)
    OWNER TO hpa_migrator;

ALTER DEFAULT PRIVILEGES FOR ROLE hpa_migrator IN SCHEMA hpa_control
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO hpa_platform_admin;
