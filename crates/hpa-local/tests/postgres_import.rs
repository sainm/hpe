use std::process::Command;

use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use uuid::Uuid;

#[tokio::test]
async fn sqlite_import_is_tenant_safe_and_idempotent() {
    let Ok(postgres_url) = std::env::var("HPA_POSTGRES_TEST_URL") else {
        eprintln!("skipped: HPA_POSTGRES_TEST_URL is not set");
        return;
    };
    let suffix = Uuid::now_v7().simple().to_string();
    let tenant_id = format!("import_{suffix}");
    let source = std::env::temp_dir().join(format!("hpa-sqlite-import-{suffix}"));
    std::fs::create_dir_all(source.join("tenants")).expect("create import fixture");

    let control = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(source.join("control.db"))
                .create_if_missing(true),
        )
        .await
        .expect("create control fixture");
    for statement in [
        "CREATE TABLE enterprise_tenants (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            status TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
        "CREATE TABLE enterprise_quotas (
            tenant_id TEXT PRIMARY KEY,
            max_workflows INTEGER NOT NULL,
            max_executions_per_day INTEGER NOT NULL,
            max_skills INTEGER NOT NULL,
            max_agents INTEGER NOT NULL,
            max_plugins INTEGER NOT NULL,
            max_connections INTEGER NOT NULL,
            max_concurrent_runs INTEGER NOT NULL
        )",
        "CREATE TABLE enterprise_audit (
            id TEXT PRIMARY KEY,
            at TEXT NOT NULL,
            tenant_id TEXT,
            actor_id TEXT,
            actor_type TEXT NOT NULL,
            action TEXT NOT NULL,
            resource TEXT NOT NULL,
            decision TEXT NOT NULL,
            reason TEXT,
            detail_json TEXT NOT NULL
        )",
        "CREATE TABLE enterprise_members (
            id TEXT PRIMARY KEY,
            tenant_id TEXT NOT NULL,
            subject TEXT NOT NULL,
            user_name TEXT,
            external_id TEXT,
            display_name TEXT NOT NULL,
            email TEXT,
            role TEXT NOT NULL,
            base_role TEXT NOT NULL,
            scim_json TEXT NOT NULL,
            status TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
        "CREATE TABLE enterprise_scim_groups (
            id TEXT PRIMARY KEY,
            tenant_id TEXT NOT NULL,
            display_name TEXT NOT NULL,
            external_id TEXT,
            role TEXT,
            scim_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
        "CREATE TABLE enterprise_scim_group_members (
            tenant_id TEXT NOT NULL,
            group_id TEXT NOT NULL,
            member_id TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (group_id, member_id)
        )",
    ] {
        sqlx::query(statement)
            .execute(&control)
            .await
            .expect("create control fixture table");
    }
    sqlx::query(
        "INSERT INTO enterprise_tenants
             (id, name, status, created_at, updated_at)
         VALUES ($1, 'Imported', 'active',
                 '2026-11-01T00:00:00Z', '2026-11-01T00:00:00Z')",
    )
    .bind(&tenant_id)
    .execute(&control)
    .await
    .expect("insert fixture tenant");
    sqlx::query(
        "INSERT INTO enterprise_quotas
         VALUES ($1, 10, 100, 10, 10, 10, 10, 2)",
    )
    .bind(&tenant_id)
    .execute(&control)
    .await
    .expect("insert fixture quota");
    let audit_id = format!("audit_{suffix}");
    sqlx::query(
        "INSERT INTO enterprise_audit
             (id, at, tenant_id, actor_type, action, resource, decision, detail_json)
         VALUES ($1, '2026-11-01T00:00:01Z', $2, 'test', 'import', 'fixture',
                 'allow', '{\"source\":\"sqlite\"}')",
    )
    .bind(&audit_id)
    .bind(&tenant_id)
    .execute(&control)
    .await
    .expect("insert fixture audit");
    sqlx::query(
        "INSERT INTO enterprise_members
         (id, tenant_id, subject, user_name, external_id, display_name, email,
          role, base_role, scim_json, status, created_at, updated_at)
         VALUES ('member-1', $1, 'oidc|fixture', 'fixture@example.test',
                 'oidc|fixture', 'Fixture User', 'fixture@example.test',
                 'developer', 'auditor', '{\"title\":\"Fixture\"}',
                 'active', '2026-11-01T00:00:00Z', '2026-11-01T00:00:00Z')",
    )
    .bind(&tenant_id)
    .execute(&control)
    .await
    .expect("insert fixture SCIM member");
    sqlx::query(
        "INSERT INTO enterprise_scim_groups
         (id, tenant_id, display_name, external_id, role, scim_json, created_at, updated_at)
         VALUES ('group-1', $1, 'Imported Developers', 'group-fixture',
                 'developer', '{}', '2026-11-01T00:00:00Z', '2026-11-01T00:00:00Z')",
    )
    .bind(&tenant_id)
    .execute(&control)
    .await
    .expect("insert fixture SCIM group");
    sqlx::query(
        "INSERT INTO enterprise_scim_group_members
         (tenant_id, group_id, member_id, created_at)
         VALUES ($1, 'group-1', 'member-1', '2026-11-01T00:00:00Z')",
    )
    .bind(&tenant_id)
    .execute(&control)
    .await
    .expect("insert fixture SCIM group member");
    control.close().await;

    let tenant_path = source.join("tenants").join(&tenant_id);
    std::fs::create_dir_all(&tenant_path).expect("create tenant fixture directory");
    let business = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(tenant_path.join("hpa.db"))
                .create_if_missing(true),
        )
        .await
        .expect("create business fixture");
    for statement in [
        "CREATE TABLE workflows (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            version TEXT NOT NULL,
            description TEXT,
            published INTEGER NOT NULL,
            ir_json TEXT NOT NULL,
            yaml TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
        "CREATE TABLE workflow_versions (
            workflow_id TEXT NOT NULL,
            version TEXT NOT NULL,
            registry_version TEXT NOT NULL,
            ir_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (workflow_id, version)
        )",
        "CREATE TABLE executions (
            id TEXT PRIMARY KEY,
            workflow_id TEXT NOT NULL,
            workflow_version TEXT NOT NULL,
            status TEXT NOT NULL,
            state_json TEXT NOT NULL,
            workflow_json TEXT NOT NULL,
            output_json TEXT,
            error_json TEXT,
            started_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
        "CREATE TABLE events (
            execution_id TEXT NOT NULL,
            sequence INTEGER NOT NULL,
            payload_json TEXT NOT NULL,
            PRIMARY KEY (execution_id, sequence)
        )",
        "CREATE TABLE artifacts (
            id TEXT PRIMARY KEY,
            execution_id TEXT NOT NULL,
            name TEXT NOT NULL,
            kind TEXT NOT NULL,
            mime_type TEXT NOT NULL,
            size INTEGER NOT NULL,
            content_json TEXT NOT NULL,
            created_at TEXT NOT NULL
        )",
        "CREATE TABLE mcp_tokens (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            token_hash TEXT NOT NULL UNIQUE,
            scopes_json TEXT NOT NULL,
            tenant_id TEXT NOT NULL,
            created_at TEXT NOT NULL,
            last_used_at TEXT,
            revoked_at TEXT
        )",
    ] {
        sqlx::query(statement)
            .execute(&business)
            .await
            .expect("create business fixture table");
    }
    sqlx::query(
        "INSERT INTO workflows
         VALUES ('workflow-1', 'Imported workflow', '1.0.0', NULL, 1,
                 '{\"nodes\":[],\"edges\":[]}', 'name: Imported workflow',
                 '2026-11-01T00:00:00Z', '2026-11-01T00:00:00Z')",
    )
    .execute(&business)
    .await
    .expect("insert fixture workflow");
    sqlx::query(
        "INSERT INTO workflow_versions
         VALUES ('workflow-1', '1.0.0', '1',
                 '{\"nodes\":[],\"edges\":[]}', '2026-11-01T00:00:00Z')",
    )
    .execute(&business)
    .await
    .expect("insert fixture workflow version");
    sqlx::query(
        "INSERT INTO executions
         VALUES ('execution-1', 'workflow-1', '1.0.0', 'succeeded',
                 '{\"steps\":[]}', '{\"nodes\":[],\"edges\":[]}',
                 '{\"ok\":true}', NULL,
                 '2026-11-01T00:00:01Z', '2026-11-01T00:00:02Z')",
    )
    .execute(&business)
    .await
    .expect("insert fixture execution");
    sqlx::query(
        "INSERT INTO events
         VALUES ('execution-1', 1, '{\"type\":\"execution.completed\"}')",
    )
    .execute(&business)
    .await
    .expect("insert fixture event");
    sqlx::query(
        "INSERT INTO artifacts
         VALUES ('artifact-1', 'execution-1', 'result.json', 'output',
                 'application/json', 11, '{\"value\":42}',
                 '2026-11-01T00:00:02Z')",
    )
    .execute(&business)
    .await
    .expect("insert fixture artifact");
    sqlx::query(
        "INSERT INTO mcp_tokens
         VALUES ('token-1', 'fixture', 'unique_hash',
                 '[\"mcp:read\"]', 'wrong-tenant',
                 '2026-11-01T00:00:00Z', NULL, NULL)",
    )
    .execute(&business)
    .await
    .expect("insert fixture MCP token");
    business.close().await;

    let first = Command::new(env!("CARGO_BIN_EXE_hpa"))
        .args([
            "db",
            "import-sqlite",
            "--source",
            source.to_str().expect("UTF-8 fixture path"),
        ])
        .env("HPA_POSTGRES_URL", &postgres_url)
        .output()
        .expect("run first import");
    assert!(
        first.status.success(),
        "first import failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first_report: Value =
        serde_json::from_slice(&first.stdout).expect("parse first import report");
    assert_eq!(
        first_report["tables"]["hpa_tenant.workflows"]["inserted"],
        1
    );
    assert_eq!(
        first_report["tables"]["hpa_tenant.executions"]["inserted"],
        1
    );
    assert_eq!(first_report["tables"]["hpa_control.members"]["inserted"], 1);
    assert_eq!(
        first_report["tables"]["hpa_control.scim_groups"]["inserted"],
        1
    );
    assert_eq!(
        first_report["tables"]["hpa_control.scim_group_members"]["inserted"],
        1
    );

    let second = Command::new(env!("CARGO_BIN_EXE_hpa"))
        .args([
            "db",
            "import-sqlite",
            "--source",
            source.to_str().expect("UTF-8 fixture path"),
        ])
        .env("HPA_POSTGRES_URL", &postgres_url)
        .output()
        .expect("run second import");
    assert!(
        second.status.success(),
        "second import failed: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    let second_report: Value =
        serde_json::from_slice(&second.stdout).expect("parse second import report");
    assert_eq!(
        second_report["tables"]["hpa_tenant.workflows"]["inserted"],
        0
    );
    assert_eq!(
        second_report["tables"]["hpa_tenant.executions"]["inserted"],
        0
    );
    assert_eq!(
        second_report["tables"]["hpa_control.scim_group_members"]["inserted"],
        0
    );

    let postgres = PgPoolOptions::new()
        .max_connections(1)
        .connect(&postgres_url)
        .await
        .expect("connect PostgreSQL verifier");
    let (workflows, executions, events, artifacts): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
                 (SELECT count(*) FROM hpa_tenant.workflows WHERE tenant_id = $1),
                 (SELECT count(*) FROM hpa_tenant.executions WHERE tenant_id = $1),
                 (SELECT count(*) FROM hpa_tenant.events WHERE tenant_id = $1),
                 (SELECT count(*) FROM hpa_tenant.artifacts WHERE tenant_id = $1)",
    )
    .bind(&tenant_id)
    .fetch_one(&postgres)
    .await
    .expect("verify imported rows");
    assert_eq!((workflows, executions, events, artifacts), (1, 1, 1, 1));
    let target_tenant: String =
        sqlx::query_scalar("SELECT tenant_id FROM hpa_tenant.mcp_tokens WHERE id = 'token-1'")
            .fetch_one(&postgres)
            .await
            .expect("verify injected tenant");
    assert_eq!(target_tenant, tenant_id);
    let (base_role, group_role): (String, String) = sqlx::query_as(
        "SELECT m.base_role, g.role
         FROM hpa_control.members m
         JOIN hpa_control.scim_group_members gm ON gm.member_id = m.id
         JOIN hpa_control.scim_groups g ON g.id = gm.group_id
         WHERE m.id = 'member-1' AND m.tenant_id = $1",
    )
    .bind(&tenant_id)
    .fetch_one(&postgres)
    .await
    .expect("verify imported SCIM role graph");
    assert_eq!(base_role, "auditor");
    assert_eq!(group_role, "developer");
    let artifact: Value =
        sqlx::query_scalar("SELECT content_json FROM hpa_tenant.artifacts WHERE id = 'artifact-1'")
            .fetch_one(&postgres)
            .await
            .expect("read imported JSON");
    assert_eq!(artifact["value"], 42);

    sqlx::query("DELETE FROM hpa_control.audit WHERE id = $1")
        .bind(&audit_id)
        .execute(&postgres)
        .await
        .expect("clean imported audit");
    sqlx::query("DELETE FROM hpa_control.tenants WHERE id = $1")
        .bind(&tenant_id)
        .execute(&postgres)
        .await
        .expect("clean imported tenant");
    postgres.close().await;
    std::fs::remove_dir_all(source).expect("remove import fixture");
}
