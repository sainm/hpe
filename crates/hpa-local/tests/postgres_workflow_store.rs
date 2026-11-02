use hpa_local::postgres_execution::{
    PostgresExecutionError, PostgresExecutionStore, WorkflowRecord,
};
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations/postgres");

fn workflow_record(id: &str, version: &str, published: i64) -> WorkflowRecord {
    let mut workflow: Value =
        serde_json::from_str(include_str!("../../../fixtures/01_sequence.json"))
            .expect("sequence workflow");
    workflow["id"] = Value::String(id.to_string());
    workflow["name"] = Value::String(format!("Workflow {id}"));
    workflow["workflowVersion"] = Value::String(version.to_string());
    WorkflowRecord {
        id: id.to_string(),
        name: format!("Workflow {id}"),
        version: version.to_string(),
        description: Some("PostgreSQL workflow store test".to_string()),
        published,
        ir_json: serde_json::to_string(&workflow).unwrap(),
        yaml: "id: workflow\n".to_string(),
        created_at: "2026-11-02T00:00:00Z".to_string(),
        updated_at: "2026-11-02T00:00:01Z".to_string(),
    }
}

#[tokio::test]
async fn postgres_workflow_crud_and_frozen_versions_are_tenant_scoped() {
    let Ok(database_url) = std::env::var("HPA_POSTGRES_TEST_URL") else {
        eprintln!("skipped: HPA_POSTGRES_TEST_URL is not set");
        return;
    };
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .expect("connect PostgreSQL");
    MIGRATOR.run(&pool).await.expect("apply migrations");
    let suffix = Uuid::now_v7().simple().to_string();
    let tenant_id = format!("workflow_{suffix}");
    sqlx::query("INSERT INTO hpa_control.tenants (id, name) VALUES ($1, $1)")
        .bind(&tenant_id)
        .execute(&pool)
        .await
        .expect("insert workflow tenant");

    let workflow_id = format!("wf_pg_{suffix}");
    let store = PostgresExecutionStore::new(pool.clone(), &tenant_id);
    let other = PostgresExecutionStore::new(pool.clone(), "other-workflow-tenant");
    let mut draft = workflow_record(&workflow_id, "1.0.0", 0);
    store.upsert_workflow(&draft).await.expect("upsert draft");
    let loaded = store
        .get_workflow(&workflow_id)
        .await
        .expect("get draft")
        .expect("draft exists");
    assert_eq!(loaded.published, 0);
    assert_eq!(store.workflow_count().await.unwrap(), 1);
    assert!(
        other
            .list_workflows()
            .await
            .expect("other tenant workflows")
            .is_empty()
    );

    draft.published = 1;
    draft.updated_at = "2026-11-02T00:00:02Z".to_string();
    store
        .publish_workflow(&draft, "registry-workflow")
        .await
        .expect("publish v1");
    let versions = store
        .list_versions(&workflow_id)
        .await
        .expect("v1 versions");
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].registry_version, "registry-workflow");
    store
        .publish_workflow(&draft, "registry-workflow")
        .await
        .expect("idempotent republish");

    let mut conflicting = draft.clone();
    let mut ir: Value = serde_json::from_str(&conflicting.ir_json).unwrap();
    ir["name"] = json!("Changed");
    conflicting.ir_json = serde_json::to_string(&ir).unwrap();
    assert!(matches!(
        store
            .publish_workflow(&conflicting, "registry-workflow")
            .await
            .expect_err("frozen version conflict"),
        PostgresExecutionError::WorkflowSnapshotConflict { .. }
    ));

    let mut second = workflow_record(&workflow_id, "2.0.0", 1);
    second.updated_at = "2026-11-02T00:00:03Z".to_string();
    store
        .publish_workflow(&second, "registry-workflow-v2")
        .await
        .expect("publish v2");
    assert_eq!(store.list_versions(&workflow_id).await.unwrap().len(), 2);
    let current = store
        .get_workflow(&workflow_id)
        .await
        .unwrap()
        .expect("current workflow");
    assert_eq!(current.version, "2.0.0");
    assert_eq!(current.published, 1);

    sqlx::query("DELETE FROM hpa_control.tenants WHERE id = $1")
        .bind(&tenant_id)
        .execute(&pool)
        .await
        .expect("clean workflow tenant");
    pool.close().await;
}
