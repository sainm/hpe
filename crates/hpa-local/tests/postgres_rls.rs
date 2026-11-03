use std::borrow::Cow;
use std::fs;

use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations/postgres");

async fn tenant_transaction<'a>(
    pool: &'a PgPool,
    role: &str,
    tenant_id: &str,
) -> sqlx::Result<Transaction<'a, Postgres>> {
    let role_sql = match role {
        "hpa_tenant_runtime" => "SET LOCAL ROLE hpa_tenant_runtime",
        "hpa_worker_runtime" => "SET LOCAL ROLE hpa_worker_runtime",
        "hpa_platform_admin" => "SET LOCAL ROLE hpa_platform_admin",
        "hpa_operator" => "SET LOCAL ROLE hpa_operator",
        "hpa_readonly" => "SET LOCAL ROLE hpa_readonly",
        _ => panic!("unexpected test role"),
    };
    let mut transaction = pool.begin().await?;
    sqlx::query(role_sql).execute(&mut *transaction).await?;
    sqlx::query("SELECT set_config('hpa.tenant_id', $1, true)")
        .bind(tenant_id)
        .execute(&mut *transaction)
        .await?;
    Ok(transaction)
}

fn is_insufficient_privilege(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|database| database.code())
        .is_some_and(|code| code == Cow::Borrowed("42501"))
}

#[tokio::test]
async fn postgres_migrations_enforce_forced_rls_and_role_boundaries() {
    let Ok(database_url) = std::env::var("HPA_POSTGRES_TEST_URL") else {
        eprintln!("skipped: HPA_POSTGRES_TEST_URL is not set");
        return;
    };
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .expect("connect to PostgreSQL");
    MIGRATOR
        .run(&pool)
        .await
        .expect("apply PostgreSQL migrations");

    let suffix = Uuid::now_v7().simple().to_string();
    let alpha = format!("alpha_{suffix}");
    let beta = format!("beta_{suffix}");
    let alpha_workflow = format!("workflow_{suffix}_alpha");
    let beta_workflow = format!("workflow_{suffix}_beta");

    for (tenant_id, name) in [(&alpha, "RLS Alpha"), (&beta, "RLS Beta")] {
        sqlx::query("INSERT INTO hpa_control.tenants (id, name) VALUES ($1, $2)")
            .bind(tenant_id)
            .bind(name)
            .execute(&pool)
            .await
            .expect("insert test tenant");
    }
    for (tenant_id, workflow_id, name) in [
        (&alpha, &alpha_workflow, "Alpha workflow"),
        (&beta, &beta_workflow, "Beta workflow"),
    ] {
        sqlx::query(
            "INSERT INTO hpa_tenant.workflows
                 (tenant_id, id, name, version, ir_json, yaml)
             VALUES ($1, $2, $3, '1.0.0', '{}'::jsonb, $4)",
        )
        .bind(tenant_id)
        .bind(workflow_id)
        .bind(name)
        .bind(format!("name: {name}"))
        .execute(&pool)
        .await
        .expect("insert test workflow");
    }

    let rls_tables = sqlx::query(
        "SELECT n.nspname, c.relname, c.relrowsecurity, c.relforcerowsecurity,
                count(p.oid)
         FROM pg_class c
         JOIN pg_namespace n ON n.oid = c.relnamespace
         LEFT JOIN pg_policy p ON p.polrelid = c.oid
         WHERE c.relkind = 'r'
           AND (
               n.nspname = 'hpa_tenant'
               OR (
                   n.nspname = 'hpa_control'
                   AND (
                       c.relname = 'tenants'
                       OR EXISTS (
                           SELECT 1
                           FROM information_schema.columns column_info
                           WHERE column_info.table_schema = n.nspname
                             AND column_info.table_name = c.relname
                             AND column_info.column_name = 'tenant_id'
                       )
                   )
               )
           )
         GROUP BY n.nspname, c.relname, c.relrowsecurity, c.relforcerowsecurity
         ORDER BY n.nspname, c.relname",
    )
    .fetch_all(&pool)
    .await
    .expect("inspect RLS metadata");
    assert_eq!(rls_tables.len(), 68, "unexpected tenant table count");
    for row in rls_tables {
        let schema: String = row.get("nspname");
        let table: String = row.get("relname");
        assert!(
            row.get::<bool, _>("relrowsecurity"),
            "{schema}.{table} RLS disabled"
        );
        assert!(
            row.get::<bool, _>("relforcerowsecurity"),
            "{schema}.{table} FORCE RLS disabled"
        );
        assert!(
            row.get::<i64, _>("count") > 0,
            "{schema}.{table} has no RLS policy"
        );
    }

    let tenant_root_foreign_keys: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT conrelid)
         FROM pg_constraint
         WHERE contype = 'f'
           AND confrelid = 'hpa_control.tenants'::regclass
           AND connamespace = 'hpa_tenant'::regnamespace",
    )
    .fetch_one(&pool)
    .await
    .expect("inspect tenant foreign keys");
    assert_eq!(
        tenant_root_foreign_keys, 38,
        "every hpa_tenant table must reference hpa_control.tenants"
    );

    let role_attributes = sqlx::query(
        "SELECT rolname, rolsuper, rolbypassrls
         FROM pg_roles
         WHERE rolname = ANY($1)",
    )
    .bind(vec![
        "hpa_tenant_runtime".to_string(),
        "hpa_worker_runtime".to_string(),
        "hpa_platform_admin".to_string(),
        "hpa_operator".to_string(),
        "hpa_readonly".to_string(),
    ])
    .fetch_all(&pool)
    .await
    .expect("inspect runtime roles");
    assert_eq!(role_attributes.len(), 5);
    for row in role_attributes {
        assert!(!row.get::<bool, _>("rolsuper"));
        assert!(!row.get::<bool, _>("rolbypassrls"));
    }

    let role_memberships: i64 = sqlx::query_scalar(
        "SELECT count(*)
         FROM pg_auth_members membership
         JOIN pg_roles granted ON granted.oid = membership.roleid
         JOIN pg_roles member ON member.oid = membership.member
         WHERE granted.rolname IN (
             'hpa_platform_admin', 'hpa_operator', 'hpa_readonly'
         )
           AND member.rolname IN (
               'hpa_tenant_runtime', 'hpa_worker_runtime'
           )",
    )
    .fetch_one(&pool)
    .await
    .expect("inspect privileged role memberships");
    assert_eq!(
        role_memberships, 0,
        "runtime roles must not inherit privileged roles"
    );

    let mut alpha_tx = tenant_transaction(&pool, "hpa_tenant_runtime", &alpha)
        .await
        .expect("start alpha transaction");
    let visible_alpha: i64 =
        sqlx::query_scalar("SELECT count(*) FROM hpa_tenant.workflows WHERE tenant_id = $1")
            .bind(&alpha)
            .fetch_one(&mut *alpha_tx)
            .await
            .expect("read alpha workflow");
    let visible_beta: i64 =
        sqlx::query_scalar("SELECT count(*) FROM hpa_tenant.workflows WHERE tenant_id = $1")
            .bind(&beta)
            .fetch_one(&mut *alpha_tx)
            .await
            .expect("probe beta workflow");
    let guessed_beta: i64 =
        sqlx::query_scalar("SELECT count(*) FROM hpa_tenant.workflows WHERE id = $1")
            .bind(&beta_workflow)
            .fetch_one(&mut *alpha_tx)
            .await
            .expect("probe beta workflow ID");
    let updated_beta = sqlx::query("UPDATE hpa_tenant.workflows SET name = 'forged' WHERE id = $1")
        .bind(&beta_workflow)
        .execute(&mut *alpha_tx)
        .await
        .expect("probe beta update");
    let deleted_beta = sqlx::query("DELETE FROM hpa_tenant.workflows WHERE id = $1")
        .bind(&beta_workflow)
        .execute(&mut *alpha_tx)
        .await
        .expect("probe beta delete");
    for (label, statement) in [
        ("tokens", "SELECT count(*) FROM hpa_control.tokens"),
        ("workers", "SELECT count(*) FROM hpa_control.workers"),
        (
            "worker jobs",
            "SELECT count(*) FROM hpa_control.worker_jobs",
        ),
        ("quotas", "SELECT count(*) FROM hpa_control.quotas"),
    ] {
        sqlx::query(statement)
            .fetch_one(&mut *alpha_tx)
            .await
            .unwrap_or_else(|error| panic!("tenant runtime lacks access to {label}: {error}"));
    }
    assert_eq!(visible_alpha, 1);
    assert_eq!(visible_beta, 0);
    assert_eq!(guessed_beta, 0);
    assert_eq!(updated_beta.rows_affected(), 0);
    assert_eq!(deleted_beta.rows_affected(), 0);

    sqlx::query("SELECT set_config('hpa.tenant_id', $1, true)")
        .bind(&beta)
        .execute(&mut *alpha_tx)
        .await
        .expect("switch tenant in pooled transaction");
    let switched_beta: i64 =
        sqlx::query_scalar("SELECT count(*) FROM hpa_tenant.workflows WHERE tenant_id = $1")
            .bind(&beta)
            .fetch_one(&mut *alpha_tx)
            .await
            .expect("read beta after tenant switch");
    assert_eq!(switched_beta, 1);
    alpha_tx.rollback().await.expect("rollback isolation probe");

    let mut cross_insert = tenant_transaction(&pool, "hpa_tenant_runtime", &alpha)
        .await
        .expect("start cross-insert transaction");
    let cross_insert_error = sqlx::query(
        "INSERT INTO hpa_tenant.workflows
             (tenant_id, id, name, version, ir_json, yaml)
         VALUES ($1, $2, 'forged', '1.0.0', '{}'::jsonb, 'name: forged')",
    )
    .bind(&beta)
    .bind(format!("forged_{suffix}"))
    .execute(&mut *cross_insert)
    .await
    .expect_err("cross-tenant insert must fail");
    assert!(is_insufficient_privilege(&cross_insert_error));
    cross_insert
        .rollback()
        .await
        .expect("rollback cross insert");

    let mut default_deny = tenant_transaction(&pool, "hpa_tenant_runtime", "")
        .await
        .expect("start default-deny transaction");
    let default_visible: i64 = sqlx::query_scalar("SELECT count(*) FROM hpa_tenant.workflows")
        .fetch_one(&mut *default_deny)
        .await
        .expect("read without tenant context");
    assert_eq!(default_visible, 0);
    default_deny
        .rollback()
        .await
        .expect("rollback default deny");

    let mut readonly = tenant_transaction(&pool, "hpa_readonly", &alpha)
        .await
        .expect("start readonly transaction");
    let readonly_visible: i64 = sqlx::query_scalar("SELECT count(*) FROM hpa_tenant.workflows")
        .fetch_one(&mut *readonly)
        .await
        .expect("readonly workflow read");
    assert_eq!(readonly_visible, 1);
    let readonly_error = sqlx::query(
        "INSERT INTO hpa_tenant.workflows
             (tenant_id, id, name, version, ir_json, yaml)
         VALUES ($1, $2, 'readonly', '1.0.0', '{}'::jsonb, 'name: readonly')",
    )
    .bind(&alpha)
    .bind(format!("readonly_{suffix}"))
    .execute(&mut *readonly)
    .await
    .expect_err("readonly role must not write");
    assert!(is_insufficient_privilege(&readonly_error));
    readonly.rollback().await.expect("rollback readonly probe");

    let mut secret_read = tenant_transaction(&pool, "hpa_readonly", &alpha)
        .await
        .expect("start secret-read transaction");
    let secret_error = sqlx::query("SELECT * FROM hpa_control.oauth_signing_keys")
        .fetch_all(&mut *secret_read)
        .await
        .expect_err("readonly role must not read OAuth signing secrets");
    assert!(is_insufficient_privilege(&secret_error));
    secret_read.rollback().await.expect("rollback secret probe");

    let mut platform = tenant_transaction(&pool, "hpa_platform_admin", "")
        .await
        .expect("start platform transaction");
    let platform_visible: i64 =
        sqlx::query_scalar("SELECT count(*) FROM hpa_tenant.workflows WHERE tenant_id = ANY($1)")
            .bind(vec![alpha.clone(), beta.clone()])
            .fetch_one(&mut *platform)
            .await
            .expect("platform admin cross-tenant read");
    assert_eq!(platform_visible, 2);
    platform.rollback().await.expect("rollback platform probe");

    sqlx::query("DELETE FROM hpa_control.tenants WHERE id = ANY($1)")
        .bind(vec![alpha, beta])
        .execute(&pool)
        .await
        .expect("clean up test tenants");
    pool.close().await;
}

#[tokio::test]
async fn postgres_migrator_rejects_modified_history() {
    let Ok(database_url) = std::env::var("HPA_POSTGRES_TEST_URL") else {
        eprintln!("skipped: HPA_POSTGRES_TEST_URL is not set");
        return;
    };
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("connect to PostgreSQL");
    let suffix = Uuid::now_v7().simple().to_string();
    let directory = std::env::temp_dir().join(format!("hpa-migration-{suffix}"));
    fs::create_dir_all(&directory).expect("create temporary migration directory");
    let migration_path = directory.join("0001_checksum.sql");
    let probe_table = format!("hpa_checksum_probe_{suffix}");
    let migration_table = format!("_hpa_checksum_migrations_{suffix}");
    fs::write(
        &migration_path,
        format!("CREATE TABLE public.{probe_table} (id BIGINT PRIMARY KEY);\n"),
    )
    .expect("write initial migration");

    let mut initial = sqlx::migrate::Migrator::new(directory.as_path())
        .await
        .expect("load initial migration");
    initial.dangerous_set_table_name(format!("public.{migration_table}"));
    initial.run(&pool).await.expect("apply initial migration");

    fs::write(
        &migration_path,
        format!(
            "CREATE TABLE public.{probe_table} (id BIGINT PRIMARY KEY);\n-- modified after apply\n"
        ),
    )
    .expect("modify migration");
    let mut modified = sqlx::migrate::Migrator::new(directory.as_path())
        .await
        .expect("reload modified migration");
    modified.dangerous_set_table_name(format!("public.{migration_table}"));
    let error = modified
        .run(&pool)
        .await
        .expect_err("modified migration history must be rejected");
    assert!(
        matches!(error, sqlx::migrate::MigrateError::VersionMismatch(1)),
        "unexpected migration error: {error}"
    );

    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP TABLE public.{probe_table}"
    )))
    .execute(&pool)
    .await
    .expect("drop checksum probe table");
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP TABLE public.{migration_table}"
    )))
    .execute(&pool)
    .await
    .expect("drop checksum migration table");
    pool.close().await;
    fs::remove_dir_all(directory).expect("remove temporary migration directory");
}
