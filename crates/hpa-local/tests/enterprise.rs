use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use reqwest::Method;
use reqwest::header::CONTENT_TYPE;
use serde_json::{Value, json};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hpa")
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

fn fixture_workflow(name: &str) -> Value {
    let content = std::fs::read_to_string(fixture(name)).expect("read fixture");
    serde_json::from_str(&content).expect("workflow json")
}

fn temp_dir(label: &str) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("hpa-enterprise-{label}-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&path).expect("temp dir");
    path
}

fn run_cli(data_dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(binary())
        .arg("--data-dir")
        .arg(data_dir)
        .args(args)
        .output()
        .expect("run cli")
}

fn init_admin(data_dir: &Path) -> String {
    let output = run_cli(data_dir, &["enterprise", "init"]);
    assert!(
        output.status.success(),
        "enterprise init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("init json");
    assert_eq!(value["tokenInfo"]["platformAdmin"], true);
    value["token"].as_str().expect("init token").to_string()
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("addr")
        .port()
}

async fn wait_for_health(client: &reqwest::Client, port: u16) -> bool {
    for _ in 0..400 {
        if let Ok(response) = client
            .get(format!("http://127.0.0.1:{port}/api/health"))
            .send()
            .await
            && response.status().is_success()
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

struct Response {
    status: u16,
    content_type: Option<String>,
    body: String,
}

impl Response {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

struct Server {
    child: Child,
    port: u16,
    client: reqwest::Client,
}

impl Server {
    async fn start(data_dir: &Path, enterprise: bool) -> Self {
        let port = free_port();
        let mut command = Command::new(binary());
        command
            .arg("--data-dir")
            .arg(data_dir)
            .args(["serve", "--port", &port.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .env_remove("HPA_ENTERPRISE_MODE");
        if enterprise {
            command.env("HPA_ENTERPRISE_MODE", "required");
        }
        let mut child = command.spawn().expect("spawn server");
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("http client");
        if !wait_for_health(&client, port).await {
            let _ = child.kill();
            let _ = child.wait();
            panic!("server did not become healthy on port {port}");
        }
        Self {
            child,
            port,
            client,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Response {
        let mut request = self.client.request(method, self.url(path));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.expect("http request");
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body = response.text().await.unwrap_or_default();
        Response {
            status,
            content_type,
            body,
        }
    }

    async fn get(&self, path: &str, token: Option<&str>) -> Response {
        self.send(Method::GET, path, token, None).await
    }

    async fn post(&self, path: &str, token: Option<&str>, body: Value) -> Response {
        self.send(Method::POST, path, token, Some(body)).await
    }

    async fn put(&self, path: &str, token: Option<&str>, body: Value) -> Response {
        self.send(Method::PUT, path, token, Some(body)).await
    }

    async fn delete(&self, path: &str, token: Option<&str>, body: Value) -> Response {
        self.send(Method::DELETE, path, token, Some(body)).await
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn create_tenant(server: &Server, admin: &str, name: &str) -> String {
    let response = server
        .post(
            "/api/enterprise/tenants",
            Some(admin),
            json!({"name": name}),
        )
        .await;
    assert_eq!(
        response.status, 200,
        "create tenant `{name}` failed: {}",
        response.body
    );
    response.json()["id"]
        .as_str()
        .expect("tenant id")
        .to_string()
}

async fn create_token(
    server: &Server,
    admin: &str,
    tenant_id: &str,
    name: &str,
    role: &str,
) -> (String, String) {
    let response = server
        .post(
            "/api/enterprise/tokens",
            Some(admin),
            json!({"tenantId": tenant_id, "name": name, "role": role, "platformAdmin": false}),
        )
        .await;
    assert_eq!(
        response.status, 200,
        "create token `{name}` failed: {}",
        response.body
    );
    let value = response.json();
    (
        value["token"].as_str().expect("token secret").to_string(),
        value["tokenInfo"]["id"]
            .as_str()
            .expect("token id")
            .to_string(),
    )
}

struct ProcessGuard(Child);

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn wait_for_execution(server: &Server, execution_id: &str, status: &str) -> Value {
    for _ in 0..240 {
        let response = server
            .get(&format!("/api/executions/{execution_id}"), None)
            .await;
        if response.status == 200 {
            let value = response.json();
            if value["summary"]["status"] == status {
                return value;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("execution {execution_id} did not reach {status}");
}

fn quota_body(max_workflows: i64, max_concurrent_runs: i64) -> Value {
    json!({
        "maxWorkflows": max_workflows,
        "maxExecutionsPerDay": 1000,
        "maxSkills": 100,
        "maxAgents": 100,
        "maxPlugins": 50,
        "maxConnections": 100,
        "maxConcurrentRuns": max_concurrent_runs
    })
}

#[tokio::test]
async fn enterprise_auth_boundary_and_health() {
    let data_dir = temp_dir("auth-required");
    let admin = init_admin(&data_dir);
    let mut server = Server::start(&data_dir, true).await;

    let health = server.get("/api/health", None).await;
    assert_eq!(health.status, 200, "health failed: {}", health.body);
    let health_value = health.json();
    assert!(
        health_value["brand"]["productName"].is_string(),
        "health missing brand.productName: {}",
        health.body
    );
    let brand: Value =
        serde_json::from_str(include_str!("../../../branding/brand.json")).expect("brand file");
    assert_eq!(
        health_value["brand"]["productName"], brand["productName"],
        "unexpected brand: {}",
        health.body
    );
    assert_eq!(health_value["enterprise"], true);

    let unauthenticated = server.get("/api/workflows", None).await;
    assert_eq!(
        unauthenticated.status, 401,
        "workflows should require a token: {}",
        unauthenticated.body
    );
    assert_eq!(unauthenticated.json()["error"]["code"], "UNAUTHORIZED");

    let authenticated = server.get("/api/workflows", Some(&admin)).await;
    assert_eq!(
        authenticated.status, 200,
        "admin token rejected: {}",
        authenticated.body
    );
    assert!(authenticated.json().is_array());
    server.stop();

    let local_dir = temp_dir("auth-local");
    let mut local = Server::start(&local_dir, false).await;
    let health = local.get("/api/health", None).await;
    assert_eq!(health.status, 200);
    assert_eq!(health.json()["enterprise"], false);
    let local_list = local.get("/api/workflows", None).await;
    assert_eq!(
        local_list.status, 200,
        "local mode should not require auth: {}",
        local_list.body
    );
    local.stop();
}

#[tokio::test]
async fn tenant_isolation_and_admin_boundary() {
    let data_dir = temp_dir("isolation");
    let admin = init_admin(&data_dir);
    let mut server = Server::start(&data_dir, true).await;

    let alpha = create_tenant(&server, &admin, "Alpha").await;
    let beta = create_tenant(&server, &admin, "Beta").await;
    let (alpha_owner, _) = create_token(&server, &admin, &alpha, "alpha-owner", "owner").await;
    let (beta_viewer, _) = create_token(&server, &admin, &beta, "beta-viewer", "viewer").await;

    let created = server
        .post(
            "/api/workflows",
            Some(&alpha_owner),
            json!({"ir": fixture_workflow("01_sequence.json")}),
        )
        .await;
    assert_eq!(created.status, 200, "create failed: {}", created.body);
    assert_eq!(created.json()["id"], "wf_fixture_sequence");

    let alpha_list = server.get("/api/workflows", Some(&alpha_owner)).await;
    assert_eq!(
        alpha_list.status, 200,
        "alpha list failed: {}",
        alpha_list.body
    );
    let alpha_ids = alpha_list
        .json()
        .as_array()
        .expect("alpha workflows")
        .iter()
        .map(|row| row["id"].as_str().unwrap_or_default().to_string())
        .collect::<Vec<_>>();
    assert_eq!(alpha_ids, vec!["wf_fixture_sequence".to_string()]);

    let beta_list = server.get("/api/workflows", Some(&beta_viewer)).await;
    assert_eq!(
        beta_list.status, 200,
        "beta list failed: {}",
        beta_list.body
    );
    assert!(
        beta_list
            .json()
            .as_array()
            .expect("beta workflows")
            .is_empty(),
        "beta tenant must not see alpha workflows: {}",
        beta_list.body
    );

    let cross = server
        .get("/api/workflows/wf_fixture_sequence", Some(&beta_viewer))
        .await;
    assert_eq!(
        cross.status, 404,
        "cross-tenant fetch must not resolve: {}",
        cross.body
    );
    assert_eq!(cross.json()["error"]["code"], "NOT_FOUND");
    assert!(
        cross.json().get("ir").is_none(),
        "cross-tenant leak: {}",
        cross.body
    );

    let control = server
        .get("/api/enterprise/tenants", Some(&alpha_owner))
        .await;
    assert_eq!(
        control.status, 403,
        "tenant token reached control plane: {}",
        control.body
    );
    assert_eq!(control.json()["error"]["code"], "PLATFORM_ADMIN_REQUIRED");
    server.stop();
}

#[tokio::test]
async fn rbac_roles_enforced_per_tenant() {
    let data_dir = temp_dir("rbac");
    let admin = init_admin(&data_dir);
    let mut server = Server::start(&data_dir, true).await;
    let alpha = create_tenant(&server, &admin, "Alpha").await;
    let (owner, _) = create_token(&server, &admin, &alpha, "owner", "owner").await;
    let (viewer, _) = create_token(&server, &admin, &alpha, "viewer", "viewer").await;
    let (operator, _) = create_token(&server, &admin, &alpha, "operator", "operator").await;
    let (approver, _) = create_token(&server, &admin, &alpha, "approver", "approver").await;

    let created = server
        .post(
            "/api/workflows",
            Some(&owner),
            json!({"ir": fixture_workflow("01_sequence.json")}),
        )
        .await;
    assert_eq!(created.status, 200, "owner create failed: {}", created.body);

    let viewer_get = server.get("/api/workflows", Some(&viewer)).await;
    assert_eq!(
        viewer_get.status, 200,
        "viewer read failed: {}",
        viewer_get.body
    );
    let viewer_post = server
        .post(
            "/api/workflows",
            Some(&viewer),
            json!({"ir": fixture_workflow("02_condition.json")}),
        )
        .await;
    assert_eq!(
        viewer_post.status, 403,
        "viewer write not denied: {}",
        viewer_post.body
    );
    assert_eq!(viewer_post.json()["error"]["code"], "RBAC_DENIED");

    let operator_run = server
        .post(
            "/api/workflows/wf_fixture_sequence/executions",
            Some(&operator),
            json!({"input": {"value": 1}}),
        )
        .await;
    assert_eq!(
        operator_run.status, 200,
        "operator execute failed: {}",
        operator_run.body
    );
    let operator_post = server
        .post(
            "/api/workflows",
            Some(&operator),
            json!({"ir": fixture_workflow("02_condition.json")}),
        )
        .await;
    assert_eq!(
        operator_post.status, 403,
        "operator create not denied: {}",
        operator_post.body
    );
    assert_eq!(operator_post.json()["error"]["code"], "RBAC_DENIED");

    let approver_get = server.get("/api/workflows", Some(&approver)).await;
    assert_eq!(
        approver_get.status, 200,
        "approver read failed: {}",
        approver_get.body
    );
    let approver_post = server
        .post(
            "/api/workflows",
            Some(&approver),
            json!({"ir": fixture_workflow("02_condition.json")}),
        )
        .await;
    assert_eq!(
        approver_post.status, 403,
        "approver create not denied: {}",
        approver_post.body
    );

    let approval_created = server
        .post(
            "/api/workflows",
            Some(&owner),
            json!({"ir": fixture_workflow("08_approval.json")}),
        )
        .await;
    assert_eq!(
        approval_created.status, 200,
        "approval workflow create failed: {}",
        approval_created.body
    );
    let started = server
        .post(
            "/api/workflows/wf_fixture_approval/executions",
            Some(&operator),
            json!({}),
        )
        .await;
    assert_eq!(started.status, 200, "approval run failed: {}", started.body);

    let mut approval_id = None;
    let mut node_id = None;
    for _ in 0..100 {
        let pending = server.get("/api/approvals", Some(&approver)).await;
        if pending.status == 200
            && let Some(first) = pending
                .json()
                .as_array()
                .and_then(|rows| rows.first())
                .cloned()
        {
            approval_id = first["executionId"].as_str().map(str::to_string);
            node_id = first["request"]["nodeId"].as_str().map(str::to_string);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let approval_id = approval_id.expect("execution waiting for approval");
    let node_id = node_id.expect("approval node id");
    let decided = server
        .post(
            &format!("/api/executions/{approval_id}/approvals"),
            Some(&approver),
            json!({"nodeId": node_id, "decision": "approved", "form": {"decision": "approve"}}),
        )
        .await;
    assert_eq!(
        decided.status, 200,
        "approver decision failed: {}",
        decided.body
    );
    assert_eq!(
        decided.json()["summary"]["workflowId"],
        "wf_fixture_approval"
    );
    server.stop();
}

#[tokio::test]
async fn tokens_secret_listing_revoke_and_admin_only() {
    let data_dir = temp_dir("tokens");
    let admin = init_admin(&data_dir);
    let mut server = Server::start(&data_dir, true).await;
    let alpha = create_tenant(&server, &admin, "Alpha").await;
    let (owner, _) = create_token(&server, &admin, &alpha, "owner", "owner").await;

    let list_denied = server.get("/api/enterprise/tokens", Some(&owner)).await;
    assert_eq!(list_denied.status, 403, "{}", list_denied.body);
    assert_eq!(
        list_denied.json()["error"]["code"],
        "PLATFORM_ADMIN_REQUIRED"
    );
    let create_denied = server
        .post(
            "/api/enterprise/tokens",
            Some(&owner),
            json!({"tenantId": alpha, "name": "nope", "role": "viewer"}),
        )
        .await;
    assert_eq!(create_denied.status, 403, "{}", create_denied.body);
    assert_eq!(
        create_denied.json()["error"]["code"],
        "PLATFORM_ADMIN_REQUIRED"
    );

    let created = server
        .post(
            "/api/enterprise/tokens",
            Some(&admin),
            json!({"tenantId": alpha, "name": "temporary", "role": "viewer"}),
        )
        .await;
    assert_eq!(created.status, 200, "token create failed: {}", created.body);
    let secret = created.json()["token"]
        .as_str()
        .expect("token secret")
        .to_string();
    assert!(secret.starts_with("hpa_ent_"));
    assert_eq!(
        created.body.matches(&secret).count(),
        1,
        "token secret must be returned exactly once: {}",
        created.body
    );
    let token_id = created.json()["tokenInfo"]["id"]
        .as_str()
        .expect("token id")
        .to_string();

    let list = server.get("/api/enterprise/tokens", Some(&admin)).await;
    assert_eq!(list.status, 200, "token list failed: {}", list.body);
    assert!(
        list.body.contains(&token_id),
        "token list missing id: {}",
        list.body
    );
    assert!(
        !list.body.contains(&secret) && !list.body.contains("hpa_ent_"),
        "token list leaked a secret: {}",
        list.body
    );

    let revoke_denied = server
        .delete(
            &format!("/api/enterprise/tokens/{token_id}"),
            Some(&owner),
            json!({}),
        )
        .await;
    assert_eq!(revoke_denied.status, 403, "{}", revoke_denied.body);
    assert_eq!(
        revoke_denied.json()["error"]["code"],
        "PLATFORM_ADMIN_REQUIRED"
    );

    let revoked = server
        .delete(
            &format!("/api/enterprise/tokens/{token_id}"),
            Some(&admin),
            json!({}),
        )
        .await;
    assert_eq!(revoked.status, 200, "revoke failed: {}", revoked.body);
    assert_eq!(revoked.json()["revoked"], true);

    let old_token = server.get("/api/workflows", Some(&secret)).await;
    assert_eq!(
        old_token.status, 401,
        "revoked token still accepted: {}",
        old_token.body
    );
    assert_eq!(old_token.json()["error"]["code"], "UNAUTHORIZED");
    server.stop();
}

#[tokio::test]
async fn members_directory_crud_and_tenant_filter() {
    let data_dir = temp_dir("members");
    let admin = init_admin(&data_dir);
    let mut server = Server::start(&data_dir, true).await;
    let alpha = create_tenant(&server, &admin, "Alpha").await;
    let beta = create_tenant(&server, &admin, "Beta").await;
    let (owner, _) = create_token(&server, &admin, &alpha, "owner", "owner").await;

    let denied = server.get("/api/enterprise/members", Some(&owner)).await;
    assert_eq!(denied.status, 403, "{}", denied.body);
    assert_eq!(denied.json()["error"]["code"], "PLATFORM_ADMIN_REQUIRED");

    let created = server
        .post(
            "/api/enterprise/members",
            Some(&admin),
            json!({
                "tenantId": alpha,
                "subject": "oidc|ada",
                "displayName": "Ada",
                "email": "Ada@Example.COM",
                "role": "developer"
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "member create failed: {}",
        created.body
    );
    let member = created.json();
    assert_eq!(member["tenantId"], alpha);
    assert_eq!(member["subject"], "oidc|ada");
    assert_eq!(member["email"], "ada@example.com");
    assert_eq!(member["role"], "developer");
    assert_eq!(member["status"], "active");
    let member_id = member["id"].as_str().expect("member id").to_string();

    let duplicate = server
        .post(
            "/api/enterprise/members",
            Some(&admin),
            json!({"tenantId": alpha, "subject": "oidc|ada", "role": "viewer"}),
        )
        .await;
    assert_eq!(duplicate.status, 409, "{}", duplicate.body);
    assert_eq!(duplicate.json()["error"]["code"], "MEMBER_EXISTS");

    let beta_member = server
        .post(
            "/api/enterprise/members",
            Some(&admin),
            json!({"tenantId": beta, "subject": "oidc|bob", "role": "viewer"}),
        )
        .await;
    assert_eq!(beta_member.status, 200, "{}", beta_member.body);

    let all = server.get("/api/enterprise/members", Some(&admin)).await;
    assert_eq!(all.status, 200, "{}", all.body);
    assert!(all.body.contains("oidc|ada") && all.body.contains("oidc|bob"));
    assert!(!all.body.contains("Ada@Example.COM"));

    let alpha_only = server
        .get(
            &format!("/api/enterprise/members?tenantId={alpha}"),
            Some(&admin),
        )
        .await;
    assert_eq!(alpha_only.status, 200, "{}", alpha_only.body);
    assert!(alpha_only.body.contains("oidc|ada"));
    assert!(!alpha_only.body.contains("oidc|bob"));

    let updated = server
        .put(
            &format!("/api/enterprise/members/{member_id}"),
            Some(&admin),
            json!({"displayName": "Ada Lovelace", "role": "operator", "status": "disabled"}),
        )
        .await;
    assert_eq!(updated.status, 200, "{}", updated.body);
    assert_eq!(updated.json()["displayName"], "Ada Lovelace");
    assert_eq!(updated.json()["role"], "operator");
    assert_eq!(updated.json()["status"], "disabled");

    let invalid_status = server
        .put(
            &format!("/api/enterprise/members/{member_id}"),
            Some(&admin),
            json!({"status": "suspended"}),
        )
        .await;
    assert_eq!(invalid_status.status, 400, "{}", invalid_status.body);
    assert_eq!(
        invalid_status.json()["error"]["code"],
        "MEMBER_STATUS_INVALID"
    );
    let invalid_role = server
        .put(
            &format!("/api/enterprise/members/{member_id}"),
            Some(&admin),
            json!({"role": "superuser"}),
        )
        .await;
    assert_eq!(invalid_role.status, 400, "{}", invalid_role.body);
    assert_eq!(invalid_role.json()["error"]["code"], "ROLE_INVALID");

    let deleted = server
        .delete(
            &format!("/api/enterprise/members/{member_id}"),
            Some(&admin),
            json!({}),
        )
        .await;
    assert_eq!(deleted.status, 200, "{}", deleted.body);
    assert_eq!(deleted.json()["deleted"], true);

    let leftover = server
        .post(
            "/api/enterprise/members",
            Some(&admin),
            json!({"tenantId": alpha, "subject": "oidc|leftover", "role": "viewer"}),
        )
        .await;
    assert_eq!(leftover.status, 200, "{}", leftover.body);
    let removed = server
        .delete(
            &format!("/api/enterprise/tenants/{alpha}"),
            Some(&admin),
            json!({"confirm": true}),
        )
        .await;
    assert_eq!(removed.status, 200, "{}", removed.body);
    let after_delete = server
        .get(
            &format!("/api/enterprise/members?tenantId={alpha}"),
            Some(&admin),
        )
        .await;
    assert_eq!(after_delete.status, 200, "{}", after_delete.body);
    assert!(
        !after_delete.body.contains("oidc|leftover"),
        "tenant delete left members: {}",
        after_delete.body
    );

    let audit = server
        .get("/api/enterprise/audit?limit=100", Some(&admin))
        .await;
    assert_eq!(audit.status, 200, "{}", audit.body);
    for action in ["member.create", "member.update", "member.delete"] {
        assert!(audit.body.contains(action), "audit missing {action}");
    }
    server.stop();
}

#[tokio::test]
async fn quota_max_workflows_and_audit() {
    let data_dir = temp_dir("quota");
    let admin = init_admin(&data_dir);
    let mut server = Server::start(&data_dir, true).await;
    let alpha = create_tenant(&server, &admin, "Alpha").await;
    let (owner, _) = create_token(&server, &admin, &alpha, "owner", "owner").await;

    let updated = server
        .put(
            &format!("/api/enterprise/quotas/{alpha}"),
            Some(&admin),
            quota_body(0, 8),
        )
        .await;
    assert_eq!(updated.status, 200, "quota update failed: {}", updated.body);
    assert_eq!(updated.json()["maxWorkflows"], 0);

    let denied = server
        .post(
            "/api/workflows",
            Some(&owner),
            json!({"ir": fixture_workflow("01_sequence.json")}),
        )
        .await;
    assert_eq!(denied.status, 429, "quota denial expected: {}", denied.body);
    assert_eq!(denied.json()["error"]["code"], "TENANT_QUOTA");

    let audit = server
        .get(
            &format!("/api/enterprise/audit?tenant_id={alpha}"),
            Some(&admin),
        )
        .await;
    assert_eq!(audit.status, 200, "audit failed: {}", audit.body);
    let entries = audit.json();
    let rows = entries["entries"].as_array().expect("audit entries");
    let quota_denial = rows.iter().find(|row| {
        row["action"] == "quota.check"
            && row["decision"] == "deny"
            && row["tenantId"] == alpha.as_str()
    });
    let quota_denial = quota_denial.expect("quota denial audit entry missing");
    assert_eq!(quota_denial["reason"], "tenant quota exceeded");
    assert_eq!(quota_denial["detail"]["limit"], 0);

    let restored = server
        .put(
            &format!("/api/enterprise/quotas/{alpha}"),
            Some(&admin),
            quota_body(100, 8),
        )
        .await;
    assert_eq!(
        restored.status, 200,
        "quota restore failed: {}",
        restored.body
    );
    assert_eq!(restored.json()["maxWorkflows"], 100);

    let created = server
        .post(
            "/api/workflows",
            Some(&owner),
            json!({"ir": fixture_workflow("01_sequence.json")}),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "create after quota restore failed: {}",
        created.body
    );
    server.stop();
}

#[tokio::test]
async fn audit_entries_and_csv_export() {
    let data_dir = temp_dir("audit");
    let admin = init_admin(&data_dir);
    let mut server = Server::start(&data_dir, true).await;
    let alpha = create_tenant(&server, &admin, "Alpha").await;
    let (_owner, _) = create_token(&server, &admin, &alpha, "owner", "owner").await;
    let (viewer, _) = create_token(&server, &admin, &alpha, "viewer", "viewer").await;

    let denied = server
        .post(
            "/api/workflows",
            Some(&viewer),
            json!({"ir": fixture_workflow("01_sequence.json")}),
        )
        .await;
    assert_eq!(denied.status, 403, "{}", denied.body);

    let audit = server
        .get(
            &format!("/api/enterprise/audit?tenant_id={alpha}"),
            Some(&admin),
        )
        .await;
    assert_eq!(audit.status, 200, "audit failed: {}", audit.body);
    let value = audit.json();
    let rows = value["entries"].as_array().expect("audit entries");
    assert!(rows.iter().all(|row| row.get("decision").is_some()));
    assert!(rows.iter().all(|row| row.get("reason").is_some()));
    let rbac_denial = rows
        .iter()
        .find(|row| row["action"] == "POST /workflows" && row["decision"] == "deny")
        .expect("rbac denial audit entry missing");
    assert_eq!(rbac_denial["reason"], "rbac denied");
    assert_eq!(rbac_denial["detail"]["role"], "viewer");
    let tenant_create = rows
        .iter()
        .find(|row| row["action"] == "tenant.create" && row["decision"] == "allow");
    assert!(tenant_create.is_some(), "tenant.create audit entry missing");

    let csv = server
        .get(
            &format!("/api/enterprise/audit/export?format=csv&tenant_id={alpha}"),
            Some(&admin),
        )
        .await;
    assert_eq!(csv.status, 200, "csv export failed: {}", csv.body);
    let content_type = csv.content_type.clone().unwrap_or_default();
    assert!(
        content_type.starts_with("text/csv"),
        "unexpected content type: {content_type}"
    );
    let header = csv.body.lines().next().unwrap_or_default();
    assert_eq!(
        header, "at,tenantId,actorId,actorType,action,resource,decision,reason",
        "unexpected csv header: {header}"
    );
    assert!(
        csv.body.contains("\"deny\""),
        "csv missing denial: {}",
        csv.body
    );
    assert!(
        !csv.body.contains("hpa_ent_") && !csv.body.contains(&viewer),
        "csv leaked a tenant secret: {}",
        csv.body
    );
    server.stop();
}

#[tokio::test]
async fn tenant_lifecycle_delete_and_protected_local() {
    let data_dir = temp_dir("lifecycle");
    let admin = init_admin(&data_dir);
    let mut server = Server::start(&data_dir, true).await;

    let local = server
        .delete(
            "/api/enterprise/tenants/local",
            Some(&admin),
            json!({"confirm": true}),
        )
        .await;
    assert_eq!(
        local.status, 409,
        "local delete not blocked: {}",
        local.body
    );
    assert_eq!(local.json()["error"]["code"], "TENANT_PROTECTED");

    let disposable = create_tenant(&server, &admin, "Disposable").await;
    let (token, _) = create_token(&server, &admin, &disposable, "disposable", "owner").await;
    let before = server.get("/api/workflows", Some(&token)).await;
    assert_eq!(before.status, 200, "tenant should exist: {}", before.body);

    let deleted = server
        .delete(
            &format!("/api/enterprise/tenants/{disposable}"),
            Some(&admin),
            json!({"confirm": true}),
        )
        .await;
    assert_eq!(deleted.status, 200, "delete failed: {}", deleted.body);
    assert_eq!(deleted.json()["deleted"], true);

    let after = server.get("/api/workflows", Some(&token)).await;
    assert_eq!(
        after.status, 401,
        "deleted tenant token still accepted: {}",
        after.body
    );

    let tenants = server.get("/api/enterprise/tenants", Some(&admin)).await;
    assert_eq!(tenants.status, 200, "tenant list failed: {}", tenants.body);
    assert!(
        !tenants.body.contains(&disposable),
        "deleted tenant still listed: {}",
        tenants.body
    );
    server.stop();
}

#[tokio::test]
async fn concurrent_run_quota_enforced() {
    let data_dir = temp_dir("concurrency");
    let admin = init_admin(&data_dir);
    let mut server = Server::start(&data_dir, true).await;
    let alpha = create_tenant(&server, &admin, "Alpha").await;
    let (owner, _) = create_token(&server, &admin, &alpha, "owner", "owner").await;

    let created = server
        .post(
            "/api/workflows",
            Some(&owner),
            json!({"ir": fixture_workflow("01_sequence.json")}),
        )
        .await;
    assert_eq!(created.status, 200, "create failed: {}", created.body);

    let updated = server
        .put(
            &format!("/api/enterprise/quotas/{alpha}"),
            Some(&admin),
            quota_body(100, 0),
        )
        .await;
    assert_eq!(updated.status, 200, "quota update failed: {}", updated.body);

    let blocked = server
        .post(
            "/api/workflows/wf_fixture_sequence/executions",
            Some(&owner),
            json!({"input": {"value": 1}}),
        )
        .await;
    assert_eq!(
        blocked.status, 429,
        "concurrency denial expected: {}",
        blocked.body
    );
    assert_eq!(blocked.json()["error"]["code"], "TENANT_CONCURRENCY");
    server.stop();
}

#[tokio::test]
async fn worker_daemon_executes_remote_workflow_and_resumes_approval() {
    let data_dir = temp_dir("worker-daemon");
    let admin = init_admin(&data_dir);
    let mut server = Server::start(&data_dir, false).await;

    let sequence = server
        .post(
            "/api/workflows",
            None,
            json!({"ir": fixture_workflow("01_sequence.json")}),
        )
        .await;
    assert_eq!(sequence.status, 200, "{}", sequence.body);
    let approval = server
        .post(
            "/api/workflows",
            None,
            json!({"ir": fixture_workflow("08_approval.json")}),
        )
        .await;
    assert_eq!(approval.status, 200, "{}", approval.body);

    let worker = server
        .post(
            "/api/enterprise/workers",
            Some(&admin),
            json!({
                "tenantId": "local",
                "name": "daemon-test",
                "kind": "local",
                "capabilities": ["workflow"],
                "maxConcurrency": 2
            }),
        )
        .await;
    assert_eq!(worker.status, 200, "{}", worker.body);
    let worker_token = worker.json()["workerToken"]
        .as_str()
        .expect("worker token")
        .to_string();
    assert!(worker_token.starts_with("hpa_wrk_"));

    let wrong_lease = server
        .post(
            "/api/worker/lease",
            Some("hpa_wrk_wrong"),
            json!({"slots": 1, "version": env!("CARGO_PKG_VERSION"), "capabilities": ["workflow"]}),
        )
        .await;
    assert_eq!(wrong_lease.status, 401, "{}", wrong_lease.body);

    let sequence_job = server
        .post(
            "/api/enterprise/worker-jobs",
            Some(&admin),
            json!({
                "tenantId": "local",
                "workflowId": "wf_fixture_sequence",
                "input": {"value": 41}
            }),
        )
        .await;
    assert_eq!(sequence_job.status, 200, "{}", sequence_job.body);
    let sequence_execution = sequence_job.json()["executionId"]
        .as_str()
        .expect("execution id")
        .to_string();

    let approval_job = server
        .post(
            "/api/enterprise/worker-jobs",
            Some(&admin),
            json!({
                "tenantId": "local",
                "workflowId": "wf_fixture_approval",
                "input": {}
            }),
        )
        .await;
    assert_eq!(approval_job.status, 200, "{}", approval_job.body);
    let approval_execution = approval_job.json()["executionId"]
        .as_str()
        .expect("execution id")
        .to_string();

    let daemon = Command::new(binary())
        .args([
            "worker",
            "--control-url",
            &server.url(""),
            "--max-concurrency",
            "2",
        ])
        .env("HPA_WORKER_TOKEN", &worker_token)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn worker daemon");
    let _daemon = ProcessGuard(daemon);

    let completed = wait_for_execution(&server, &sequence_execution, "succeeded").await;
    assert_eq!(completed["summary"]["workflowId"], "wf_fixture_sequence");

    let waiting = wait_for_execution(&server, &approval_execution, "waiting_approval").await;
    let node_id = waiting["summary"]["waiting"]["nodeId"]
        .as_str()
        .expect("approval node")
        .to_string();
    let decided = server
        .post(
            &format!("/api/executions/{approval_execution}/approvals"),
            None,
            json!({"nodeId": node_id, "decision": "approved"}),
        )
        .await;
    assert_eq!(decided.status, 200, "{}", decided.body);
    let approved = wait_for_execution(&server, &approval_execution, "succeeded").await;
    assert_eq!(approved["summary"]["workflowId"], "wf_fixture_approval");

    let pending_job = server
        .post(
            "/api/enterprise/worker-jobs",
            Some(&admin),
            json!({
                "tenantId": "local",
                "workflowId": "wf_fixture_approval",
                "input": {}
            }),
        )
        .await;
    assert_eq!(pending_job.status, 200, "{}", pending_job.body);
    let pending_job_id = pending_job.json()["id"]
        .as_str()
        .expect("pending job id")
        .to_string();
    let pending_execution = pending_job.json()["executionId"]
        .as_str()
        .expect("pending execution id")
        .to_string();
    let waiting = wait_for_execution(&server, &pending_execution, "waiting_approval").await;
    assert_eq!(waiting["summary"]["workflowId"], "wf_fixture_approval");
    let cancelled = server
        .post(
            &format!("/api/enterprise/worker-jobs/{pending_job_id}/cancel"),
            Some(&admin),
            json!({}),
        )
        .await;
    assert_eq!(cancelled.status, 200, "{}", cancelled.body);
    assert_eq!(cancelled.json()["status"], "cancelled");
    let cancelled_execution = wait_for_execution(&server, &pending_execution, "cancelled").await;
    assert_eq!(cancelled_execution["summary"]["status"], "cancelled");

    let jobs = server
        .get("/api/enterprise/worker-jobs?tenantId=local", Some(&admin))
        .await;
    assert_eq!(jobs.status, 200, "{}", jobs.body);
    let job_values = jobs.json();
    let job_list = job_values["jobs"].as_array().expect("jobs");
    assert_eq!(job_list.len(), 3);
    assert_eq!(
        job_list
            .iter()
            .filter(|job| job["status"] == "succeeded")
            .count(),
        2
    );
    assert_eq!(
        job_list
            .iter()
            .filter(|job| job["status"] == "cancelled")
            .count(),
        1
    );
    assert!(job_list.iter().all(|job| job["workerId"].is_string()));

    server.stop();
}
