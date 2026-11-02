use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hpa")
}

fn temp_dir(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("hpa-oauth-{label}-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&path).expect("temp dir");
    path
}

fn run_cli(data_dir: &Path, args: &[&str]) -> String {
    let output = Command::new(binary())
        .arg("--data-dir")
        .arg(data_dir)
        .args(args)
        .output()
        .expect("run cli");
    assert!(
        output.status.success(),
        "cli `{args:?}` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("addr")
        .port()
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
            .stderr(Stdio::null());
        if enterprise {
            command.env("HPA_ENTERPRISE_MODE", "required");
        } else {
            command.env_remove("HPA_ENTERPRISE_MODE");
        }
        let mut child = command.spawn().expect("spawn server");
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

    fn issuer(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    async fn register(&self, redirect_uris: &[String], auth_method: &str) -> Value {
        let response = self
            .client
            .post(self.url("/oauth/register"))
            .json(&json!({
                "client_name": "integration client",
                "redirect_uris": redirect_uris,
                "token_endpoint_auth_method": auth_method,
                "scope": "mcp:read mcp:execute offline_access",
            }))
            .send()
            .await
            .expect("register");
        assert_eq!(response.status(), 201, "registration must succeed");
        response.json().await.expect("register json")
    }

    async fn authorize(&self, query: &[(&str, &str)]) -> reqwest::Response {
        let mut url = reqwest::Url::parse(&self.url("/oauth/authorize")).expect("authorize url");
        {
            let mut pairs = url.query_pairs_mut();
            for (key, value) in query {
                pairs.append_pair(key, value);
            }
        }
        self.client.get(url).send().await.expect("authorize")
    }

    fn code_from_redirect(location: &str) -> String {
        let url = reqwest::Url::parse(location).expect("redirect url");
        url.query_pairs()
            .find(|(key, _)| key == "code")
            .map(|(_, value)| value.to_string())
            .expect("code in redirect")
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn pkce() -> (String, String) {
    let verifier = URL_SAFE_NO_PAD.encode(uuid::Uuid::now_v7().as_bytes())
        + &URL_SAFE_NO_PAD.encode(uuid::Uuid::now_v7().as_bytes());
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

fn mcp_discover_body() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "server/discover",
        "params": {"_meta": {
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientCapabilities": {},
            "io.modelcontextprotocol/clientInfo": {"name": "oauth-test", "version": "1"},
        }}
    })
}

fn redirect_param(location: &str, name: &str) -> Option<String> {
    let url = reqwest::Url::parse(location).ok()?;
    url.query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.to_string())
}

#[tokio::test]
async fn authorization_code_flow_issues_usable_mcp_token() {
    let data_dir = temp_dir("flow");
    let server = Server::start(&data_dir, false).await;
    let redirect_uri = "http://127.0.0.1:4173/callback".to_string();
    let client = server
        .register(std::slice::from_ref(&redirect_uri), "none")
        .await;
    let client_id = client["client_id"].as_str().expect("client id").to_string();
    assert!(
        client.get("client_secret").is_none(),
        "public client has no secret"
    );

    let metadata: Value = server
        .client
        .get(server.url("/.well-known/oauth-authorization-server"))
        .send()
        .await
        .expect("metadata")
        .json()
        .await
        .expect("metadata json");
    assert_eq!(metadata["issuer"], json!(server.issuer()));
    assert_eq!(
        metadata["code_challenge_methods_supported"],
        json!(["S256"])
    );
    assert_eq!(
        metadata["authorization_endpoint"],
        json!(format!("{}/oauth/authorize", server.issuer()))
    );

    let jwks: Value = server
        .client
        .get(server.url("/oauth/jwks"))
        .send()
        .await
        .expect("jwks")
        .json()
        .await
        .expect("jwks json");
    assert_eq!(jwks["keys"][0]["alg"], json!("ES256"));
    assert!(jwks["keys"][0]["kid"].is_string());

    let (verifier, challenge) = pkce();
    let consent = server
        .authorize(&[
            ("response_type", "code"),
            ("client_id", &client_id),
            ("redirect_uri", &redirect_uri),
            ("scope", "mcp:read mcp:execute"),
            ("state", "state-1"),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
        ])
        .await;
    assert_eq!(
        consent.status(),
        303,
        "first authorize must ask for consent"
    );
    let location = consent
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .expect("consent location")
        .to_string();
    let request_id = location
        .split("request=")
        .nth(1)
        .expect("request id")
        .to_string();

    let detail: Value = server
        .client
        .get(server.url(&format!("/api/oauth/requests/{request_id}")))
        .send()
        .await
        .expect("request detail")
        .json()
        .await
        .expect("detail json");
    assert_eq!(detail["clientName"], json!("integration client"));
    assert_eq!(detail["tenantId"], json!("local"));
    assert_eq!(detail["scopes"], json!(["mcp:read", "mcp:execute"]));
    assert_eq!(detail["redirectHost"], json!("127.0.0.1"));

    let decision: Value = server
        .client
        .post(server.url(&format!("/api/oauth/requests/{request_id}/decision")))
        .json(&json!({"decision": "allow", "remember": true}))
        .send()
        .await
        .expect("decision")
        .json()
        .await
        .expect("decision json");
    let approved = decision["redirect"]
        .as_str()
        .expect("approved redirect")
        .to_string();
    assert_eq!(
        redirect_param(&approved, "state"),
        Some("state-1".to_string())
    );
    let code = Server::code_from_redirect(&approved);

    let token: Value = server
        .client
        .post(server.url("/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("client_id", client_id.as_str()),
            ("code_verifier", verifier.as_str()),
        ])
        .send()
        .await
        .expect("token")
        .json()
        .await
        .expect("token json");
    let access = token["access_token"]
        .as_str()
        .expect("access token")
        .to_string();
    let refresh = token["refresh_token"]
        .as_str()
        .expect("refresh token")
        .to_string();
    assert_eq!(token["token_type"], json!("Bearer"));
    assert_eq!(token["expires_in"], json!(3600));

    let discover = server
        .client
        .post(server.url("/mcp"))
        .bearer_auth(&access)
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "server/discover")
        .json(&mcp_discover_body())
        .send()
        .await
        .expect("mcp discover");
    assert_eq!(discover.status(), 200);
    let body: Value = discover.json().await.expect("mcp json");
    assert_eq!(body["result"]["resultType"], json!("complete"));

    let introspection: Value = server
        .client
        .post(server.url("/oauth/introspect"))
        .form(&[("token", access.as_str())])
        .send()
        .await
        .expect("introspect")
        .json()
        .await
        .expect("introspect json");
    assert_eq!(introspection["active"], json!(true));
    assert_eq!(introspection["tenant_id"], json!("local"));
    assert_eq!(introspection["scope"], json!("mcp:read mcp:execute"));

    let rotated: Value = server
        .client
        .post(server.url("/oauth/token"))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh.as_str()),
            ("client_id", client_id.as_str()),
        ])
        .send()
        .await
        .expect("refresh")
        .json()
        .await
        .expect("refresh json");
    assert!(rotated["access_token"].is_string());
    assert_ne!(
        rotated["refresh_token"].as_str(),
        Some(refresh.as_str()),
        "refresh token must rotate"
    );

    let revoke = server
        .client
        .post(server.url("/oauth/revoke"))
        .form(&[
            (
                "token",
                rotated["access_token"].as_str().unwrap_or_default(),
            ),
            ("client_id", client_id.as_str()),
        ])
        .send()
        .await
        .expect("revoke");
    assert_eq!(revoke.status(), 200);
    let after: Value = server
        .client
        .post(server.url("/oauth/introspect"))
        .form(&[(
            "token",
            rotated["access_token"].as_str().unwrap_or_default(),
        )])
        .send()
        .await
        .expect("introspect after revoke")
        .json()
        .await
        .expect("introspect json");
    assert_eq!(after["active"], json!(false));

    let blocked = server
        .client
        .post(server.url("/mcp"))
        .bearer_auth(rotated["access_token"].as_str().unwrap_or_default())
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "server/discover")
        .json(&mcp_discover_body())
        .send()
        .await
        .expect("mcp after revoke");
    assert_eq!(blocked.status(), 401);
}

#[tokio::test]
async fn authorization_server_rejects_bad_requests() {
    let data_dir = temp_dir("hardening");
    let server = Server::start(&data_dir, false).await;
    let redirect_uri = "http://127.0.0.1:4173/callback".to_string();
    let client = server
        .register(std::slice::from_ref(&redirect_uri), "none")
        .await;
    let client_id = client["client_id"].as_str().expect("client id").to_string();
    let (verifier, challenge) = pkce();

    let mismatch = server
        .authorize(&[
            ("response_type", "code"),
            ("client_id", &client_id),
            ("redirect_uri", "http://127.0.0.1:4173/other"),
            ("scope", "mcp:read"),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
        ])
        .await;
    assert_eq!(mismatch.status(), 400);
    assert!(mismatch.headers().get("location").is_none());

    let no_pkce = server
        .authorize(&[
            ("response_type", "code"),
            ("client_id", &client_id),
            ("redirect_uri", &redirect_uri),
            ("scope", "mcp:read"),
        ])
        .await;
    let location = no_pkce
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .expect("error redirect")
        .to_string();
    assert_eq!(
        redirect_param(&location, "error").as_deref(),
        Some("invalid_request")
    );

    let bad_scope = server
        .authorize(&[
            ("response_type", "code"),
            ("client_id", &client_id),
            ("redirect_uri", &redirect_uri),
            ("scope", "mcp:read admin:everything"),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("state", "s2"),
        ])
        .await;
    let location = bad_scope
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .expect("scope redirect")
        .to_string();
    assert_eq!(
        redirect_param(&location, "error").as_deref(),
        Some("invalid_scope")
    );
    assert_eq!(redirect_param(&location, "state").as_deref(), Some("s2"));

    let plain_pkce = server
        .authorize(&[
            ("response_type", "code"),
            ("client_id", &client_id),
            ("redirect_uri", &redirect_uri),
            ("scope", "mcp:read"),
            ("code_challenge", &verifier),
            ("code_challenge_method", "plain"),
        ])
        .await;
    let location = plain_pkce
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .expect("method redirect")
        .to_string();
    assert_eq!(
        redirect_param(&location, "error").as_deref(),
        Some("invalid_request")
    );

    let jar = server
        .authorize(&[
            ("response_type", "code"),
            ("client_id", &client_id),
            ("redirect_uri", &redirect_uri),
            ("scope", "mcp:read"),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("request_uri", "urn:ietf:params:oauth:request_uri:test"),
        ])
        .await;
    assert_eq!(jar.status(), 400);

    let jarm = server
        .authorize(&[
            ("response_type", "code"),
            ("client_id", &client_id),
            ("redirect_uri", &redirect_uri),
            ("scope", "mcp:read"),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("response_mode", "jwt"),
        ])
        .await;
    let location = jarm
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .expect("JARM error redirect");
    assert_eq!(
        redirect_param(location, "error").as_deref(),
        Some("invalid_request")
    );

    let mut dpop_url = reqwest::Url::parse(&server.url("/oauth/authorize")).expect("authorize URL");
    dpop_url
        .query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &client_id)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("scope", "mcp:read")
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");
    let dpop = server
        .client
        .get(dpop_url)
        .header("DPoP", "not-supported")
        .send()
        .await
        .expect("DPoP request");
    assert_eq!(dpop.status(), 400);

    let insecure = server
        .client
        .post(server.url("/oauth/register"))
        .json(&json!({
            "client_name": "insecure",
            "redirect_uris": ["http://example.com/callback"],
        }))
        .send()
        .await
        .expect("register insecure");
    assert_eq!(insecure.status(), 400);

    // Wrong verifier is rejected and the code stays unusable afterwards.
    let consent = server
        .authorize(&[
            ("response_type", "code"),
            ("client_id", &client_id),
            ("redirect_uri", &redirect_uri),
            ("scope", "mcp:read"),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
        ])
        .await;
    let location = consent
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .expect("consent location")
        .to_string();
    let request_id = location
        .split("request=")
        .nth(1)
        .expect("request id")
        .to_string();
    let decision: Value = server
        .client
        .post(server.url(&format!("/api/oauth/requests/{request_id}/decision")))
        .json(&json!({"decision": "allow", "remember": false}))
        .send()
        .await
        .expect("decision")
        .json()
        .await
        .expect("decision json");
    let code = Server::code_from_redirect(decision["redirect"].as_str().expect("redirect"));

    let bad_verifier = server
        .client
        .post(server.url("/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("client_id", client_id.as_str()),
            (
                "code_verifier",
                "wrong-verifier-wrong-verifier-wrong-verifier-x",
            ),
        ])
        .send()
        .await
        .expect("bad verifier");
    assert_eq!(bad_verifier.status(), 400);
    let body: Value = bad_verifier.json().await.expect("error json");
    assert_eq!(body["error"], json!("invalid_grant"));

    let token: Value = server
        .client
        .post(server.url("/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("client_id", client_id.as_str()),
            ("code_verifier", verifier.as_str()),
        ])
        .send()
        .await
        .expect("token")
        .json()
        .await
        .expect("token json");
    let refresh = token["refresh_token"]
        .as_str()
        .expect("refresh token")
        .to_string();

    let replay = server
        .client
        .post(server.url("/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("client_id", client_id.as_str()),
            ("code_verifier", verifier.as_str()),
        ])
        .send()
        .await
        .expect("replay");
    assert_eq!(replay.status(), 400);

    // Refresh reuse revokes the whole family, including the newest tokens.
    let rotated: Value = server
        .client
        .post(server.url("/oauth/token"))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh.as_str()),
            ("client_id", client_id.as_str()),
        ])
        .send()
        .await
        .expect("rotate")
        .json()
        .await
        .expect("rotate json");
    let newest_access = rotated["access_token"]
        .as_str()
        .expect("rotated access")
        .to_string();
    let reuse = server
        .client
        .post(server.url("/oauth/token"))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh.as_str()),
            ("client_id", client_id.as_str()),
        ])
        .send()
        .await
        .expect("reuse");
    assert_eq!(reuse.status(), 400);
    let body: Value = reuse.json().await.expect("reuse json");
    assert_eq!(body["error"], json!("invalid_grant"));
    let rejected = server
        .client
        .post(server.url("/mcp"))
        .bearer_auth(&newest_access)
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "server/discover")
        .json(&mcp_discover_body())
        .send()
        .await
        .expect("mcp after reuse");
    assert_eq!(rejected.status(), 401);
}

#[tokio::test]
async fn stored_consent_skips_the_prompt_and_prompt_consent_forces_it() {
    let data_dir = temp_dir("consent");
    let server = Server::start(&data_dir, false).await;
    let redirect_uri = "http://127.0.0.1:4173/callback".to_string();
    let client = server
        .register(std::slice::from_ref(&redirect_uri), "none")
        .await;
    let client_id = client["client_id"].as_str().expect("client id").to_string();
    let (_, challenge) = pkce();
    let query = |extra: Option<(&'static str, &'static str)>| {
        let mut pairs = vec![
            ("response_type", "code"),
            ("client_id", client_id.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("scope", "mcp:read"),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
        ];
        if let Some(pair) = extra {
            pairs.push(pair);
        }
        pairs
    };

    let first = server.authorize(&query(None)).await;
    let location = first
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .expect("consent location")
        .to_string();
    let request_id = location
        .split("request=")
        .nth(1)
        .expect("request id")
        .to_string();
    let decision: Value = server
        .client
        .post(server.url(&format!("/api/oauth/requests/{request_id}/decision")))
        .json(&json!({"decision": "allow", "remember": true}))
        .send()
        .await
        .expect("decision")
        .json()
        .await
        .expect("decision json");
    assert!(
        decision["redirect"]
            .as_str()
            .is_some_and(|value| value.contains("code="))
    );

    let second = server.authorize(&query(None)).await;
    let location = second
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .expect("second location")
        .to_string();
    assert!(
        location.starts_with("http://127.0.0.1:4173/callback"),
        "stored consent redirects straight to the client: {location}"
    );
    assert!(location.contains("code="));

    let forced = server.authorize(&query(Some(("prompt", "consent")))).await;
    let location = forced
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .expect("forced location")
        .to_string();
    assert!(
        location.starts_with("/consent?request="),
        "prompt=consent must ask again: {location}"
    );

    let denied = server.authorize(&query(None)).await;
    let _ = denied;
    let pending = server.authorize(&query(Some(("prompt", "consent")))).await;
    let location = pending
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .expect("pending location")
        .to_string();
    let request_id = location
        .split("request=")
        .nth(1)
        .expect("request id")
        .to_string();
    let decision: Value = server
        .client
        .post(server.url(&format!("/api/oauth/requests/{request_id}/decision")))
        .json(&json!({"decision": "deny"}))
        .send()
        .await
        .expect("deny")
        .json()
        .await
        .expect("deny json");
    assert_eq!(
        redirect_param(
            decision["redirect"].as_str().expect("deny redirect"),
            "error"
        )
        .as_deref(),
        Some("access_denied")
    );
}

#[tokio::test]
async fn admin_api_manages_clients_and_rotates_keys() {
    let data_dir = temp_dir("admin");
    let bootstrap: Value =
        serde_json::from_str(&run_cli(&data_dir, &["enterprise", "init"])).expect("init json");
    let admin_token = bootstrap["token"]
        .as_str()
        .expect("admin token")
        .to_string();
    let server = Server::start(&data_dir, true).await;

    let created: Value = server
        .client
        .post(server.url("/api/enterprise/oauth/clients"))
        .bearer_auth(&admin_token)
        .json(&json!({
            "name": "confidential client",
            "redirect_uris": ["https://client.example/callback"],
            "token_endpoint_auth_method": "client_secret_post",
            "scope": "mcp:read",
        }))
        .send()
        .await
        .expect("create client")
        .json()
        .await
        .expect("create json");
    let client_id = created["id"].as_str().expect("client id").to_string();
    let secret = created["clientSecret"]
        .as_str()
        .expect("client secret")
        .to_string();
    assert!(created["hasSecret"].as_bool().unwrap_or(false));

    let overview: Value = server
        .client
        .get(server.url("/api/enterprise/oauth/overview"))
        .bearer_auth(&admin_token)
        .send()
        .await
        .expect("overview")
        .json()
        .await
        .expect("overview json");
    assert_eq!(overview["issuer"], json!(server.issuer()));
    assert_eq!(overview["signingAlg"], json!("ES256"));
    assert!(overview["clientCount"].as_i64().unwrap_or_default() >= 1);

    let unauthorized = server
        .client
        .get(server.url("/api/enterprise/oauth/overview"))
        .send()
        .await
        .expect("overview without token");
    assert_eq!(unauthorized.status(), 401);

    let rotated: Value = server
        .client
        .post(server.url("/api/enterprise/oauth/keys/rotate"))
        .bearer_auth(&admin_token)
        .send()
        .await
        .expect("rotate")
        .json()
        .await
        .expect("rotate json");
    assert!(rotated["kid"].is_string());
    let jwks: Value = server
        .client
        .get(server.url("/oauth/jwks"))
        .send()
        .await
        .expect("jwks")
        .json()
        .await
        .expect("jwks json");
    assert!(
        jwks["keys"].as_array().map(Vec::len).unwrap_or_default() >= 2,
        "retired keys stay published during the overlap window"
    );

    let bad_secret = server
        .client
        .post(server.url("/oauth/token"))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", "hpa_rt_missing"),
            ("client_id", client_id.as_str()),
            ("client_secret", "wrong"),
        ])
        .send()
        .await
        .expect("bad secret");
    assert_eq!(bad_secret.status(), 401);

    let disable: Value = server
        .client
        .delete(server.url(&format!("/api/enterprise/oauth/clients/{client_id}")))
        .bearer_auth(&admin_token)
        .send()
        .await
        .expect("disable")
        .json()
        .await
        .expect("disable json");
    assert_eq!(disable["disabled"], json!(client_id));

    let rotated_secret = server
        .client
        .post(server.url(&format!("/api/enterprise/oauth/clients/{client_id}/secret")))
        .bearer_auth(&admin_token)
        .send()
        .await
        .expect("rotate disabled client secret");
    assert_eq!(
        rotated_secret.status(),
        409,
        "disabled clients cannot rotate secrets"
    );
    assert!(secret.starts_with("hpa_cs_"));
}
