use hpa_local::persistent_mcp::{McpStoreError, PostgresMcpStore, PostgresMcpTaskNode, TaskUpsert};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations/postgres");

#[tokio::test]
async fn persistent_mcp_state_is_tenant_and_token_scoped() {
    let Ok(database_url) = std::env::var("HPA_POSTGRES_TEST_URL") else {
        eprintln!("skipped: HPA_POSTGRES_TEST_URL is not set");
        return;
    };
    let pool = PgPoolOptions::new()
        .max_connections(6)
        .connect(&database_url)
        .await
        .expect("connect PostgreSQL");
    MIGRATOR.run(&pool).await.expect("apply migrations");

    let suffix = Uuid::now_v7().simple().to_string();
    let alpha = format!("mcp_alpha_{suffix}");
    let beta = format!("mcp_beta_{suffix}");
    for tenant in [&alpha, &beta] {
        sqlx::query("INSERT INTO hpa_control.tenants (id, name) VALUES ($1, $1)")
            .bind(tenant)
            .execute(&pool)
            .await
            .expect("insert MCP test tenant");
    }
    let alpha_token = PostgresMcpStore::new(pool.clone(), &alpha, "token-alpha");
    let alpha_other_token = PostgresMcpStore::new(pool.clone(), &alpha, "token-other");
    let beta_token = PostgresMcpStore::new(pool.clone(), &beta, "token-beta");

    let session = alpha_token
        .create_session(Some("subject-alpha"), 3600)
        .await
        .expect("create persistent session");
    assert!(
        alpha_token
            .validate_session(&session.id)
            .await
            .expect("validate owning token")
    );
    assert!(
        !alpha_other_token
            .validate_session(&session.id)
            .await
            .expect("reject same-tenant other token")
    );
    assert!(
        !beta_token
            .validate_session(&session.id)
            .await
            .expect("reject other tenant")
    );
    let token_mismatch = alpha_other_token
        .create_subscription(&session.id, &json!({"toolsListChanged": true}), 60)
        .await
        .expect_err("other token must not create a subscription");
    assert!(matches!(token_mismatch, McpStoreError::SessionNotFound(_)));

    let subscription = alpha_token
        .create_subscription(
            &session.id,
            &json!({
                "toolsListChanged": true,
                "resourcesListChanged": false,
                "promptsListChanged": false
            }),
            60,
        )
        .await
        .expect("create persistent subscription");
    let tools_sequence = alpha_token
        .publish_notification("tools", &json!({"toolsListChanged": true}))
        .await
        .expect("publish tools notification");
    alpha_token
        .publish_notification("prompts", &json!({"promptsListChanged": true}))
        .await
        .expect("publish filtered prompts notification");
    let notifications = alpha_token
        .poll_subscription(&subscription, 0, 10)
        .await
        .expect("poll subscription");
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].kind, "tools");
    assert_eq!(notifications[0].sequence, tools_sequence);
    let cross_tenant = beta_token
        .poll_subscription(&subscription, 0, 10)
        .await
        .expect_err("other tenant must not poll subscription");
    assert!(matches!(
        cross_tenant,
        McpStoreError::SubscriptionNotFound(_)
    ));
    let resumed = PostgresMcpStore::new(pool.clone(), &alpha, "token-alpha")
        .poll_subscription(&subscription, tools_sequence, 10)
        .await
        .expect("resume from sequence");
    assert!(resumed.is_empty());
    for _ in 0..3 {
        alpha_token
            .publish_notification("tools", &json!({"toolsListChanged": true}))
            .await
            .expect("publish bounded backlog fixture");
    }
    assert_eq!(
        alpha_token
            .pending_notification_count(&subscription, tools_sequence, 2)
            .await
            .expect("capped backlog count"),
        2
    );
    assert_eq!(
        alpha_token
            .pending_notification_count(&subscription, tools_sequence, 10)
            .await
            .expect("full backlog count"),
        3
    );
    assert!(matches!(
        beta_token
            .pending_notification_count(&subscription, tools_sequence, 10)
            .await
            .expect_err("other tenant must not inspect subscription backlog"),
        McpStoreError::SubscriptionNotFound(_)
    ));

    let task_id = format!("task_{suffix}");
    let task = TaskUpsert {
        id: task_id.clone(),
        session_id: Some(session.id.clone()),
        method: "workflow.execute".to_string(),
        status: "working".to_string(),
        input: json!({"workflowId": "workflow-1"}),
        output: None,
        error: None,
        approval: Some(json!({"required": true, "state": "pending"})),
        execution_id: Some(format!("execution_{suffix}")),
        owner_node_id: Some("node-alpha".to_string()),
        expires_in_seconds: Some(60),
    };
    let created = alpha_token.upsert_task(&task).await.expect("create task");
    assert_eq!(created.status, "working");
    assert!(matches!(
        alpha_other_token
            .get_task(&task_id)
            .await
            .expect_err("other token must not read task"),
        McpStoreError::TaskNotFound(_)
    ));
    let completed = alpha_token
        .upsert_task(&TaskUpsert {
            status: "completed".to_string(),
            output: Some(json!({"status": "succeeded"})),
            approval: Some(json!({"required": true, "state": "approved"})),
            ..task.clone()
        })
        .await
        .expect("complete task");
    assert_eq!(completed.status, "completed");
    assert_eq!(
        completed.output_json.expect("task output")["status"],
        "succeeded"
    );
    assert!(matches!(
        alpha_token
            .cancel_task(&task_id)
            .await
            .expect_err("completed task cannot be cancelled"),
        McpStoreError::InvalidTaskTransition { .. }
    ));

    let cancellable_task_id = format!("task_cancel_{suffix}");
    alpha_token
        .upsert_task(&TaskUpsert {
            id: cancellable_task_id.clone(),
            session_id: Some(session.id.clone()),
            method: "workflow.execute".to_string(),
            status: "input_required".to_string(),
            input: json!({"workflowId": "workflow-cancel"}),
            output: None,
            error: None,
            approval: Some(json!({"required": true, "state": "pending"})),
            execution_id: Some(format!("execution_cancel_{suffix}")),
            owner_node_id: Some("node-alpha".to_string()),
            expires_in_seconds: Some(60),
        })
        .await
        .expect("create cancellable task");
    assert!(matches!(
        alpha_other_token
            .request_task_cancel(&cancellable_task_id)
            .await
            .expect_err("other token must not request cancellation"),
        McpStoreError::TaskNotFound(_)
    ));
    let requested = alpha_token
        .request_task_cancel(&cancellable_task_id)
        .await
        .expect("request durable task cancellation");
    assert_eq!(
        requested.approval_json.as_ref().expect("approval")["cancelRequested"],
        true
    );
    let node_alpha = PostgresMcpTaskNode::new(pool.clone(), &alpha, "node-alpha");
    let node_beta = PostgresMcpTaskNode::new(pool.clone(), &alpha, "node-beta");
    assert!(
        node_beta
            .claim(10)
            .await
            .expect("non-owner claim")
            .is_empty()
    );
    let commands = node_alpha.claim(10).await.expect("claim cancellation");
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].task_id, cancellable_task_id);
    assert_eq!(commands[0].action, "cancel");
    assert_eq!(
        commands[0].execution_id.as_deref(),
        Some(format!("execution_cancel_{suffix}").as_str())
    );
    let cancelled = alpha_token
        .cancel_task(&cancellable_task_id)
        .await
        .expect("cancel persistent task");
    assert_eq!(cancelled.status, "cancelled");
    assert!(
        node_alpha
            .complete(&commands[0].id)
            .await
            .expect("complete task command")
    );
    assert!(
        node_alpha
            .claim(10)
            .await
            .expect("no duplicate command")
            .is_empty()
    );

    assert!(
        alpha_token
            .close_session(&session.id)
            .await
            .expect("close session")
    );
    assert!(
        !alpha_token
            .validate_session(&session.id)
            .await
            .expect("closed session is invalid")
    );
    let task_after_session = alpha_token
        .get_task(&task_id)
        .await
        .expect("task survives session close");
    assert!(task_after_session.session_id.is_none());
    assert!(matches!(
        alpha_token
            .poll_subscription(&subscription, 0, 10)
            .await
            .expect_err("subscription is removed with session"),
        McpStoreError::SubscriptionNotFound(_)
    ));

    let expiring = alpha_token
        .create_session(None, 1)
        .await
        .expect("create expiring session");
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    assert!(
        !alpha_token
            .validate_session(&expiring.id)
            .await
            .expect("expired session is invalid")
    );

    sqlx::query("DELETE FROM hpa_control.tenants WHERE id = ANY($1)")
        .bind(vec![alpha, beta])
        .execute(&pool)
        .await
        .expect("clean MCP tenants");
    pool.close().await;
}
