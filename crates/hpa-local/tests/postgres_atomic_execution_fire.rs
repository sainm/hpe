use hpa_core::compiler::compile_workflow;
use hpa_core::ir::Workflow;
use hpa_core::runtime::{new_state, start_event};
use hpa_local::postgres_execution::{ExecutionRecord, PostgresExecutionStore};
use hpa_local::postgres_trigger::TriggerFireRecord;
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations/postgres");

#[tokio::test]
async fn duplicate_trigger_fire_rolls_back_the_second_execution() {
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
    let tenant_id = format!("atomic_{suffix}");
    sqlx::query("INSERT INTO hpa_control.tenants (id, name) VALUES ($1, $1)")
        .bind(&tenant_id)
        .execute(&pool)
        .await
        .expect("insert atomic tenant");

    let workflow: Workflow =
        serde_json::from_str(include_str!("../../../fixtures/01_sequence.json"))
            .expect("sequence workflow");
    let compilation = compile_workflow(&workflow).expect("compile workflow");
    let store = PostgresExecutionStore::new(pool.clone(), &tenant_id);
    store
        .sync_workflow(
            &workflow.to_json_string(false).unwrap(),
            &workflow.workflow_version,
            "registry-atomic",
            "2026-11-02T00:00:00Z",
            "2026-11-02T00:00:01Z",
        )
        .await
        .expect("sync workflow");

    let mut first = new_state(
        &workflow,
        &compilation.plan,
        format!("exec_first_{suffix}"),
        json!({"value": 1}),
        Default::default(),
        tenant_id.clone(),
    )
    .expect("first state");
    let first_event = start_event(&mut first);
    let first_record = ExecutionRecord {
        id: first.id.clone(),
        workflow_id: workflow.id.clone(),
        workflow_version: workflow.workflow_version.clone(),
        status: "running".to_string(),
        state_json: serde_json::to_string(&first).unwrap(),
        workflow_json: workflow.to_json_string(false).unwrap(),
        output_json: None,
        error_json: None,
        started_at: first.started_at.clone(),
        updated_at: first.updated_at.clone(),
    };
    let first_fire = TriggerFireRecord {
        id: format!("fire_first_{suffix}"),
        workflow_id: workflow.id.clone(),
        trigger_id: "event".to_string(),
        kind: "event".to_string(),
        fired_at: "2026-11-02T00:00:02Z".to_string(),
        scheduled_at: None,
        execution_id: Some(first.id.clone()),
        status: "started".to_string(),
        event_id: Some(format!("event-{suffix}")),
        detail_json: Some(json!({"origin": "event:test"}).to_string()),
    };
    assert!(
        store
            .insert_execution_with_trigger(&first_record, &first_event, &first_fire)
            .await
            .expect("insert first execution")
    );

    let mut second = new_state(
        &workflow,
        &compilation.plan,
        format!("exec_second_{suffix}"),
        json!({"value": 2}),
        Default::default(),
        tenant_id.clone(),
    )
    .expect("second state");
    let second_event = start_event(&mut second);
    let second_record = ExecutionRecord {
        id: second.id.clone(),
        workflow_id: workflow.id.clone(),
        workflow_version: workflow.workflow_version.clone(),
        status: "running".to_string(),
        state_json: serde_json::to_string(&second).unwrap(),
        workflow_json: workflow.to_json_string(false).unwrap(),
        output_json: None,
        error_json: None,
        started_at: second.started_at.clone(),
        updated_at: second.updated_at.clone(),
    };
    let second_fire = TriggerFireRecord {
        id: format!("fire_second_{suffix}"),
        execution_id: Some(second.id.clone()),
        ..first_fire
    };
    assert!(
        !store
            .insert_execution_with_trigger(&second_record, &second_event, &second_fire)
            .await
            .expect("duplicate trigger fire")
    );

    let execution_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM hpa_tenant.executions
         WHERE tenant_id = $1 AND workflow_id = $2",
    )
    .bind(&tenant_id)
    .bind(&workflow.id)
    .fetch_one(&pool)
    .await
    .expect("count atomic executions");
    assert_eq!(execution_count, 1);
    let execution_id: String = sqlx::query_scalar(
        "SELECT execution_id FROM hpa_tenant.trigger_fires
         WHERE tenant_id = $1 AND workflow_id = $2",
    )
    .bind(&tenant_id)
    .bind(&workflow.id)
    .fetch_one(&pool)
    .await
    .expect("read atomic trigger fire");
    assert_eq!(execution_id, first.id);

    sqlx::query("DELETE FROM hpa_control.tenants WHERE id = $1")
        .bind(&tenant_id)
        .execute(&pool)
        .await
        .expect("clean atomic tenant");
    pool.close().await;
}
