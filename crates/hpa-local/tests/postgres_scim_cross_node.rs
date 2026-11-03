use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use reqwest::Method;
use serde_json::{Value, json};
use sqlx::Row;
use sqlx::sqlite::SqlitePoolOptions;

const USER_SCHEMA: &str = "urn:ietf:params:scim:schemas:core:2.0:User";
const GROUP_SCHEMA: &str = "urn:ietf:params:scim:schemas:core:2.0:Group";
const PATCH_SCHEMA: &str = "urn:ietf:params:scim:api:messages:2.0:PatchOp";
const ROLE_SCHEMA: &str = "urn:hpa:params:scim:schemas:extension:role:1.0";

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hpa")
}

fn temp_dir(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("hpa-pg-scim-{label}-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&path).expect("temp dir");
    path
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("addr")
        .port()
}

fn run_cli(data_dir: &Path, args: &[&str], database_url: &str) -> String {
    let output = Command::new(binary())
        .arg("--data-dir")
        .arg(data_dir)
        .args(args)
        .env("HPA_POSTGRES_URL", database_url)
        .output()
        .expect("run cli");
    assert!(
        output.status.success(),
        "cli `{args:?}` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

struct Server {
    child: Child,
    port: u16,
    client: reqwest::Client,
}

impl Server {
    async fn start(data_dir: &Path, database_url: &str) -> Self {
        let port = free_port();
        let child = Command::new(binary())
            .arg("--data-dir")
            .arg(data_dir)
            .args(["serve", "--port", &port.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .env("HPA_POSTGRES_URL", database_url)
            .env("HPA_POSTGRES_RUNTIME", "1")
            .env("HPA_ENTERPRISE_MODE", "required")
            .spawn()
            .expect("spawn server");
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("http client");
        let mut healthy = false;
        for _ in 0..400 {
            if let Ok(response) = client
                .get(format!("http://127.0.0.1:{port}/api/health"))
                .send()
                .await
                && response.status().is_success()
            {
                healthy = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if !healthy {
            let mut child = child;
            let _ = child.kill();
            let _ = child.wait();
            panic!("server did not become healthy");
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

    async fn scim(
        &self,
        method: Method,
        path: &str,
        token: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let mut request = self
            .client
            .request(method, self.url(path))
            .bearer_auth(token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.expect("SCIM request");
        let status = response.status().as_u16();
        let bytes = response.bytes().await.expect("SCIM response body");
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, value)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn sqlite_count(path: &Path, table: &str) -> i64 {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&format!("sqlite://{}", path.display()))
        .await
        .expect("open sqlite");
    let query = format!("SELECT count(*) FROM {table}");
    let count: i64 = sqlx::query(sqlx::AssertSqlSafe(query))
        .fetch_one(&pool)
        .await
        .expect("count rows")
        .get(0);
    pool.close().await;
    count
}

#[tokio::test]
async fn scim_directory_is_authoritative_in_postgres_across_nodes() {
    let Ok(database_url) = std::env::var("HPA_POSTGRES_TEST_URL") else {
        eprintln!("skipped: HPA_POSTGRES_TEST_URL is not set");
        return;
    };
    let dir_a = temp_dir("a");
    run_cli(&dir_a, &["db", "migrate"], &database_url);
    let initialized: Value =
        serde_json::from_str(&run_cli(&dir_a, &["enterprise", "init"], &database_url))
            .expect("init json");
    let token = initialized["token"]
        .as_str()
        .expect("init token")
        .to_string();
    let dir_b = temp_dir("b");
    let server_a = Server::start(&dir_a, &database_url).await;
    let server_b = Server::start(&dir_b, &database_url).await;

    let user_name = "cross.node@example.test";
    let (status, created) = server_a
        .scim(
            Method::POST,
            "/scim/v2/Users",
            &token,
            Some(json!({
                "schemas": [USER_SCHEMA],
                "userName": user_name,
                "displayName": "Cross Node User",
                "emails": [{"value": user_name, "primary": true}]
            })),
        )
        .await;
    assert_eq!(status, 201, "{created}");
    let user_id = created["id"].as_str().expect("user id").to_string();
    assert_eq!(created[ROLE_SCHEMA]["effectiveRole"], json!("viewer"));

    let (status, listed) = server_b
        .scim(
            Method::GET,
            "/scim/v2/Users?filter=userName%20eq%20%22cross.node%40example.test%22",
            &token,
            None,
        )
        .await;
    assert_eq!(status, 200, "{listed}");
    assert_eq!(listed["totalResults"], json!(1), "{listed}");
    assert_eq!(listed["Resources"][0]["id"], json!(user_id));

    let (status, group) = server_a
        .scim(
            Method::POST,
            "/scim/v2/Groups",
            &token,
            Some(json!({
                "schemas": [GROUP_SCHEMA, ROLE_SCHEMA],
                "displayName": "Cross Developers",
                ROLE_SCHEMA: {"role": "developer"},
                "members": [{"value": user_id}]
            })),
        )
        .await;
    assert_eq!(status, 201, "{group}");
    let group_id = group["id"].as_str().expect("group id").to_string();

    let (status, groups_on_b) = server_b
        .scim(Method::GET, "/scim/v2/Groups", &token, None)
        .await;
    assert_eq!(status, 200, "{groups_on_b}");
    assert_eq!(groups_on_b["totalResults"], json!(1), "{groups_on_b}");
    assert_eq!(groups_on_b["Resources"][0]["id"], json!(group_id));

    let (status, user_on_b) = server_b
        .scim(
            Method::GET,
            &format!("/scim/v2/Users/{user_id}"),
            &token,
            None,
        )
        .await;
    assert_eq!(status, 200, "{user_on_b}");
    assert_eq!(
        user_on_b[ROLE_SCHEMA]["effectiveRole"],
        json!("developer"),
        "{user_on_b}"
    );
    let groups = user_on_b["groups"].as_array().expect("user groups");
    assert!(
        groups.iter().any(|group| group["value"] == json!(group_id)),
        "{user_on_b}"
    );

    let (status, patched) = server_b
        .scim(
            Method::PATCH,
            &format!("/scim/v2/Users/{user_id}"),
            &token,
            Some(json!({
                "schemas": [PATCH_SCHEMA],
                "Operations": [{"op": "replace", "path": "active", "value": false}]
            })),
        )
        .await;
    assert_eq!(status, 200, "{patched}");
    assert_eq!(patched["active"], json!(false), "{patched}");

    let (status, user_on_a) = server_a
        .scim(
            Method::GET,
            &format!("/scim/v2/Users/{user_id}"),
            &token,
            None,
        )
        .await;
    assert_eq!(status, 200, "{user_on_a}");
    assert_eq!(user_on_a["active"], json!(false), "{user_on_a}");
    assert_eq!(user_on_a[ROLE_SCHEMA]["effectiveRole"], json!("developer"));

    let (status, deleted) = server_a
        .scim(
            Method::DELETE,
            &format!("/scim/v2/Groups/{group_id}"),
            &token,
            None,
        )
        .await;
    assert_eq!(status, 204, "{deleted}");

    let (status, fallback) = server_b
        .scim(
            Method::GET,
            &format!("/scim/v2/Users/{user_id}"),
            &token,
            None,
        )
        .await;
    assert_eq!(status, 200, "{fallback}");
    assert_eq!(fallback[ROLE_SCHEMA]["role"], json!("viewer"), "{fallback}");
    assert_eq!(
        fallback[ROLE_SCHEMA]["effectiveRole"],
        json!("viewer"),
        "{fallback}"
    );
    assert!(
        fallback.get("groups").is_none() || fallback["groups"].is_null(),
        "{fallback}"
    );

    let (status, deleted_user) = server_b
        .scim(
            Method::DELETE,
            &format!("/scim/v2/Users/{user_id}"),
            &token,
            None,
        )
        .await;
    assert_eq!(status, 204, "{deleted_user}");

    for dir in [&dir_a, &dir_b] {
        for table in [
            "enterprise_members",
            "enterprise_scim_groups",
            "enterprise_scim_group_members",
        ] {
            assert_eq!(
                sqlite_count(&dir.join("control.db"), table).await,
                0,
                "{table} must live in PostgreSQL"
            );
        }
    }

    drop(server_a);
    drop(server_b);
    let _ = std::fs::remove_dir_all(&dir_a);
    let _ = std::fs::remove_dir_all(&dir_b);
}
