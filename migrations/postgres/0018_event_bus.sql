CREATE OR REPLACE FUNCTION hpa_internal.claim_outbox_topic(
    p_consumer TEXT,
    p_topic TEXT,
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
    IF p_topic IS NULL OR length(trim(p_topic)) = 0 THEN
        RAISE EXCEPTION 'topic is required' USING ERRCODE = '22023';
    END IF;
    IF p_limit NOT BETWEEN 1 AND 100 THEN
        RAISE EXCEPTION 'limit must be between 1 and 100' USING ERRCODE = '22023';
    END IF;
    IF p_lease_seconds NOT BETWEEN 1 AND 3600 THEN
        RAISE EXCEPTION 'lease seconds must be between 1 and 3600'
            USING ERRCODE = '22023';
    END IF;

    RETURN QUERY
    WITH candidate AS (
        SELECT outbox.id
        FROM hpa_control.outbox outbox
        WHERE outbox.topic = p_topic
          AND outbox.status IN ('pending', 'leased')
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

REVOKE ALL ON FUNCTION hpa_internal.claim_outbox_topic(TEXT, TEXT, INTEGER, INTEGER)
    FROM PUBLIC;
GRANT EXECUTE ON FUNCTION hpa_internal.claim_outbox_topic(TEXT, TEXT, INTEGER, INTEGER)
    TO hpa_tenant_runtime, hpa_platform_admin;
ALTER FUNCTION hpa_internal.claim_outbox_topic(TEXT, TEXT, INTEGER, INTEGER)
    OWNER TO hpa_migrator;
