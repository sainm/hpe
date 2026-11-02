use hpa_local::distributed_runtime::{
    IdempotencyStatus, OutboxMessageInput, RuntimeError, SchedulerLeaseStore, TenantRuntime,
    WorkerLeaseStore,
};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations/postgres");

fn message(id: &str, key: &str, max_attempts: i32) -> OutboxMessageInput {
    OutboxMessageInput {
        id: id.to_string(),
        topic: "executions".to_string(),
        event_type: "execution.completed".to_string(),
        aggregate_type: "execution".to_string(),
        aggregate_id: format!("aggregate-{id}"),
        payload: json!({"executionId": id}),
        headers: json!({"source": "test"}),
        idempotency_key: key.to_string(),
        max_attempts,
        trace_id: Some(format!("trace-{id}")),
    }
}

#[tokio::test]
async fn distributed_outbox_leases_and_idempotency_are_fail_closed() {
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
    let tenant_id = format!("runtime_{suffix}");
    sqlx::query("INSERT INTO hpa_control.tenants (id, name) VALUES ($1, 'Runtime test')")
        .bind(&tenant_id)
        .execute(&pool)
        .await
        .expect("insert runtime tenant");

    let consumer_a = TenantRuntime::new(pool.clone(), &tenant_id, "consumer-a");
    let consumer_b = TenantRuntime::new(pool.clone(), &tenant_id, "consumer-b");
    let consumer_c = TenantRuntime::new(pool.clone(), &tenant_id, "consumer-c");

    let first = message(&format!("message-{suffix}-1"), "event-1", 10);
    assert!(consumer_a.enqueue(&first).await.expect("enqueue first"));
    let duplicate = message(&format!("message-{suffix}-duplicate"), "event-1", 10);
    assert!(
        !consumer_a
            .enqueue(&duplicate)
            .await
            .expect("enqueue duplicate")
    );

    let (claimed_a, claimed_b) = tokio::join!(consumer_a.claim(1, 30), consumer_b.claim(1, 30));
    let claimed_a = claimed_a.expect("claim as A");
    let claimed_b = claimed_b.expect("claim as B");
    assert_eq!(
        claimed_a.len() + claimed_b.len(),
        1,
        "one outbox row must be claimed once"
    );
    let (winner, winner_message) = if let Some(message) = claimed_a.first() {
        (&consumer_a, message)
    } else {
        (&consumer_b, claimed_b.first().expect("one claim winner"))
    };
    assert!(
        !winner
            .complete(&winner_message.id, winner_message.fencing_token - 1)
            .await
            .expect("reject stale complete")
    );
    assert!(
        winner
            .complete(&winner_message.id, winner_message.fencing_token)
            .await
            .expect("complete claimed message")
    );
    assert!(
        consumer_c
            .claim(1, 30)
            .await
            .expect("claim processed message")
            .is_empty()
    );

    let retry = message(&format!("message-{suffix}-retry"), "event-retry", 3);
    assert!(consumer_a.enqueue(&retry).await.expect("enqueue retry"));
    let first_lease = consumer_a
        .claim(1, 1)
        .await
        .expect("claim first lease")
        .pop()
        .expect("first lease row");
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    let second_lease = consumer_b
        .claim(1, 30)
        .await
        .expect("take over expired lease")
        .pop()
        .expect("second lease row");
    assert!(second_lease.fencing_token > first_lease.fencing_token);
    assert!(
        !consumer_a
            .complete(&first_lease.id, first_lease.fencing_token)
            .await
            .expect("reject old owner")
    );
    assert!(
        consumer_b
            .fail(
                &second_lease.id,
                second_lease.fencing_token,
                "transient failure",
                0,
            )
            .await
            .expect("return message to queue")
    );
    let third_lease = consumer_c
        .claim(1, 30)
        .await
        .expect("claim retry")
        .pop()
        .expect("third lease row");
    assert!(
        consumer_c
            .fail(
                &third_lease.id,
                third_lease.fencing_token,
                "permanent failure",
                0,
            )
            .await
            .expect("dead-letter exhausted message")
    );
    let retry_status: String =
        sqlx::query_scalar("SELECT status FROM hpa_control.outbox WHERE id = $1")
            .bind(&retry.id)
            .fetch_one(&pool)
            .await
            .expect("read retry status");
    assert_eq!(retry_status, "dead");

    let reservation = consumer_a
        .reserve_idempotency("workflow.execute", "request-1", "hash-a")
        .await
        .expect("reserve idempotency");
    assert!(reservation.acquired);
    let duplicate_reservation = consumer_b
        .reserve_idempotency("workflow.execute", "request-1", "hash-a")
        .await
        .expect("read concurrent idempotency");
    assert!(!duplicate_reservation.acquired);
    assert_eq!(duplicate_reservation.status, "in_progress");
    let mismatch = consumer_b
        .reserve_idempotency("workflow.execute", "request-1", "hash-b")
        .await
        .expect_err("reject idempotency hash mismatch");
    assert!(matches!(
        mismatch,
        RuntimeError::IdempotencyKeyReused { .. }
    ));
    assert!(
        consumer_a
            .finish_idempotency(
                "workflow.execute",
                "request-1",
                IdempotencyStatus::Succeeded,
                Some("execution"),
                Some("execution-1"),
                Some(&json!({"status": "succeeded"})),
            )
            .await
            .expect("finish idempotency")
    );
    let replay = consumer_b
        .reserve_idempotency("workflow.execute", "request-1", "hash-a")
        .await
        .expect("read completed idempotency");
    assert!(!replay.acquired);
    assert_eq!(replay.status, "succeeded");
    assert_eq!(replay.resource_id.as_deref(), Some("execution-1"));
    assert_eq!(
        replay.response.expect("stored response")["status"],
        "succeeded"
    );

    let scheduler_a = SchedulerLeaseStore::new(pool.clone(), "scheduler-a");
    let scheduler_b = SchedulerLeaseStore::new(pool.clone(), "scheduler-b");
    let lease_key = format!("scheduler:{suffix}");
    let first_scheduler = scheduler_a
        .acquire(&lease_key, 1)
        .await
        .expect("acquire scheduler lease")
        .expect("scheduler A wins");
    assert!(
        scheduler_b
            .acquire(&lease_key, 1)
            .await
            .expect("probe scheduler B")
            .is_none()
    );
    assert!(
        scheduler_a
            .renew(&lease_key, first_scheduler.fencing_token, 1)
            .await
            .expect("renew scheduler lease")
    );
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    let second_scheduler = scheduler_b
        .acquire(&lease_key, 30)
        .await
        .expect("take over scheduler lease")
        .expect("scheduler B takes over");
    assert!(second_scheduler.fencing_token > first_scheduler.fencing_token);
    assert!(
        !scheduler_a
            .renew(&lease_key, first_scheduler.fencing_token, 30)
            .await
            .expect("reject old scheduler")
    );
    assert!(
        scheduler_b
            .release(&lease_key, second_scheduler.fencing_token)
            .await
            .expect("release scheduler lease")
    );

    let worker_a_id = format!("worker-a-{suffix}");
    let worker_b_id = format!("worker-b-{suffix}");
    for worker_id in [&worker_a_id, &worker_b_id] {
        sqlx::query(
            "INSERT INTO hpa_control.workers (
                 id, tenant_id, name, kind, status, capabilities_json,
                 max_concurrency, runtime_version, last_heartbeat_at,
                 registered_at, metadata_json
             )
             VALUES (
                 $1, $2, $1, 'workflow', 'active', '[\"workflow\"]'::jsonb,
                 1, 'test', 0, 0, '{}'::jsonb
             )",
        )
        .bind(worker_id)
        .bind(&tenant_id)
        .execute(&pool)
        .await
        .expect("insert worker");
    }
    let worker_job_id = format!("job-{suffix}-1");
    let second_worker_job_id = format!("job-{suffix}-2");
    sqlx::query(
        "INSERT INTO hpa_control.worker_jobs (
             id, tenant_id, execution_id, workflow_id, workflow_version,
             workflow_json, state_json, status, runtime_version,
             required_capabilities_json, created_at, updated_at
         )
         VALUES (
             $1, $2, $1, 'workflow-1', '1.0.0',
             '{\"nodes\":[],\"edges\":[]}'::jsonb, '{}'::jsonb,
             'queued', 'test', '[\"workflow\"]'::jsonb, 0, 0
         )",
    )
    .bind(&worker_job_id)
    .bind(&tenant_id)
    .execute(&pool)
    .await
    .expect("insert worker job");
    let worker_a = WorkerLeaseStore::new(pool.clone(), &tenant_id, &worker_a_id);
    let worker_b = WorkerLeaseStore::new(pool.clone(), &tenant_id, &worker_b_id);
    assert!(worker_a.heartbeat().await.expect("worker heartbeat"));
    let first_worker_lease = worker_a
        .claim(1)
        .await
        .expect("claim worker job")
        .expect("worker A lease");
    assert!(worker_b.claim(1).await.expect("probe worker B").is_none());
    assert!(
        worker_a
            .renew(
                &first_worker_lease.id,
                first_worker_lease.lease_id.as_deref().expect("lease id"),
                first_worker_lease.fencing_token,
                1,
            )
            .await
            .expect("renew worker lease")
    );
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    let second_worker_lease = worker_b
        .claim(30)
        .await
        .expect("take over worker job")
        .expect("worker B lease");
    assert_eq!(second_worker_lease.id, first_worker_lease.id);
    assert!(second_worker_lease.fencing_token > first_worker_lease.fencing_token);
    assert!(
        !worker_a
            .finish(
                &first_worker_lease.id,
                first_worker_lease.lease_id.as_deref().expect("lease id"),
                first_worker_lease.fencing_token,
                "succeeded",
                None,
            )
            .await
            .expect("reject stale worker completion")
    );
    assert!(
        worker_b
            .finish(
                &second_worker_lease.id,
                second_worker_lease.lease_id.as_deref().expect("lease id"),
                second_worker_lease.fencing_token,
                "succeeded",
                None,
            )
            .await
            .expect("finish worker job")
    );

    sqlx::query(
        "INSERT INTO hpa_control.worker_jobs (
             id, tenant_id, execution_id, workflow_id, workflow_version,
             workflow_json, state_json, status, runtime_version,
             required_capabilities_json, created_at, updated_at
         )
         VALUES (
             $1, $2, $1, 'workflow-2', '1.0.0',
             '{\"nodes\":[],\"edges\":[]}'::jsonb, '{}'::jsonb,
             'queued', 'test', '[\"workflow\"]'::jsonb, 0, 0
         )",
    )
    .bind(&second_worker_job_id)
    .bind(&tenant_id)
    .execute(&pool)
    .await
    .expect("insert redrive worker job");

    let redrive_lease = worker_a
        .claim(30)
        .await
        .expect("claim redrive job")
        .expect("redrive lease");
    assert_eq!(redrive_lease.id, second_worker_job_id);
    assert!(
        worker_a
            .requeue(
                &redrive_lease.id,
                redrive_lease.lease_id.as_deref().expect("lease id"),
                redrive_lease.fencing_token,
                "worker restart",
            )
            .await
            .expect("requeue worker job")
    );
    let redriven = worker_b
        .claim(30)
        .await
        .expect("claim redriven job")
        .expect("redriven lease");
    assert_eq!(redriven.id, second_worker_job_id);
    assert!(redriven.fencing_token > redrive_lease.fencing_token);

    sqlx::query("DELETE FROM hpa_control.tenants WHERE id = $1")
        .bind(&tenant_id)
        .execute(&pool)
        .await
        .expect("clean runtime tenant");
    pool.close().await;
}
