use hpa_core::ir::Workflow;
use hpa_local::postgres_execution::PostgresExecutionStore;
use hpa_local::postgres_trigger::{PostgresTriggerStore, TriggerFireRecord, TriggerSeedRecord};
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations/postgres");

fn workflow_with_triggers() -> Workflow {
    let mut value: Value = serde_json::from_str(include_str!("../../../fixtures/01_sequence.json"))
        .expect("sequence fixture");
    value["triggers"] = json!([
        {
            "id": "nightly",
            "type": "schedule",
            "config": {"cron": "0 9 * * *", "timezone": "+08:00"}
        },
        {
            "id": "hook",
            "type": "webhook",
            "config": {"secret": "0123456789abcdef"}
        },
        {
            "id": "bus",
            "type": "event",
            "config": {"topic": "orders.created"}
        }
    ]);
    serde_json::from_value(value).expect("workflow with triggers")
}

#[tokio::test]
async fn postgres_trigger_state_and_fires_are_tenant_scoped_and_idempotent() {
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
    let tenant_id = format!("trigger_{suffix}");
    sqlx::query("INSERT INTO hpa_control.tenants (id, name) VALUES ($1, $1)")
        .bind(&tenant_id)
        .execute(&pool)
        .await
        .expect("insert trigger tenant");
    let workflow = workflow_with_triggers();
    PostgresExecutionStore::new(pool.clone(), &tenant_id)
        .sync_workflow(
            &workflow.to_json_string(false).unwrap(),
            &workflow.workflow_version,
            "registry-trigger-test",
            "2026-11-02T00:00:00Z",
            "2026-11-02T00:00:01Z",
        )
        .await
        .expect("sync workflow snapshot");
    let store = PostgresTriggerStore::new(pool.clone(), &tenant_id);
    let other_store = PostgresTriggerStore::new(pool.clone(), "other-trigger-tenant");
    let schedule_at = 1_793_000_000_000i64;
    let seeds = vec![
        TriggerSeedRecord {
            trigger_id: "nightly".to_string(),
            kind: "schedule".to_string(),
            config_json: r#"{"cron":"0 9 * * *","timezone":"+08:00"}"#.to_string(),
            enabled: true,
            next_fire_at: Some(schedule_at),
        },
        TriggerSeedRecord {
            trigger_id: "hook".to_string(),
            kind: "webhook".to_string(),
            config_json: r#"{"secret":"0123456789abcdef"}"#.to_string(),
            enabled: true,
            next_fire_at: None,
        },
        TriggerSeedRecord {
            trigger_id: "bus".to_string(),
            kind: "event".to_string(),
            config_json: r#"{"topic":"orders.created"}"#.to_string(),
            enabled: true,
            next_fire_at: None,
        },
    ];
    store
        .sync_triggers(
            &workflow.id,
            Some(&workflow.workflow_version),
            &seeds,
            "2026-11-02T00:00:02Z",
        )
        .await
        .expect("sync triggers");
    let triggers = store.list_triggers().await.expect("list triggers");
    assert_eq!(triggers.len(), 3);
    assert!(
        other_store
            .list_triggers()
            .await
            .expect("other tenant triggers")
            .is_empty()
    );
    let due = store
        .due_triggers(schedule_at, 10)
        .await
        .expect("due triggers");
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].trigger_id, "nightly");
    assert!(
        store
            .claim_schedule_slot(
                &workflow.id,
                "nightly",
                schedule_at,
                schedule_at,
                Some(schedule_at + 60_000),
                "2026-11-02T00:00:03Z",
            )
            .await
            .expect("claim schedule")
    );
    assert!(
        !store
            .claim_schedule_slot(
                &workflow.id,
                "nightly",
                schedule_at,
                schedule_at,
                None,
                "2026-11-02T00:00:04Z",
            )
            .await
            .expect("duplicate claim")
    );
    assert!(
        store
            .set_trigger_enabled(&workflow.id, "hook", false, None, "2026-11-02T00:00:05Z",)
            .await
            .expect("disable webhook")
    );
    store
        .note_trigger_execution(&workflow.id, "nightly", "exec-trigger-postgres", "running")
        .await
        .expect("note execution");

    let fire = TriggerFireRecord {
        id: format!("fire_{suffix}"),
        workflow_id: workflow.id.clone(),
        trigger_id: "bus".to_string(),
        kind: "event".to_string(),
        fired_at: "2026-11-02T00:00:06Z".to_string(),
        scheduled_at: None,
        execution_id: Some("exec-trigger-postgres".to_string()),
        status: "started".to_string(),
        event_id: Some(format!("event-{suffix}")),
        detail_json: Some(r#"{"origin":"event:orders.created"}"#.to_string()),
    };
    assert!(
        store
            .record_trigger_fire(&fire)
            .await
            .expect("record first fire")
    );
    let duplicate = TriggerFireRecord {
        id: format!("fire_duplicate_{suffix}"),
        ..fire.clone()
    };
    assert!(
        !store
            .record_trigger_fire(&duplicate)
            .await
            .expect("reject duplicate event")
    );
    assert!(
        store
            .trigger_event_delivered("bus", fire.event_id.as_deref().unwrap())
            .await
            .expect("delivered event")
    );
    assert_eq!(store.list_trigger_fires(10).await.unwrap().len(), 1);

    store
        .sync_triggers(
            &workflow.id,
            Some(&workflow.workflow_version),
            &seeds[..2],
            "2026-11-02T00:00:07Z",
        )
        .await
        .expect("remove event subscription");
    let triggers = store.list_triggers().await.expect("triggers after sync");
    assert_eq!(triggers.len(), 2);
    let nightly = store
        .get_trigger(&workflow.id, "nightly")
        .await
        .expect("get nightly")
        .expect("nightly exists");
    assert_eq!(
        nightly.last_execution_id.as_deref(),
        Some("exec-trigger-postgres")
    );
    assert_eq!(nightly.last_fire_at, Some(schedule_at));

    sqlx::query("DELETE FROM hpa_control.tenants WHERE id = $1")
        .bind(&tenant_id)
        .execute(&pool)
        .await
        .expect("clean trigger tenant");
    pool.close().await;
}
