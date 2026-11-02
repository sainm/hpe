use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signer, SigningKey};
use reqwest::header::{COOKIE, LOCATION, SET_COOKIE};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hpa")
}

fn temp_dir(label: &str) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("hpa-browser-sso-{label}-{}", uuid::Uuid::now_v7()));
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

struct Server {
    child: Child,
    port: u16,
    client: reqwest::Client,
}

impl Server {
    async fn start(data_dir: &Path) -> Self {
        let port = free_port();
        let child = Command::new(binary())
            .arg("--data-dir")
            .arg(data_dir)
            .args(["serve", "--port", &port.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .env("HPA_ENTERPRISE_MODE", "required")
            .spawn()
            .expect("spawn server");
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("http client");
        if !wait_for_health(&client, port).await {
            let mut child = child;
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

    fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct MockIdp {
    port: u16,
    issuer: String,
    wrong_nonce: Arc<AtomicBool>,
    state: Arc<Mutex<IdpState>>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

#[derive(Default)]
struct IdpState {
    active_refresh_tokens: HashSet<String>,
    next_refresh_id: u64,
    reject_refresh: bool,
    logout_requests: Vec<HashMap<String, String>>,
}

impl MockIdp {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("idp bind");
        listener.set_nonblocking(true).expect("idp nonblocking");
        let port = listener.local_addr().expect("idp addr").port();
        let issuer = format!("http://127.0.0.1:{port}");
        let signing = SigningKey::from_bytes(&[13u8; 32]);
        let public = URL_SAFE_NO_PAD.encode(signing.verifying_key().to_bytes());
        let jwks = json!({
            "keys": [{
                "kty": "OKP",
                "crv": "Ed25519",
                "kid": "primary",
                "alg": "EdDSA",
                "use": "sig",
                "x": public
            }]
        })
        .to_string();
        let used: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
        let wrong_nonce = Arc::new(AtomicBool::new(false));
        let state = Arc::new(Mutex::new(IdpState::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_issuer = issuer.clone();
        let thread_wrong = wrong_nonce.clone();
        let thread_state = state.clone();
        let thread_stop = stop.clone();
        let handle = thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => handle_connection(
                        stream,
                        &jwks,
                        &signing,
                        &thread_issuer,
                        &used,
                        &thread_wrong,
                        &thread_state,
                    ),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            port,
            issuer,
            wrong_nonce,
            state,
            stop,
            handle: Some(handle),
        }
    }

    fn set_wrong_nonce(&self, value: bool) {
        self.wrong_nonce.store(value, Ordering::Relaxed);
    }

    fn set_reject_refresh(&self, value: bool) {
        self.state.lock().expect("idp state").reject_refresh = value;
    }

    fn refresh_tokens_issued(&self) -> usize {
        self.state.lock().expect("idp state").next_refresh_id as usize
    }

    fn logout_requests(&self) -> Vec<HashMap<String, String>> {
        self.state
            .lock()
            .expect("idp state")
            .logout_requests
            .clone()
    }

    fn token_endpoint(&self) -> String {
        format!("{}/token", self.issuer)
    }

    fn jwks_url(&self) -> String {
        format!("{}/jwks", self.issuer)
    }

    fn authorize_endpoint(&self) -> String {
        format!("{}/authorize", self.issuer)
    }

    fn end_session_endpoint(&self) -> String {
        format!("{}/logout", self.issuer)
    }
}

impl Drop for MockIdp {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn handle_connection(
    mut stream: TcpStream,
    jwks: &str,
    signing: &SigningKey,
    issuer: &str,
    used: &Arc<Mutex<HashSet<String>>>,
    wrong_nonce: &Arc<AtomicBool>,
    idp_state: &Arc<Mutex<IdpState>>,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                buffer.extend_from_slice(&chunk[..read]);
                if let Some(header_end) = find_sequence(&buffer, b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&buffer[..header_end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())?
                        })
                        .unwrap_or(0);
                    if buffer.len() >= header_end + 4 + length {
                        break;
                    }
                }
                if buffer.len() > 65_536 {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let header_end = find_sequence(&buffer, b"\r\n\r\n").unwrap_or(buffer.len());
    let head = String::from_utf8_lossy(&buffer[..header_end]).to_string();
    let first = head.lines().next().unwrap_or_default().to_string();
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or_default();
    let body = String::from_utf8_lossy(&buffer[(header_end + 4).min(buffer.len())..]).to_string();
    if method == "GET" && path == "/jwks" {
        respond(&mut stream, 200, "application/json", jwks);
        return;
    }
    if method == "GET" && path.starts_with("/logout") {
        let query = path
            .split_once('?')
            .map(|(_, query)| parse_form(query))
            .unwrap_or_default();
        let redirect = query
            .get("post_logout_redirect_uri")
            .expect("post_logout_redirect_uri")
            .clone();
        let state = query.get("state").expect("logout state").clone();
        idp_state
            .lock()
            .expect("idp state")
            .logout_requests
            .push(query);
        let separator = if redirect.contains('?') { '&' } else { '?' };
        respond_redirect(&mut stream, &format!("{redirect}{separator}state={state}"));
        return;
    }
    if method == "POST" && path == "/token" {
        let form = parse_form(&body);
        if form.get("grant_type").map(String::as_str) == Some("refresh_token") {
            let provided = form.get("refresh_token").cloned().unwrap_or_default();
            let (accepted, refresh_token) = {
                let mut state = idp_state.lock().expect("idp state");
                let accepted =
                    !state.reject_refresh && state.active_refresh_tokens.remove(&provided);
                state.next_refresh_id += 1;
                let refresh_token = format!("refresh-{}", state.next_refresh_id);
                if accepted {
                    state.active_refresh_tokens.insert(refresh_token.clone());
                }
                (accepted, refresh_token)
            };
            if !accepted {
                respond_error(&mut stream, 400, "invalid_grant");
                return;
            }
            let id_token = mint_id_token(signing, issuer, "hpa-web", None);
            let response = json!({
                "access_token": "refreshed-access",
                "token_type": "Bearer",
                "expires_in": 600,
                "refresh_token": refresh_token,
                "refresh_expires_in": 86_400,
                "id_token": id_token
            })
            .to_string();
            respond(&mut stream, 200, "application/json", &response);
            return;
        }
        let Some(code) = form.get("code") else {
            respond_error(&mut stream, 400, "invalid_request");
            return;
        };
        let Some((nonce, challenge)) = code.split_once('.') else {
            respond_error(&mut stream, 400, "invalid_grant");
            return;
        };
        let fresh = used
            .lock()
            .map(|mut used| used.insert(code.clone()))
            .unwrap_or(false);
        if !fresh {
            respond_error(&mut stream, 400, "invalid_grant");
            return;
        }
        let Some(verifier) = form.get("code_verifier") else {
            respond_error(&mut stream, 400, "invalid_request");
            return;
        };
        let computed = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        if computed != *challenge {
            respond_error(&mut stream, 400, "invalid_grant");
            return;
        }
        let nonce = if wrong_nonce.load(Ordering::Relaxed) {
            "wrong-nonce"
        } else {
            nonce
        };
        let id_token = mint_id_token(signing, issuer, "hpa-web", Some(nonce));
        let refresh_token = {
            let mut state = idp_state.lock().expect("idp state");
            state.next_refresh_id += 1;
            let refresh_token = format!("refresh-{}", state.next_refresh_id);
            state.active_refresh_tokens.insert(refresh_token.clone());
            refresh_token
        };
        let body = json!({
            "access_token": "authorization-access",
            "token_type": "Bearer",
            "expires_in": 600,
            "refresh_token": refresh_token,
            "refresh_expires_in": 86_400,
            "id_token": id_token
        })
        .to_string();
        respond(&mut stream, 200, "application/json", &body);
        return;
    }
    respond_error(&mut stream, 404, "not_found");
}

fn mint_id_token(
    signing: &SigningKey,
    issuer: &str,
    audience: &str,
    nonce: Option<&str>,
) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default();
    let header = json!({"alg": "EdDSA", "typ": "JWT", "kid": "primary"});
    let mut claims = json!({
        "iss": issuer,
        "aud": audience,
        "sub": "oidc|admin",
        "tenant_id": "local",
        "role": "owner",
        "email": "admin@example.test",
        "name": "Browser Admin",
        "iat": now,
        "nbf": now - 5,
        "exp": now + 600
    });
    if let Some(nonce) = nonce {
        claims["nonce"] = Value::String(nonce.to_string());
    }
    let input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).expect("header")),
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).expect("claims"))
    );
    let signature = signing.sign(input.as_bytes());
    format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature.to_bytes()))
}

fn parse_form(body: &str) -> HashMap<String, String> {
    body.split('&')
        .filter_map(|pair| {
            let (name, value) = pair.split_once('=')?;
            Some((percent_decode(name), percent_decode(value)))
        })
        .collect()
}

fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Ok(value) = u8::from_str_radix(&raw[index + 1..index + 3], 16)
        {
            out.push(value);
            index += 3;
            continue;
        }
        out.push(if bytes[index] == b'+' {
            b' '
        } else {
            bytes[index]
        });
        index += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn find_sequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn respond(stream: &mut TcpStream, status: u16, content_type: &str, body: &str) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        _ => "Error",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

fn respond_redirect(stream: &mut TcpStream, location: &str) {
    let response = format!(
        "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    let _ = stream.write_all(response.as_bytes());
}

fn respond_error(stream: &mut TcpStream, status: u16, code: &str) {
    let body = json!({"error": code}).to_string();
    respond(stream, status, "application/json", &body);
}

fn session_cookie(response: &reqwest::Response) -> String {
    for value in response.headers().get_all(SET_COOKIE) {
        if let Ok(text) = value.to_str()
            && text.starts_with("hpa_session=")
        {
            return text.split(';').next().expect("cookie pair").to_string();
        }
    }
    panic!("no hpa_session cookie in response");
}

fn cookie_flags(response: &reqwest::Response) -> String {
    for value in response.headers().get_all(SET_COOKIE) {
        if let Ok(text) = value.to_str()
            && text.starts_with("hpa_session=")
        {
            return text.to_string();
        }
    }
    panic!("no hpa_session cookie in response");
}

struct LoginParams {
    state: String,
    code: String,
}

async fn start_login(server: &Server, redirect: &str) -> LoginParams {
    let response = server
        .client
        .get(server.url(&format!(
            "/auth/oidc/login?redirect={}",
            urlencoding(redirect)
        )))
        .send()
        .await
        .expect("login request");
    assert_eq!(response.status(), 303, "login should redirect");
    let location = response
        .headers()
        .get(LOCATION)
        .and_then(|value| value.to_str().ok())
        .expect("login location");
    let url = reqwest::Url::parse(location).expect("login location url");
    let params: HashMap<String, String> = url.query_pairs().into_owned().collect();
    assert_eq!(
        params.get("response_type").map(String::as_str),
        Some("code")
    );
    assert_eq!(params.get("client_id").map(String::as_str), Some("hpa-web"));
    assert_eq!(
        params.get("code_challenge_method").map(String::as_str),
        Some("S256")
    );
    assert_eq!(
        params.get("scope").map(String::as_str),
        Some("openid email profile offline_access")
    );
    let challenge = params.get("code_challenge").cloned().expect("challenge");
    assert_eq!(challenge.len(), 43);
    let nonce = params.get("nonce").cloned().expect("nonce");
    let state = params.get("state").cloned().expect("state");
    LoginParams {
        state: state.clone(),
        code: format!("{nonce}.{challenge}"),
    }
}

fn urlencoding(raw: &str) -> String {
    raw.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

async fn complete_login(server: &Server, params: &LoginParams) -> reqwest::Response {
    server
        .client
        .get(server.url(&format!(
            "/auth/oidc/callback?code={}&state={}",
            urlencoding(&params.code),
            urlencoding(&params.state)
        )))
        .send()
        .await
        .expect("callback request")
}

fn location_of(response: &reqwest::Response) -> String {
    response
        .headers()
        .get(LOCATION)
        .and_then(|value| value.to_str().ok())
        .expect("location header")
        .to_string()
}

async fn json_body(response: reqwest::Response) -> Value {
    let status = response.status();
    let text = response.text().await.expect("body");
    serde_json::from_str(&text)
        .unwrap_or_else(|_| panic!("expected json for status {status}: {text}"))
}

async fn configure(server: &Server, idp: &MockIdp, admin: &str) {
    let payload = json!({
        "issuer": idp.issuer.clone(),
        "audience": "hpa-web",
        "jwksUrl": idp.jwks_url(),
        "network": [],
        "clientId": "hpa-web",
        "authorizationEndpoint": idp.authorize_endpoint(),
        "tokenEndpoint": idp.token_endpoint(),
        "redirectUri": server.url("/auth/oidc/callback"),
        "scopes": ["openid", "email", "profile", "offline_access"],
        "adminSubjects": ["admin@example.test"],
        "cookieSecure": false,
        "sessionTtlSeconds": 3600,
        "endSessionEndpoint": idp.end_session_endpoint(),
        "postLogoutRedirectUri": server.url("/auth/oidc/logout/callback")
    });
    let response = server
        .client
        .put(server.url("/api/enterprise/oidc"))
        .bearer_auth(admin)
        .json(&payload)
        .send()
        .await
        .expect("configure oidc");
    assert_eq!(response.status(), 200);
    let config = json_body(response).await;
    assert_eq!(config["clientId"], "hpa-web");
    assert_eq!(
        config["scopes"],
        json!(["openid", "email", "profile", "offline_access"])
    );
    assert_eq!(config["endSessionEndpoint"], idp.end_session_endpoint());
}

#[tokio::test]
async fn browser_sso_login_session_csrf_and_failures() {
    let data_dir = temp_dir("happy");
    let admin = init_admin(&data_dir);
    let server = Server::start(&data_dir).await;
    let idp = MockIdp::start();
    configure(&server, &idp, &admin).await;

    let login = start_login(&server, "/enterprise").await;
    let callback = complete_login(&server, &login).await;
    assert_eq!(callback.status(), 303);
    assert_eq!(location_of(&callback), "/enterprise");
    let mut cookie = session_cookie(&callback);
    let flags = cookie_flags(&callback);
    assert!(
        flags.contains("HttpOnly"),
        "cookie must be HttpOnly: {flags}"
    );
    assert!(
        flags.contains("SameSite=Lax"),
        "cookie must be SameSite=Lax"
    );
    assert!(flags.contains("Path=/"));
    assert!(flags.contains("Max-Age=3600"));
    assert!(!flags.contains("Secure"));

    let session = server
        .client
        .get(server.url("/api/auth/session"))
        .header(COOKIE, &cookie)
        .send()
        .await
        .expect("session");
    let session = json_body(session).await;
    assert_eq!(session["authenticated"], true);
    assert_eq!(session["ssoConfigured"], true);
    assert_eq!(session["principal"]["platformAdmin"], true);
    assert_eq!(session["principal"]["email"], "admin@example.test");
    assert_eq!(session["principal"]["tenantId"], "local");
    let csrf = session["csrfToken"].as_str().expect("csrf").to_string();

    let tenants = server
        .client
        .get(server.url("/api/enterprise/tenants"))
        .header(COOKIE, &cookie)
        .send()
        .await
        .expect("tenants");
    assert_eq!(tenants.status(), 200);

    let workflows = server
        .client
        .get(server.url("/api/workflows"))
        .header(COOKIE, &cookie)
        .send()
        .await
        .expect("tenant workflows");
    assert_eq!(workflows.status(), 200);

    let without_csrf = server
        .client
        .post(server.url("/api/enterprise/tokens"))
        .header(COOKIE, &cookie)
        .json(&json!({"tenantId": "local", "name": "cookie-client", "role": "viewer"}))
        .send()
        .await
        .expect("token without csrf");
    assert_eq!(without_csrf.status(), 403);
    let body = json_body(without_csrf).await;
    assert_eq!(body["error"]["code"], "CSRF_REQUIRED");

    let with_csrf = server
        .client
        .post(server.url("/api/enterprise/tokens"))
        .header(COOKIE, &cookie)
        .header("x-hpa-csrf", &csrf)
        .json(&json!({"tenantId": "local", "name": "cookie-client", "role": "viewer"}))
        .send()
        .await
        .expect("token with csrf");
    assert_eq!(with_csrf.status(), 200);
    let created = json_body(with_csrf).await;
    assert_eq!(created["tokenInfo"]["role"], "viewer");

    let bearer_write = server
        .client
        .post(server.url("/api/enterprise/tokens"))
        .bearer_auth(&admin)
        .json(&json!({"tenantId": "local", "name": "bearer-client", "role": "viewer"}))
        .send()
        .await
        .expect("bearer token");
    assert_eq!(bearer_write.status(), 200);

    let replay = complete_login(&server, &login).await;
    assert_eq!(replay.status(), 303);
    assert_eq!(location_of(&replay), "/login?error=state_invalid");

    let escape = server
        .client
        .get(server.url(&format!(
            "/auth/oidc/login?redirect={}",
            urlencoding("//evil.example")
        )))
        .send()
        .await
        .expect("redirect escape");
    assert_eq!(escape.status(), 303);
    assert_eq!(location_of(&escape), "/login?error=redirect_invalid");

    let member = server
        .client
        .post(server.url("/api/enterprise/members"))
        .bearer_auth(&admin)
        .json(&json!({
            "tenantId": "local",
            "subject": "oidc|admin",
            "displayName": "Browser Admin",
            "email": "admin@example.test",
            "role": "owner"
        }))
        .send()
        .await
        .expect("create member");
    assert_eq!(member.status(), 200);
    let member = json_body(member).await;
    let member_id = member["id"].as_str().expect("member id").to_string();
    let disabled = server
        .client
        .put(server.url(&format!("/api/enterprise/members/{member_id}")))
        .bearer_auth(&admin)
        .json(&json!({"status": "disabled"}))
        .send()
        .await
        .expect("disable member");
    assert_eq!(disabled.status(), 200);

    let blocked = start_login(&server, "/enterprise").await;
    let blocked = complete_login(&server, &blocked).await;
    assert_eq!(blocked.status(), 303);
    assert_eq!(location_of(&blocked), "/login?error=member_disabled");

    let enabled = server
        .client
        .put(server.url(&format!("/api/enterprise/members/{member_id}")))
        .bearer_auth(&admin)
        .json(&json!({"status": "active"}))
        .send()
        .await
        .expect("enable member");
    assert_eq!(enabled.status(), 200);
    idp.set_wrong_nonce(true);
    let nonce_login = start_login(&server, "/enterprise").await;
    let nonce_failure = complete_login(&server, &nonce_login).await;
    assert_eq!(nonce_failure.status(), 303);
    assert_eq!(location_of(&nonce_failure), "/login?error=sso_failed");
    idp.set_wrong_nonce(false);

    let refresh_without_csrf = server
        .client
        .post(server.url("/api/auth/refresh"))
        .header(COOKIE, &cookie)
        .send()
        .await
        .expect("refresh without csrf");
    assert_eq!(refresh_without_csrf.status(), 403);
    assert_eq!(
        json_body(refresh_without_csrf).await["error"]["code"],
        "CSRF_REQUIRED"
    );

    let old_cookie = cookie.clone();
    let issued_before_refresh = idp.refresh_tokens_issued();
    let refresh = server
        .client
        .post(server.url("/api/auth/refresh"))
        .header(COOKIE, &cookie)
        .header("x-hpa-csrf", &csrf)
        .send()
        .await
        .expect("refresh");
    assert_eq!(refresh.status(), 200);
    cookie = session_cookie(&refresh);
    assert_ne!(cookie, old_cookie, "refresh must rotate the session cookie");
    let refresh_body = json_body(refresh).await;
    assert_eq!(refresh_body["authenticated"], true);
    assert_eq!(refresh_body["refreshed"], true);
    assert_eq!(refresh_body["refreshAvailable"], true);
    assert!(
        refresh_body["expiresAt"].as_i64().unwrap()
            > refresh_body["accessTokenExpiresAt"].as_i64().unwrap()
    );
    assert_eq!(idp.refresh_tokens_issued(), issued_before_refresh + 1);

    let rotated_old = server
        .client
        .get(server.url("/api/auth/session"))
        .header(COOKIE, &old_cookie)
        .send()
        .await
        .expect("old session cookie");
    assert_eq!(json_body(rotated_old).await["authenticated"], false);

    let logout = server
        .client
        .post(server.url("/api/auth/logout"))
        .header(COOKIE, &cookie)
        .header("x-hpa-csrf", &csrf)
        .send()
        .await
        .expect("logout");
    assert_eq!(logout.status(), 200);
    let cleared_cookie = cookie_flags(&logout);
    assert!(cleared_cookie.starts_with("hpa_session=;"));
    assert!(cleared_cookie.contains("Max-Age=0"));
    let logout_body = json_body(logout).await;
    assert_eq!(logout_body["loggedOut"], true);
    let logout_url = logout_body["logoutUrl"]
        .as_str()
        .expect("RP-initiated logout URL");
    let idp_logout = server
        .client
        .get(logout_url)
        .send()
        .await
        .expect("IdP logout");
    assert_eq!(idp_logout.status(), 302);
    let callback_location = location_of(&idp_logout);
    let logout_callback = server
        .client
        .get(callback_location.clone())
        .send()
        .await
        .expect("logout callback");
    assert_eq!(logout_callback.status(), 303);
    assert_eq!(location_of(&logout_callback), "/login?loggedOut=1");
    let replayed_callback = server
        .client
        .get(callback_location)
        .send()
        .await
        .expect("replayed logout callback");
    assert_eq!(
        location_of(&replayed_callback),
        "/login?error=logout_state_invalid"
    );
    let logout_requests = idp.logout_requests();
    assert_eq!(logout_requests.len(), 1);
    assert_eq!(
        logout_requests[0].get("client_id").map(String::as_str),
        Some("hpa-web")
    );
    assert!(
        logout_requests[0]
            .get("id_token_hint")
            .is_some_and(|token| token.split('.').count() == 3)
    );

    let after = server
        .client
        .get(server.url("/api/auth/session"))
        .header(COOKIE, &cookie)
        .send()
        .await
        .expect("session after logout");
    assert_eq!(json_body(after).await["authenticated"], false);

    let denied = server
        .client
        .get(server.url("/api/enterprise/tenants"))
        .header(COOKIE, &cookie)
        .send()
        .await
        .expect("tenants after logout");
    assert_eq!(denied.status(), 401);

    let _ = std::fs::remove_dir_all(&data_dir);
}

#[tokio::test]
async fn browser_sso_requires_configuration_and_csrf() {
    let data_dir = temp_dir("edge");
    let admin = init_admin(&data_dir);
    let server = Server::start(&data_dir).await;

    let unavailable = server
        .client
        .get(server.url("/auth/oidc/login"))
        .send()
        .await
        .expect("login without config");
    assert_eq!(unavailable.status(), 303);
    assert_eq!(location_of(&unavailable), "/login?error=sso_unavailable");

    let session = server
        .client
        .get(server.url("/api/auth/session"))
        .send()
        .await
        .expect("session");
    let session = json_body(session).await;
    assert_eq!(session["authenticated"], false);
    assert_eq!(session["ssoConfigured"], false);
    assert_eq!(session["csrfToken"], Value::Null);

    let write = server
        .client
        .post(server.url("/api/enterprise/tokens"))
        .bearer_auth(&admin)
        .json(&json!({"tenantId": "local", "name": "no-cookie", "role": "viewer"}))
        .send()
        .await
        .expect("bearer write");
    assert_eq!(write.status(), 200);

    let callback = server
        .client
        .get(server.url("/auth/oidc/callback?code=bad&state=bad"))
        .send()
        .await
        .expect("bad callback");
    assert_eq!(callback.status(), 303);
    assert_eq!(location_of(&callback), "/login?error=state_invalid");

    let _ = std::fs::remove_dir_all(&data_dir);
}

#[tokio::test]
async fn browser_sso_refresh_rejection_revokes_the_session() {
    let data_dir = temp_dir("refresh-reject");
    let admin = init_admin(&data_dir);
    let server = Server::start(&data_dir).await;
    let idp = MockIdp::start();
    configure(&server, &idp, &admin).await;

    let login = start_login(&server, "/enterprise").await;
    let callback = complete_login(&server, &login).await;
    let cookie = session_cookie(&callback);
    let session = server
        .client
        .get(server.url("/api/auth/session"))
        .header(COOKIE, &cookie)
        .send()
        .await
        .expect("session");
    let session = json_body(session).await;
    let csrf = session["csrfToken"].as_str().expect("csrf");

    idp.set_reject_refresh(true);
    let refresh = server
        .client
        .post(server.url("/api/auth/refresh"))
        .header(COOKIE, &cookie)
        .header("x-hpa-csrf", csrf)
        .send()
        .await
        .expect("rejected refresh");
    assert_eq!(refresh.status(), 401);
    assert!(cookie_flags(&refresh).starts_with("hpa_session=;"));

    let after = server
        .client
        .get(server.url("/api/auth/session"))
        .header(COOKIE, &cookie)
        .send()
        .await
        .expect("session after rejected refresh");
    assert_eq!(json_body(after).await["authenticated"], false);

    let _ = std::fs::remove_dir_all(&data_dir);
}
