#[path = "../src/enterprise_ops.rs"]
mod enterprise_ops;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

use enterprise_ops::{OpsService, WORKER_OFFLINE_MS};

fn temp_root(label: &str) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("hpa-enterprise-{label}-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&root).unwrap();
    root
}

async fn control_pool() -> SqlitePool {
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap()
}

async fn seed_tenant_db(path: &Path, value: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    sqlx::query("CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v TEXT NOT NULL)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO kv (k, v) VALUES ('k', ?) ON CONFLICT(k) DO UPDATE SET v = excluded.v",
    )
    .bind(value)
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;
}

async fn read_tenant_value(path: &Path) -> String {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    let value: String = sqlx::query_scalar("SELECT v FROM kv WHERE k = 'k'")
        .fetch_one(&pool)
        .await
        .unwrap();
    pool.close().await;
    value
}

fn epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn epoch_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[tokio::test]
async fn backups_restore_purge_delete() {
    let root = temp_root("backups");
    let data_dir = root.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let local_db = data_dir.join("hpa.db");
    seed_tenant_db(&local_db, "original").await;
    let ops = OpsService::new(control_pool().await, data_dir.clone());
    ops.ensure_schema().await.unwrap();

    assert!(ops.create_backup("../escape").await.is_err());
    assert!(ops.create_backup("bad/tenant").await.is_err());

    let first = ops.create_backup("local").await.unwrap();
    assert_eq!(first["tenantId"], "local");
    assert_eq!(first["status"], "ready");
    let first_path = PathBuf::from(first["path"].as_str().unwrap());
    assert!(first_path.starts_with(data_dir.join("backups")));
    assert!(first_path.is_file());
    assert!(first["size"].as_i64().unwrap() > 0);
    assert_eq!(first["sha256"].as_str().unwrap().len(), 64);
    assert_eq!(ops.list_backups(Some("local")).await.unwrap().len(), 1);
    assert_eq!(ops.list_backups(None).await.unwrap().len(), 1);

    seed_tenant_db(&local_db, "mutated").await;
    std::fs::write(format!("{}-wal", local_db.display()), b"stale").unwrap();
    std::fs::write(format!("{}-shm", local_db.display()), b"stale").unwrap();
    let restored = ops
        .restore_backup(first["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(restored["restored"], json!(true));
    assert_eq!(restored["requiresRestart"], json!(true));
    assert_eq!(read_tenant_value(&local_db).await, "original");
    assert!(first_path.is_file());
    assert!(!PathBuf::from(format!("{}-wal", local_db.display())).exists());
    assert!(!PathBuf::from(format!("{}-shm", local_db.display())).exists());

    let second = ops.create_backup("local").await.unwrap();
    let third = ops.create_backup("local").await.unwrap();
    let fourth = ops.create_backup("local").await.unwrap();
    let now = epoch_ms();
    let day = 86_400_000i64;
    for (backup, age_days) in [(&first, 0i64), (&second, 31), (&third, 45), (&fourth, 60)] {
        sqlx::query("UPDATE enterprise_backups SET created_at = ? WHERE id = ?")
            .bind(now - age_days * day)
            .bind(backup["id"].as_str().unwrap())
            .execute(&ops.control_pool)
            .await
            .unwrap();
    }
    let all = ops.list_backups(Some("local")).await.unwrap();
    assert_eq!(all.len(), 4);
    let paths: Vec<PathBuf> = all
        .iter()
        .map(|backup| PathBuf::from(backup["path"].as_str().unwrap()))
        .collect();
    let removed = ops.purge_backups(30, 2).await.unwrap();
    assert_eq!(removed, 2);
    let left = ops.list_backups(Some("local")).await.unwrap();
    assert_eq!(left.len(), 2);
    assert_eq!(paths.iter().filter(|path| path.exists()).count(), 2);
    assert!(ops.purge_backups(30, 2).await.unwrap() == 0);

    let next_id = left[0]["id"].as_str().unwrap().to_string();
    let next_path = PathBuf::from(left[0]["path"].as_str().unwrap());
    assert!(ops.delete_backup(&next_id).await.unwrap());
    assert!(!next_path.exists());
    assert!(!ops.delete_backup(&next_id).await.unwrap());
    assert!(ops.restore_backup(&next_id).await.is_err());
    assert_eq!(ops.list_backups(Some("local")).await.unwrap().len(), 1);

    let tenant_db = data_dir.join("tenants").join("acme").join("hpa.db");
    seed_tenant_db(&tenant_db, "acme").await;
    let acme = ops.create_backup("acme").await.unwrap();
    assert_eq!(acme["tenantId"], "acme");
    assert!(
        acme["path"]
            .as_str()
            .unwrap()
            .contains("backups/acme/backup-")
    );
    assert_eq!(ops.list_backups(Some("acme")).await.unwrap().len(), 1);
    assert_eq!(ops.list_backups(None).await.unwrap().len(), 2);

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn workers_register_heartbeat_offline_disable() {
    let root = temp_root("workers");
    let data_dir = root.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let ops = OpsService::new(control_pool().await, data_dir);
    ops.ensure_schema().await.unwrap();

    assert!(
        ops.register_worker("../bad", "n", "runner", json!({}))
            .await
            .is_err()
    );
    assert!(
        ops.register_worker("acme", "", "runner", json!({}))
            .await
            .is_err()
    );
    assert!(
        ops.register_worker("acme", "name", "runner", json!([1, 2]))
            .await
            .is_err()
    );

    let worker = ops
        .register_worker("acme", "builder", "runner", json!({"zone": "a"}))
        .await
        .unwrap();
    let id = worker["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("worker_"));
    assert_eq!(worker["tenantId"], "acme");
    assert_eq!(worker["status"], "active");
    assert_eq!(worker["metadata"]["zone"], "a");
    assert_eq!(ops.list_workers(Some("acme")).await.unwrap().len(), 1);
    assert_eq!(ops.list_workers(None).await.unwrap().len(), 1);

    let beat = ops.heartbeat_worker(&id).await.unwrap();
    assert_eq!(beat["status"], "active");

    let stale = epoch_ms() - WORKER_OFFLINE_MS - 5_000;
    sqlx::query("UPDATE enterprise_workers SET last_heartbeat_at = ? WHERE id = ?")
        .bind(stale)
        .bind(&id)
        .execute(&ops.control_pool)
        .await
        .unwrap();
    let listed = ops.list_workers(Some("acme")).await.unwrap();
    assert_eq!(listed[0]["status"], "offline");
    assert_eq!(listed[0]["lastHeartbeatAt"], json!(stale));
    let stored: String = sqlx::query_scalar("SELECT status FROM enterprise_workers WHERE id = ?")
        .bind(&id)
        .fetch_one(&ops.control_pool)
        .await
        .unwrap();
    assert_eq!(stored, "active");

    let revived = ops.heartbeat_worker(&id).await.unwrap();
    assert_eq!(revived["status"], "active");
    assert_eq!(
        ops.list_workers(Some("acme")).await.unwrap()[0]["status"],
        "active"
    );

    let disabled = ops.disable_worker(&id).await.unwrap();
    assert_eq!(disabled["status"], "disabled");
    assert_eq!(
        ops.list_workers(Some("acme")).await.unwrap()[0]["status"],
        "disabled"
    );
    let beat_after_disable = ops.heartbeat_worker(&id).await.unwrap();
    assert_eq!(beat_after_disable["status"], "disabled");
    assert!(ops.disable_worker("worker_missing").await.is_err());
    assert!(ops.heartbeat_worker("worker_missing").await.is_err());

    let null_meta = ops
        .register_worker("acme", "plain", "runner", Value::Null)
        .await
        .unwrap();
    assert_eq!(null_meta["metadata"], Value::Null);

    std::fs::remove_dir_all(&root).ok();
}

fn make_token(signing_key: &SigningKey, header: Value, claims: Value) -> String {
    let header = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).unwrap());
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
    let signing_input = format!("{header}.{payload}");
    let signature = signing_key.sign(signing_input.as_bytes());
    format!(
        "{signing_input}.{}",
        URL_SAFE_NO_PAD.encode(signature.to_bytes())
    )
}

fn jwks_body(signing_key: &SigningKey, kid: &str) -> String {
    let x = URL_SAFE_NO_PAD.encode(signing_key.verifying_key().as_bytes());
    json!({
        "keys": [{
            "kty": "OKP",
            "crv": "Ed25519",
            "kid": kid,
            "alg": "EdDSA",
            "use": "sig",
            "x": x
        }]
    })
    .to_string()
}

fn start_jwks_server(body: String) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buffer = [0u8; 4096];
            let _ = stream.read(&mut buffer);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    format!("http://{address}/jwks")
}

fn claims(
    issuer: &str,
    audience: Value,
    subject: &str,
    tenant: &str,
    role: Value,
    exp: i64,
    nbf: Option<i64>,
) -> Value {
    let mut claims = json!({
        "iss": issuer,
        "aud": audience,
        "sub": subject,
        "tenant_id": tenant,
        "role": role,
        "email": "ops@example.com",
        "exp": exp
    });
    if let Some(nbf) = nbf {
        claims["nbf"] = json!(nbf);
    }
    claims
}

#[tokio::test]
async fn oidc_verification_with_local_jwks() {
    let root = temp_root("oidc");
    let ops = OpsService::new(control_pool().await, root.join("data"));
    ops.ensure_schema().await.unwrap();

    let empty = ops.get_oidc_config().await.unwrap();
    assert_eq!(empty["issuer"], Value::Null);
    assert_eq!(empty["jwksUrl"], Value::Null);
    assert!(ops.verify_oidc_token("a.b.c").await.is_err());

    let signing_key = SigningKey::from_bytes(&[7u8; 32]);
    let other_key = SigningKey::from_bytes(&[9u8; 32]);
    let jwks_url = start_jwks_server(jwks_body(&signing_key, "primary"));
    let issuer = "https://issuer.example/";
    let audience = "hpa-control";
    let config = ops
        .set_oidc_config(issuer, audience, &jwks_url, vec!["*".to_string()])
        .await
        .unwrap();
    assert_eq!(config["issuer"], issuer);
    assert_eq!(config["audience"], audience);
    assert_eq!(config["jwksUrl"], jwks_url);
    assert_eq!(config["network"], json!(["*"]));
    assert!(config["updatedAt"].as_i64().unwrap() > 0);

    let header = json!({"alg": "EdDSA", "typ": "JWT", "kid": "primary"});
    let now = epoch_seconds();
    let valid_claims = claims(
        issuer,
        json!(audience),
        "user-1",
        "acme",
        json!("owner"),
        now + 3_600,
        Some(now - 10),
    );
    let valid = make_token(&signing_key, header.clone(), valid_claims);
    let principal = ops.verify_oidc_token(&valid).await.unwrap();
    assert_eq!(principal.subject, "user-1");
    assert_eq!(principal.tenant_id, "acme");
    assert_eq!(principal.role, "owner");
    assert_eq!(principal.email.as_deref(), Some("ops@example.com"));

    let array_aud = make_token(
        &signing_key,
        header.clone(),
        claims(
            issuer,
            json!(["other", audience]),
            "user-2",
            "acme",
            json!(["viewer", "admin"]),
            now + 3_600,
            None,
        ),
    );
    let principal = ops.verify_oidc_token(&array_aud).await.unwrap();
    assert_eq!(principal.role, "viewer");
    assert_eq!(principal.tenant_id, "acme");

    let tenant_camel = make_token(
        &signing_key,
        header.clone(),
        json!({
            "iss": issuer,
            "aud": audience,
            "sub": "user-3",
            "tenantId": "beta",
            "roles": ["auditor"],
            "exp": now + 3_600
        }),
    );
    let principal = ops.verify_oidc_token(&tenant_camel).await.unwrap();
    assert_eq!(principal.tenant_id, "beta");
    assert_eq!(principal.role, "auditor");
    assert!(principal.email.is_none());

    let wrong_audience = make_token(
        &signing_key,
        header.clone(),
        claims(
            issuer,
            json!("someone-else"),
            "user-1",
            "acme",
            json!("owner"),
            now + 3_600,
            None,
        ),
    );
    assert!(ops.verify_oidc_token(&wrong_audience).await.is_err());

    let expired = make_token(
        &signing_key,
        header.clone(),
        claims(
            issuer,
            json!(audience),
            "user-1",
            "acme",
            json!("owner"),
            now - 3_600,
            None,
        ),
    );
    assert!(ops.verify_oidc_token(&expired).await.is_err());

    let not_yet_valid = make_token(
        &signing_key,
        header.clone(),
        claims(
            issuer,
            json!(audience),
            "user-1",
            "acme",
            json!("owner"),
            now + 3_600,
            Some(now + 600),
        ),
    );
    assert!(ops.verify_oidc_token(&not_yet_valid).await.is_err());

    let bad_signature = make_token(
        &other_key,
        header.clone(),
        claims(
            issuer,
            json!(audience),
            "user-1",
            "acme",
            json!("owner"),
            now + 3_600,
            None,
        ),
    );
    assert!(ops.verify_oidc_token(&bad_signature).await.is_err());

    let bad_alg = make_token(
        &signing_key,
        json!({"alg": "HS256", "typ": "JWT", "kid": "primary"}),
        claims(
            issuer,
            json!(audience),
            "user-1",
            "acme",
            json!("owner"),
            now + 3_600,
            None,
        ),
    );
    assert!(ops.verify_oidc_token(&bad_alg).await.is_err());

    let unknown_role = make_token(
        &signing_key,
        header.clone(),
        claims(
            issuer,
            json!(audience),
            "user-1",
            "acme",
            json!("superuser"),
            now + 3_600,
            None,
        ),
    );
    assert!(ops.verify_oidc_token(&unknown_role).await.is_err());

    ops.set_oidc_config(
        issuer,
        audience,
        "https://jwks.example.com/jwks",
        vec!["allowed.example".to_string()],
    )
    .await
    .unwrap();
    assert!(ops.verify_oidc_token(&valid).await.is_err());
    assert!(ops.get_oidc_config().await.unwrap()["issuer"] == issuer);

    std::fs::remove_dir_all(&root).ok();
}
