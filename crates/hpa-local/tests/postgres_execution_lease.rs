use hpa_core::compiler::compile_workflow;
use hpa_core::ir::Workflow;
use hpa_core::runtime::{new_state, start_event};
use hpa_local::postgres_execution::{ExecutionRecord, PostgresExecutionStore};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations/postgres");

#[tokio::test]
async fn execution_lease_prevents_stale_owners_from_committing() {
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
    let tenant_id = format!("lease_{suffix}");
    sqlx::query("INSERT INTO hpa_control.tenants (id, name) VALUES ($1, $1)")
        .bind(&tenant_id)
        .execute(&pool)
        .await
        .expect("insert lease tenant");

    let workflow: Workflow =
        serde_json::from_str(include_str!("../../../fixtures/01_sequence.json"))
            .expect("sequence workflow");
    let compilation = compile_workflow(&workflow).expect("compile workflow");
    let store_a = PostgresExecutionStore::new(pool.clone(), &tenant_id);
    let store_b = PostgresExecutionStore::new(pool.clone(), &tenant_id);
    store_a
        .sync_workflow(
            &workflow.to_json_string(false).unwrap(),
            &workflow.workflow_version,
            "registry-lease",
            "2026-11-02T00:00:00Z",
            "2026-11-02T00:00:01Z",
        )
        .await
        .expect("sync lease workflow");
    let mut state = new_state(
        &workflow,
        &compilation.plan,
        format!("exec_lease_{suffix}"),
        json!({"value": 3}),
        Default::default(),
        tenant_id.clone(),
    )
    .expect("lease state");
    let start = start_event(&mut state);
    let record = ExecutionRecord {
        id: state.id.clone(),
        workflow_id: workflow.id.clone(),
        workflow_version: workflow.workflow_version.clone(),
        status: "running".to_string(),
        state_json: serde_json::to_string(&state).unwrap(),
        workflow_json: workflow.to_json_string(false).unwrap(),
        output_json: None,
        error_json: None,
        started_at: state.started_at.clone(),
        updated_at: state.updated_at.clone(),
    };
    store_a
        .insert_execution(&record, &start)
        .await
        .expect("insert lease execution");

    let first = store_a
        .acquire_execution_lease(&state.id, "node-a", 1)
        .await
        .expect("acquire first lease")
        .expect("node-a owns lease");
    assert!(
        store_b
            .acquire_execution_lease(&state.id, "node-b", 30)
            .await
            .expect("probe node-b")
            .is_none()
    );
    store_a
        .save_execution_fenced(&state, &[], "node-a", first.fencing_token)
        .await
        .expect("save under first lease");

    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    let second = store_b
        .acquire_execution_lease(&state.id, "node-b", 30)
        .await
        .expect("take over expired lease")
        .expect("node-b takes over");
    assert!(second.fencing_token > first.fencing_token);
    assert!(
        store_a
            .save_execution_fenced(&state, &[], "node-a", first.fencing_token)
            .await
            .is_err()
    );
    store_b
        .save_execution_fenced(&state, &[], "node-b", second.fencing_token)
        .await
        .expect("save under new lease");
    assert!(
        !store_a
            .release_execution_lease(&state.id, "node-a", first.fencing_token)
            .await
            .expect("reject stale release")
    );
    assert!(
        store_b
            .release_execution_lease(&state.id, "node-b", second.fencing_token)
            .await
            .expect("release current lease")
    );

    sqlx::query("DELETE FROM hpa_control.tenants WHERE id = $1")
        .bind(&tenant_id)
        .execute(&pool)
        .await
        .expect("clean lease tenant");
    pool.close().await;
}
