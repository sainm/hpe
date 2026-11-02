ALTER TABLE hpa_control.worker_jobs
    ADD COLUMN fencing_token BIGINT NOT NULL DEFAULT 0
    CHECK (fencing_token >= 0);

CREATE INDEX worker_jobs_reclaim
    ON hpa_control.worker_jobs (tenant_id, created_at)
    WHERE status IN ('queued', 'leased', 'running');

ALTER TABLE hpa_control.worker_jobs OWNER TO hpa_migrator;
