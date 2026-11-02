ALTER TABLE hpa_control.members
    ADD COLUMN IF NOT EXISTS user_name TEXT,
    ADD COLUMN IF NOT EXISTS external_id TEXT,
    ADD COLUMN IF NOT EXISTS base_role TEXT NOT NULL DEFAULT 'viewer',
    ADD COLUMN IF NOT EXISTS scim_json JSONB NOT NULL DEFAULT '{}'::jsonb;

UPDATE hpa_control.members
SET base_role = role
WHERE base_role IS NULL OR base_role = '';

CREATE UNIQUE INDEX IF NOT EXISTS members_user_name
    ON hpa_control.members (tenant_id, lower(user_name))
    WHERE user_name IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS members_external_id
    ON hpa_control.members (tenant_id, external_id)
    WHERE external_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS members_tenant_id
    ON hpa_control.members (tenant_id, id);

CREATE TABLE IF NOT EXISTS hpa_control.scim_groups (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES hpa_control.tenants(id) ON DELETE CASCADE,
    display_name TEXT NOT NULL,
    external_id TEXT,
    role TEXT CHECK (
        role IS NULL OR role IN (
            'owner', 'admin', 'developer', 'publisher',
            'operator', 'approver', 'auditor', 'viewer'
        )
    ),
    scim_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS scim_groups_display_name
    ON hpa_control.scim_groups (tenant_id, lower(display_name));
CREATE UNIQUE INDEX IF NOT EXISTS scim_groups_external_id
    ON hpa_control.scim_groups (tenant_id, external_id)
    WHERE external_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS scim_groups_tenant_id
    ON hpa_control.scim_groups (tenant_id, id);

CREATE TABLE IF NOT EXISTS hpa_control.scim_group_members (
    tenant_id TEXT NOT NULL REFERENCES hpa_control.tenants(id) ON DELETE CASCADE,
    group_id TEXT NOT NULL,
    member_id TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    PRIMARY KEY (group_id, member_id),
    FOREIGN KEY (tenant_id, group_id)
        REFERENCES hpa_control.scim_groups(tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, member_id)
        REFERENCES hpa_control.members(tenant_id, id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS scim_group_members_member
    ON hpa_control.scim_group_members (tenant_id, member_id);

ALTER TABLE hpa_control.scim_groups ENABLE ROW LEVEL SECURITY;
ALTER TABLE hpa_control.scim_groups FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS tenant_isolation ON hpa_control.scim_groups;
CREATE POLICY tenant_isolation ON hpa_control.scim_groups
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

ALTER TABLE hpa_control.scim_group_members ENABLE ROW LEVEL SECURITY;
ALTER TABLE hpa_control.scim_group_members FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS tenant_isolation ON hpa_control.scim_group_members;
CREATE POLICY tenant_isolation ON hpa_control.scim_group_members
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

GRANT SELECT, INSERT, UPDATE, DELETE ON
    hpa_control.scim_groups,
    hpa_control.scim_group_members
    TO hpa_tenant_runtime, hpa_platform_admin;
GRANT SELECT ON
    hpa_control.scim_groups,
    hpa_control.scim_group_members
    TO hpa_operator, hpa_readonly;

ALTER TABLE hpa_control.scim_groups OWNER TO hpa_migrator;
ALTER TABLE hpa_control.scim_group_members OWNER TO hpa_migrator;
