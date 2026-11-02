CREATE UNIQUE INDEX IF NOT EXISTS members_email
    ON hpa_control.members (tenant_id, lower(email))
    WHERE email IS NOT NULL;
