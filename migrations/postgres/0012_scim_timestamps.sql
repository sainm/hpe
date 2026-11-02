ALTER TABLE hpa_control.scim_groups
    ALTER COLUMN created_at TYPE TIMESTAMPTZ
        USING to_timestamp(created_at / 1000.0),
    ALTER COLUMN updated_at TYPE TIMESTAMPTZ
        USING to_timestamp(updated_at / 1000.0);

ALTER TABLE hpa_control.scim_group_members
    ALTER COLUMN created_at TYPE TIMESTAMPTZ
        USING to_timestamp(created_at / 1000.0);
