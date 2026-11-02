use hpa_core::compiler::compile_workflow;
use hpa_core::ir::Workflow;
use hpa_core::runtime::{
    ExecutionEvent, ExecutionState, ExecutionStatus, new_state, start_event, step,
};
use hpa_local::postgres_execution::{
    ExecutionRecord, PostgresExecutionError, PostgresExecutionStore,
};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations/postgres");

fn execution_record(workflow: &Workflow, state: &ExecutionState, version: &str) -> ExecutionRecord {
    ExecutionRecord {
        id: state.id.clone(),
        workflow_id: workflow.id.clone(),
        workflow_version: version.to_string(),
        status: "running".to_string(),
        state_json: serde_json::to_string(state).expect("serialize state"),
        workflow_json: workflow.to_json_string(false).expect("serialize workflow"),
        output_json: None,
        error_json: None,
        started_at: state.started_at.clone(),
        updated_at: state.updated_at.clone(),
    }
}

fn make_start_event(state: &mut ExecutionState) -> ExecutionEvent {
    start_event(state)
}

#[tokio::test]
async fn postgres_execution_store_round_trips_events_and_artifacts() {
    let Ok(database_url) = std::env::var("HPA_POSTGRES_TEST_URL") else {
        eprintln!("skipped: HPA_POSTGRES_TEST_URL is not set");
        return;
    };
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&database_url)
        .await
        .expect("connect PostgreSQL");
    MIGRATOR.run(&pool).await.expect("apply migrations");

    let suffix = Uuid::now_v7().simple().to_string();
    let tenant_id = format!("execution_{suffix}");
    sqlx::query("INSERT INTO hpa_control.tenants (id, name) VALUES ($1, $1)")
        .bind(&tenant_id)
        .execute(&pool)
        .await
        .expect("insert execution tenant");
    let store = PostgresExecutionStore::new(pool.clone(), &tenant_id);
    let other_store = PostgresExecutionStore::new(pool.clone(), "other-execution-tenant");

    let workflow: Workflow =
        serde_json::from_str(include_str!("../../../fixtures/01_sequence.json"))
            .expect("sequence workflow");
    let compilation = compile_workflow(&workflow).expect("compile workflow");
    let mut state = new_state(
        &workflow,
        &compilation.plan,
        format!("exec_{suffix}"),
        json!({"value": 7}),
        Default::default(),
        tenant_id.clone(),
    )
    .expect("new execution");
    state.registry_version = "registry-test-v1".to_string();
    let start = make_start_event(&mut state);
    let record = execution_record(&workflow, &state, "1.0.0");
    store
        .insert_execution(&record, &start)
        .await
        .expect("insert execution");
    let registry_version: String = sqlx::query_scalar(
        "SELECT registry_version FROM hpa_tenant.workflow_versions
         WHERE tenant_id = $1 AND workflow_id = $2 AND version = '1.0.0'",
    )
    .bind(&tenant_id)
    .bind(&workflow.id)
    .fetch_one(&pool)
    .await
    .expect("read registry version");
    assert_eq!(registry_version, "registry-test-v1");

    let mut events = Vec::new();
    for _ in 0..20 {
        if state.status.is_terminal() {
            break;
        }
        let result = step(&workflow, &compilation.plan, &mut state).await;
        events.extend(result.events);
    }
    assert_eq!(state.status, ExecutionStatus::Succeeded);
    store
        .save_execution(&state, &events)
        .await
        .expect("save execution");
    let loaded = store
        .get_execution(&state.id)
        .await
        .expect("load execution")
        .expect("execution exists");
    assert_eq!(loaded.status, "succeeded");
    assert_eq!(
        serde_json::from_str::<ExecutionState>(&loaded.state_json)
            .expect("parse loaded state")
            .status,
        ExecutionStatus::Succeeded
    );
    assert!(store.get_execution(&state.id).await.unwrap().is_some());
    let other = sqlx::query(
        "INSERT INTO hpa_control.tenants (id, name) VALUES ($1, $1)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind("other-execution-tenant")
    .execute(&pool)
    .await;
    other.expect("insert other tenant");
    assert!(
        other_store
            .get_execution(&state.id)
            .await
            .expect("tenant-scoped lookup")
            .is_none()
    );

    let stored_events = store
        .list_events(&state.id, -1, 100)
        .await
        .expect("list events");
    assert!(stored_events.len() >= 2);
    assert_eq!(stored_events[0].sequence, start.sequence);
    assert!(stored_events.last().unwrap().sequence > start.sequence);
    store
        .save_output_artifact(&state)
        .await
        .expect("save output artifact");
    let artifacts = store
        .list_artifacts(Some(&state.id))
        .await
        .expect("list artifacts");
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].name, "output.json");
    let counts = store.execution_counts().await.expect("execution counts");
    assert_eq!(counts, (1, 0, 0, 1, 0));

    let mut remote_state = new_state(
        &workflow,
        &compilation.plan,
        format!("exec_remote_{suffix}"),
        json!({"value": 9}),
        Default::default(),
        tenant_id.clone(),
    )
    .expect("remote execution state");
    let remote_start = make_start_event(&mut remote_state);
    let remote_record = execution_record(&workflow, &remote_state, "1.0.0");
    store
        .insert_execution(&remote_record, &remote_start)
        .await
        .expect("insert remote execution");
    store
        .mark_remote_execution(&remote_state.id, "worker-postgres")
        .await
        .expect("mark remote execution");
    assert!(
        store
            .is_remote_execution(&remote_state.id)
            .await
            .expect("read remote marker")
    );
    assert!(
        store
            .running_executions()
            .await
            .expect("local running executions")
            .iter()
            .all(|row| row.id.as_str() != remote_state.id.as_str())
    );
    assert_eq!(
        store
            .execution_count_since("1970-01-01")
            .await
            .expect("execution count since epoch"),
        2
    );
    assert_eq!(
        store
            .active_execution_count()
            .await
            .expect("active execution count"),
        1
    );

    let mut wrong_tenant = new_state(
        &workflow,
        &compilation.plan,
        format!("exec_wrong_tenant_{suffix}"),
        json!({"value": 10}),
        Default::default(),
        tenant_id.clone(),
    )
    .expect("wrong tenant state");
    wrong_tenant.tenant_id = "forged-tenant".to_string();
    let wrong_tenant_start = make_start_event(&mut wrong_tenant);
    let wrong_tenant_record = execution_record(&workflow, &wrong_tenant, "1.0.0");
    assert!(matches!(
        store
            .insert_execution(&wrong_tenant_record, &wrong_tenant_start)
            .await
            .expect_err("forged state tenant must fail"),
        PostgresExecutionError::TenantMismatch { .. }
    ));

    let mut wrong_event_state = new_state(
        &workflow,
        &compilation.plan,
        format!("exec_wrong_event_{suffix}"),
        json!({"value": 10}),
        Default::default(),
        tenant_id.clone(),
    )
    .expect("wrong event state");
    let mut wrong_event_start = make_start_event(&mut wrong_event_state);
    wrong_event_start.execution_id = "another-execution".to_string();
    let wrong_event_record = execution_record(&workflow, &wrong_event_state, "1.0.0");
    assert!(matches!(
        store
            .insert_execution(&wrong_event_record, &wrong_event_start)
            .await
            .expect_err("event execution mismatch must fail"),
        PostgresExecutionError::EventExecutionMismatch { .. }
    ));

    let mut conflicting = new_state(
        &workflow,
        &compilation.plan,
        format!("exec_conflict_{suffix}"),
        json!({"value": 7}),
        Default::default(),
        tenant_id.clone(),
    )
    .expect("conflicting state");
    let conflict_start = make_start_event(&mut conflicting);
    let mut conflict_record = execution_record(&workflow, &conflicting, "1.0.0");
    let mut changed_workflow = workflow.clone();
    changed_workflow.name = "Changed conformance workflow".to_string();
    conflict_record.workflow_json = changed_workflow.to_json_string(false).unwrap();
    let conflict = store
        .insert_execution(&conflict_record, &conflict_start)
        .await
        .expect_err("workflow snapshot must not change");
    assert!(matches!(
        conflict,
        PostgresExecutionError::WorkflowSnapshotConflict { .. }
    ));

    sqlx::query("DELETE FROM hpa_control.tenants WHERE id = ANY($1)")
        .bind(vec![tenant_id, "other-execution-tenant".to_string()])
        .execute(&pool)
        .await
        .expect("clean execution tenants");
    pool.close().await;
}
