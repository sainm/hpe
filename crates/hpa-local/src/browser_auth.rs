use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::Url;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::enterprise_ops::{OidcPrincipal, OpsError, OpsService};
use hpa_local::postgres_control::{ControlError, PostgresControlStore, SessionRecord};

const SESSION_COOKIE: &str = "hpa_session";
const FLOW_TTL_MS: i64 = 600_000;
const DEFAULT_SCOPES: &str = "openid email profile";
const DEFAULT_SESSION_TTL: i64 = 28_800;
const MIN_SESSION_TTL: i64 = 300;
const MAX_SESSION_TTL: i64 = 604_800;
const MAX_ACCESS_TOKEN_TTL: i64 = 2_592_000;
const MAX_REFRESH_TOKEN_TTL: i64 = 31_536_000;
const KEY_DIRECTORY: &str = "keys";
const KEY_FILE: &str = "browser-auth.key";
const SECRET_ENVELOPE_VERSION: &str = "v1";

#[derive(Clone)]
pub struct BrowserAuth {
    control: SqlitePool,
    ops: OpsService,
    postgres: Option<PostgresControlStore>,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct SessionPrincipal {
    pub session_id: String,
    pub csrf_token: String,
    pub tenant_id: String,
    pub subject: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub role: String,
    pub platform_admin: bool,
    pub expires_at: i64,
    pub access_token_expires_at: Option<i64>,
    pub refresh_available: bool,
}

pub struct LoginStart {
    pub location: String,
}

pub struct LoginResult {
    pub principal: SessionPrincipal,
    pub cookie: String,
    pub redirect: String,
}

#[allow(dead_code)]
pub struct RefreshResult {
    pub principal: SessionPrincipal,
    pub cookie: String,
}

#[allow(dead_code)]
pub struct LogoutResult {
    pub revoked: bool,
    pub cookie: String,
    pub logout_url: Option<String>,
}

#[derive(Debug)]
pub enum BrowserAuthError {
    Invalid(String),
    NotFound(String),
    Forbidden(String),
    Upstream(String),
    Db(sqlx::Error),
    Json(serde_json::Error),
    Io(std::io::Error),
    Crypto(String),
}

impl fmt::Display for BrowserAuthError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => write!(formatter, "invalid browser auth request: {message}"),
            Self::NotFound(message) => {
                write!(formatter, "browser auth resource not found: {message}")
            }
            Self::Forbidden(message) => write!(formatter, "browser auth forbidden: {message}"),
            Self::Upstream(message) => write!(formatter, "browser auth upstream error: {message}"),
            Self::Db(error) => write!(formatter, "browser auth database error: {error}"),
            Self::Json(error) => write!(formatter, "browser auth json error: {error}"),
            Self::Io(error) => write!(formatter, "browser auth I/O error: {error}"),
            Self::Crypto(error) => write!(formatter, "browser auth crypto error: {error}"),
        }
    }
}

impl std::error::Error for BrowserAuthError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Db(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<sqlx::Error> for BrowserAuthError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(error)
    }
}

impl From<serde_json::Error> for BrowserAuthError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<std::io::Error> for BrowserAuthError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Clone)]
pub struct BrowserConfig {
    pub client_id: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub redirect_uri: String,
    pub scopes: String,
    pub admin_subjects: Vec<String>,
    pub cookie_secure: bool,
    pub session_ttl_seconds: i64,
    pub end_session_endpoint: Option<String>,
    pub post_logout_redirect_uri: Option<String>,
}

impl BrowserConfig {
    fn from_value(value: &Value) -> Option<Self> {
        let text = |name: &str| value.get(name).and_then(Value::as_str).unwrap_or("");
        let client_id = text("clientId");
        let authorization_endpoint = text("authorizationEndpoint");
        let token_endpoint = text("tokenEndpoint");
        let redirect_uri = text("redirectUri");
        let end_session_endpoint = text("endSessionEndpoint");
        let post_logout_redirect_uri = text("postLogoutRedirectUri");
        if text("issuer").is_empty()
            || text("audience").is_empty()
            || text("jwksUrl").is_empty()
            || client_id.is_empty()
            || authorization_endpoint.is_empty()
            || token_endpoint.is_empty()
            || redirect_uri.is_empty()
        {
            return None;
        }
        let admin_subjects = value
            .get("adminSubjects")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|entry| !entry.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let scopes = text("scopes");
        let session_ttl_seconds = value
            .get("sessionTtlSeconds")
            .and_then(Value::as_i64)
            .filter(|ttl| (MIN_SESSION_TTL..=MAX_SESSION_TTL).contains(ttl))
            .unwrap_or(DEFAULT_SESSION_TTL);
        Some(Self {
            client_id: client_id.to_string(),
            authorization_endpoint: authorization_endpoint.to_string(),
            token_endpoint: token_endpoint.to_string(),
            redirect_uri: redirect_uri.to_string(),
            scopes: if scopes.is_empty() {
                DEFAULT_SCOPES.to_string()
            } else {
                scopes.to_string()
            },
            admin_subjects,
            cookie_secure: value
                .get("cookieSecure")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            session_ttl_seconds,
            end_session_endpoint: (!end_session_endpoint.is_empty())
                .then(|| end_session_endpoint.to_string()),
            post_logout_redirect_uri: (!post_logout_redirect_uri.is_empty())
                .then(|| post_logout_redirect_uri.to_string()),
        })
    }
}

struct StoredFlow {
    code_verifier: String,
    nonce: String,
    redirect: String,
}

struct MemberRecord {
    display_name: String,
    role: String,
    status: String,
}

struct CreatedSession {
    id: String,
    token: String,
    csrf_token: String,
    expires_at: i64,
    access_token_expires_at: Option<i64>,
    refresh_available: bool,
}

struct TokenGrant {
    id_token: Option<String>,
    refresh_token: Option<String>,
    access_token_expires_at: i64,
    refresh_token_expires_at: Option<i64>,
}

#[allow(dead_code)]
#[derive(Debug, sqlx::FromRow)]
struct StoredSession {
    id: String,
    csrf_token: String,
    tenant_id: String,
    subject: String,
    email: Option<String>,
    display_name: Option<String>,
    role: String,
    platform_admin: i64,
    expires_at: i64,
    revoked_at: Option<i64>,
    refresh_token_ciphertext: Option<String>,
    refresh_token_version: i64,
    refresh_token_expires_at: Option<i64>,
    id_token_ciphertext: Option<String>,
    access_token_expires_at: Option<i64>,
}

impl BrowserAuth {
    pub fn new(control: SqlitePool, ops: OpsService) -> Self {
        Self {
            control,
            ops,
            postgres: None,
        }
    }

    /// Sessions move to PostgreSQL when the control plane runs on it, so a
    /// cookie issued by one node is accepted by every other node.
    #[allow(dead_code)]
    pub fn with_postgres(mut self, pool: sqlx::postgres::PgPool) -> Self {
        self.postgres = Some(PostgresControlStore::new(pool));
        self
    }

    pub async fn ensure_schema(&self) -> Result<(), BrowserAuthError> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS enterprise_auth_flows (
                state_hash TEXT PRIMARY KEY,
                code_verifier TEXT NOT NULL,
                nonce TEXT NOT NULL,
                redirect TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL
            )",
        )
        .execute(&self.control)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS enterprise_sessions (
                id TEXT PRIMARY KEY,
                token_hash TEXT NOT NULL UNIQUE,
                csrf_token TEXT NOT NULL,
                tenant_id TEXT NOT NULL,
                subject TEXT NOT NULL,
                email TEXT,
                display_name TEXT,
                role TEXT NOT NULL,
                platform_admin INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                revoked_at INTEGER,
                last_seen_at INTEGER,
                refresh_token_ciphertext TEXT,
                refresh_token_version INTEGER NOT NULL DEFAULT 0,
                refresh_token_expires_at INTEGER,
                id_token_ciphertext TEXT,
                access_token_expires_at INTEGER,
                last_refresh_at INTEGER
            )",
        )
        .execute(&self.control)
        .await?;
        let columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('enterprise_sessions')")
                .fetch_all(&self.control)
                .await?;
        let additions = [
            (
                "refresh_token_ciphertext",
                "ALTER TABLE enterprise_sessions ADD COLUMN refresh_token_ciphertext TEXT",
            ),
            (
                "refresh_token_version",
                "ALTER TABLE enterprise_sessions ADD COLUMN refresh_token_version INTEGER NOT NULL DEFAULT 0",
            ),
            (
                "refresh_token_expires_at",
                "ALTER TABLE enterprise_sessions ADD COLUMN refresh_token_expires_at INTEGER",
            ),
            (
                "id_token_ciphertext",
                "ALTER TABLE enterprise_sessions ADD COLUMN id_token_ciphertext TEXT",
            ),
            (
                "access_token_expires_at",
                "ALTER TABLE enterprise_sessions ADD COLUMN access_token_expires_at INTEGER",
            ),
            (
                "last_refresh_at",
                "ALTER TABLE enterprise_sessions ADD COLUMN last_refresh_at INTEGER",
            ),
        ];
        for (name, statement) in additions {
            if !columns.iter().any(|column| column == name) {
                sqlx::query(statement).execute(&self.control).await?;
            }
        }
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS enterprise_logout_flows (
                state_hash TEXT PRIMARY KEY,
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL
            )",
        )
        .execute(&self.control)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS enterprise_sessions_tenant
             ON enterprise_sessions (tenant_id, created_at DESC)",
        )
        .execute(&self.control)
        .await?;
        self.encryption_key()?;
        Ok(())
    }

    pub async fn begin_login(
        &self,
        redirect: Option<&str>,
    ) -> Result<LoginStart, BrowserAuthError> {
        let config = self
            .config()
            .await?
            .ok_or_else(|| BrowserAuthError::Invalid("sso_unavailable".to_string()))?;
        let redirect = validate_redirect(redirect)?;
        let state = random_token()?;
        let nonce = random_token()?;
        let code_verifier = random_token()?;
        let challenge = pkce_challenge(&code_verifier);
        let state_hash = sha256_hex(state.as_bytes());
        let now = now_ms();
        let inserted = sqlx::query(
            "INSERT INTO enterprise_auth_flows
             (state_hash, code_verifier, nonce, redirect, created_at, expires_at)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(state_hash) DO NOTHING",
        )
        .bind(&state_hash)
        .bind(&code_verifier)
        .bind(&nonce)
        .bind(&redirect)
        .bind(now)
        .bind(now + FLOW_TTL_MS)
        .execute(&self.control)
        .await?;
        if inserted.rows_affected() == 0 {
            return Err(BrowserAuthError::Invalid("state_collision".to_string()));
        }
        let mut url = Url::parse(&config.authorization_endpoint)
            .map_err(|_| BrowserAuthError::Invalid("sso_unavailable".to_string()))?;
        url.set_fragment(None);
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &config.client_id)
            .append_pair("redirect_uri", &config.redirect_uri)
            .append_pair("scope", &config.scopes)
            .append_pair("state", &state)
            .append_pair("nonce", &nonce)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256");
        Ok(LoginStart {
            location: url.to_string(),
        })
    }

    pub async fn complete_login(
        &self,
        code: &str,
        state: &str,
    ) -> Result<LoginResult, BrowserAuthError> {
        if code.trim().is_empty() {
            return Err(BrowserAuthError::Invalid("code_invalid".to_string()));
        }
        if state.trim().is_empty() {
            return Err(BrowserAuthError::Invalid("state_invalid".to_string()));
        }
        let flow = self.take_flow(state).await?;
        let redirect = validate_redirect(Some(&flow.redirect))?;
        let config = self
            .config()
            .await?
            .ok_or_else(|| BrowserAuthError::Invalid("sso_unavailable".to_string()))?;
        let client = upstream_client()?;
        let form = [
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", config.redirect_uri.as_str()),
            ("client_id", config.client_id.as_str()),
            ("code_verifier", flow.code_verifier.as_str()),
        ];
        let response = client
            .post(&config.token_endpoint)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .form(&form)
            .send()
            .await
            .map_err(|error| BrowserAuthError::Upstream(error.to_string()))?;
        if !response.status().is_success() {
            return Err(BrowserAuthError::Upstream(
                "token endpoint rejected the authorization code".to_string(),
            ));
        }
        let document: Value = response
            .json()
            .await
            .map_err(|error| BrowserAuthError::Upstream(error.to_string()))?;
        let grant = parse_token_grant(&document, now_ms())?;
        let id_token = grant.id_token.as_deref().ok_or_else(|| {
            BrowserAuthError::Upstream("token response has no id_token".to_string())
        })?;
        let principal: OidcPrincipal = self
            .ops
            .verify_id_token(id_token, &flow.nonce)
            .await
            .map_err(browser_ops_error)?;
        self.ensure_active_tenant(&principal.tenant_id).await?;
        let member = self
            .find_member(
                &principal.tenant_id,
                &principal.subject,
                principal.email.as_deref(),
            )
            .await?;
        let mut role = principal.role.clone();
        let mut display_name = principal.name.clone();
        if let Some(member) = &member {
            if member.status != "active" {
                return Err(BrowserAuthError::Forbidden("member_disabled".to_string()));
            }
            role = member.role.clone();
            if !member.display_name.trim().is_empty() {
                display_name = Some(member.display_name.clone());
            }
        }
        let platform_admin = is_platform_admin(
            &config.admin_subjects,
            &principal.subject,
            principal.email.as_deref(),
        );
        let display_name = display_name
            .or_else(|| principal.email.clone())
            .unwrap_or_else(|| principal.subject.clone());
        let session = self
            .insert_session(
                &principal.tenant_id,
                &principal.subject,
                principal.email.as_deref(),
                Some(&display_name),
                &role,
                platform_admin,
                config.session_ttl_seconds,
                grant,
            )
            .await?;
        let cookie = session_cookie(
            &session.token,
            config.session_ttl_seconds,
            config.cookie_secure,
        );
        Ok(LoginResult {
            principal: SessionPrincipal {
                session_id: session.id,
                csrf_token: session.csrf_token,
                tenant_id: principal.tenant_id,
                subject: principal.subject,
                email: principal.email,
                display_name: Some(display_name),
                role,
                platform_admin,
                expires_at: session.expires_at,
                access_token_expires_at: session.access_token_expires_at,
                refresh_available: session.refresh_available,
            },
            cookie,
            redirect,
        })
    }

    pub async fn session(
        &self,
        cookie_header: Option<&str>,
    ) -> Result<Option<SessionPrincipal>, BrowserAuthError> {
        let Some(stored) = self.stored_session(cookie_header).await? else {
            return Ok(None);
        };
        let now = now_ms();
        if stored.revoked_at.is_some() || stored.expires_at <= now {
            return Ok(None);
        }
        self.ensure_active_tenant(&stored.tenant_id).await?;
        let member = self
            .find_member(&stored.tenant_id, &stored.subject, stored.email.as_deref())
            .await?;
        let role = match member {
            Some(member) if member.status != "active" => {
                return Err(BrowserAuthError::Forbidden("member_disabled".to_string()));
            }
            Some(member) => member.role,
            None => stored.role.clone(),
        };
        if let Some(postgres) = self.postgres.as_ref() {
            let _ = postgres.touch_session(&stored.id, now).await;
        } else {
            let _ = sqlx::query("UPDATE enterprise_sessions SET last_seen_at = ? WHERE id = ?")
                .bind(now)
                .bind(&stored.id)
                .execute(&self.control)
                .await;
        }
        Ok(Some(principal_from_stored(stored, role)))
    }

    #[allow(dead_code)]
    pub async fn refresh(
        &self,
        cookie_header: Option<&str>,
    ) -> Result<RefreshResult, BrowserAuthError> {
        let token = cookie_value(cookie_header, SESSION_COOKIE)
            .ok_or_else(|| BrowserAuthError::Invalid("session_required".to_string()))?;
        let token_hash = sha256_hex(token.as_bytes());
        let stored = self
            .stored_session_by_hash(&token_hash)
            .await?
            .ok_or_else(|| BrowserAuthError::Invalid("session_invalid".to_string()))?;
        let now = now_ms();
        if stored.revoked_at.is_some() || stored.expires_at <= now {
            return Err(BrowserAuthError::Invalid("session_invalid".to_string()));
        }
        self.ensure_active_tenant(&stored.tenant_id).await?;
        let member = self
            .find_member(&stored.tenant_id, &stored.subject, stored.email.as_deref())
            .await?;
        let current_role = match member {
            Some(member) if member.status != "active" => {
                return Err(BrowserAuthError::Forbidden("member_disabled".to_string()));
            }
            Some(member) => member.role,
            None => stored.role.clone(),
        };
        let config = self
            .config()
            .await?
            .ok_or_else(|| BrowserAuthError::Invalid("sso_unavailable".to_string()))?;
        let ciphertext = stored
            .refresh_token_ciphertext
            .as_deref()
            .ok_or_else(|| BrowserAuthError::Invalid("refresh_unavailable".to_string()))?;
        if stored.refresh_token_version <= 0 {
            return Err(BrowserAuthError::Invalid("refresh_unavailable".to_string()));
        }
        if stored
            .refresh_token_expires_at
            .is_some_and(|expires_at| expires_at <= now)
        {
            self.revoke_session_id(&stored.id).await?;
            return Err(BrowserAuthError::Forbidden(
                "session_refresh_expired".to_string(),
            ));
        }
        let refresh_token =
            match self.decrypt_secret(ciphertext, &format!("refresh-token:{}", stored.id)) {
                Ok(token) => Zeroizing::new(token),
                Err(_) => {
                    self.revoke_session_id(&stored.id).await?;
                    return Err(BrowserAuthError::Forbidden(
                        "session_refresh_failed".to_string(),
                    ));
                }
            };
        let client = upstream_client()?;
        let response = match client
            .post(&config.token_endpoint)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token.as_str()),
                ("client_id", config.client_id.as_str()),
            ])
            .send()
            .await
        {
            Ok(response) => response,
            Err(_) => {
                self.revoke_session_id(&stored.id).await?;
                return Err(BrowserAuthError::Forbidden(
                    "session_refresh_failed".to_string(),
                ));
            }
        };
        if !response.status().is_success() {
            self.revoke_session_id(&stored.id).await?;
            return Err(BrowserAuthError::Forbidden(
                "session_refresh_failed".to_string(),
            ));
        }
        let document: Value = match response.json().await {
            Ok(document) => document,
            Err(_) => {
                self.revoke_session_id(&stored.id).await?;
                return Err(BrowserAuthError::Forbidden(
                    "session_refresh_failed".to_string(),
                ));
            }
        };
        let grant = match parse_token_grant(&document, now) {
            Ok(grant) => grant,
            Err(_) => {
                self.revoke_session_id(&stored.id).await?;
                return Err(BrowserAuthError::Forbidden(
                    "session_refresh_failed".to_string(),
                ));
            }
        };
        if let Some(id_token) = grant.id_token.as_deref() {
            let principal = match self.ops.verify_oidc_token(id_token).await {
                Ok(principal) => principal,
                Err(_) => {
                    self.revoke_session_id(&stored.id).await?;
                    return Err(BrowserAuthError::Forbidden(
                        "session_refresh_failed".to_string(),
                    ));
                }
            };
            if principal.tenant_id != stored.tenant_id || principal.subject != stored.subject {
                self.revoke_session_id(&stored.id).await?;
                return Err(BrowserAuthError::Forbidden(
                    "session_refresh_identity_changed".to_string(),
                ));
            }
        }
        self.ensure_active_tenant(&stored.tenant_id).await?;
        let member = self
            .find_member(&stored.tenant_id, &stored.subject, stored.email.as_deref())
            .await?;
        let role = match member {
            Some(member) if member.status != "active" => {
                self.revoke_session_id(&stored.id).await?;
                return Err(BrowserAuthError::Forbidden("member_disabled".to_string()));
            }
            Some(member) => member.role,
            None => current_role,
        };
        let refreshed_at = now_ms();
        let session_expires_at = refreshed_at
            .saturating_add(config.session_ttl_seconds.saturating_mul(1000))
            .min(
                grant
                    .refresh_token_expires_at
                    .or(stored.refresh_token_expires_at)
                    .unwrap_or(i64::MAX),
            );
        if session_expires_at <= refreshed_at {
            self.revoke_session_id(&stored.id).await?;
            return Err(BrowserAuthError::Forbidden(
                "session_refresh_expired".to_string(),
            ));
        }
        let new_token = random_token()?;
        let new_token_hash = sha256_hex(new_token.as_bytes());
        let refresh_ciphertext = match grant.refresh_token.as_deref() {
            Some(refresh_token) => {
                match self.encrypt_secret(refresh_token, &format!("refresh-token:{}", stored.id)) {
                    Ok(ciphertext) => ciphertext,
                    Err(_) => {
                        self.revoke_session_id(&stored.id).await?;
                        return Err(BrowserAuthError::Forbidden(
                            "session_refresh_failed".to_string(),
                        ));
                    }
                }
            }
            None => ciphertext.to_string(),
        };
        let refresh_expires_at = grant
            .refresh_token_expires_at
            .or(stored.refresh_token_expires_at);
        let id_token_ciphertext = match grant.id_token.as_deref() {
            Some(id_token) => {
                match self.encrypt_secret(id_token, &format!("id-token:{}", stored.id)) {
                    Ok(ciphertext) => Some(ciphertext),
                    Err(_) => {
                        self.revoke_session_id(&stored.id).await?;
                        return Err(BrowserAuthError::Forbidden(
                            "session_refresh_failed".to_string(),
                        ));
                    }
                }
            }
            None => stored.id_token_ciphertext.clone(),
        };
        let rotated = if let Some(postgres) = self.postgres.as_ref() {
            postgres
                .rotate_session_tokens(
                    &stored.id,
                    &token_hash,
                    i32::try_from(stored.refresh_token_version).unwrap_or_default(),
                    &new_token_hash,
                    Some(refresh_ciphertext.as_str()),
                    refresh_expires_at,
                    id_token_ciphertext.as_deref(),
                    Some(grant.access_token_expires_at),
                    session_expires_at,
                    refreshed_at,
                )
                .await
                .map_err(|error| BrowserAuthError::Upstream(error.to_string()))?
        } else {
            let updated = sqlx::query(
                "UPDATE enterprise_sessions
                 SET token_hash = ?, refresh_token_ciphertext = ?,
                     refresh_token_version = refresh_token_version + 1,
                     refresh_token_expires_at = ?, id_token_ciphertext = ?,
                     access_token_expires_at = ?, expires_at = ?,
                     last_refresh_at = ?, last_seen_at = ?
                 WHERE id = ? AND token_hash = ? AND refresh_token_version = ?
                   AND revoked_at IS NULL",
            )
            .bind(&new_token_hash)
            .bind(&refresh_ciphertext)
            .bind(refresh_expires_at)
            .bind(&id_token_ciphertext)
            .bind(grant.access_token_expires_at)
            .bind(session_expires_at)
            .bind(refreshed_at)
            .bind(refreshed_at)
            .bind(&stored.id)
            .bind(&token_hash)
            .bind(stored.refresh_token_version)
            .execute(&self.control)
            .await?;
            updated.rows_affected() == 1
        };
        if !rotated {
            self.revoke_session_id(&stored.id).await?;
            return Err(BrowserAuthError::Forbidden(
                "session_refresh_replay".to_string(),
            ));
        }
        let ttl_seconds = ((session_expires_at - refreshed_at) / 1000).max(1);
        let mut principal = principal_from_stored(stored, role);
        principal.expires_at = session_expires_at;
        principal.access_token_expires_at = Some(grant.access_token_expires_at);
        principal.refresh_available =
            refresh_expires_at.is_none_or(|expires_at| expires_at > refreshed_at);
        Ok(RefreshResult {
            cookie: session_cookie(&new_token, ttl_seconds, config.cookie_secure),
            principal,
        })
    }

    pub async fn logout(
        &self,
        cookie_header: Option<&str>,
    ) -> Result<LogoutResult, BrowserAuthError> {
        let config = self.config().await?.unwrap_or_else(|| BrowserConfig {
            client_id: String::new(),
            authorization_endpoint: String::new(),
            token_endpoint: String::new(),
            redirect_uri: String::new(),
            scopes: String::new(),
            admin_subjects: Vec::new(),
            cookie_secure: false,
            session_ttl_seconds: DEFAULT_SESSION_TTL,
            end_session_endpoint: None,
            post_logout_redirect_uri: None,
        });
        let clear_cookie = clear_cookie(config.cookie_secure);
        let Some(token) = cookie_value(cookie_header, SESSION_COOKIE) else {
            return Ok(LogoutResult {
                revoked: false,
                cookie: clear_cookie,
                logout_url: None,
            });
        };
        let token_hash = sha256_hex(token.as_bytes());
        let stored = self.stored_session_by_hash(&token_hash).await?;
        let id_token = stored
            .as_ref()
            .and_then(|session| session.id_token_ciphertext.as_deref())
            .and_then(|ciphertext| {
                stored.as_ref().and_then(|session| {
                    self.decrypt_secret(ciphertext, &format!("id-token:{}", session.id))
                        .ok()
                })
            });
        let revoked = if let Some(postgres) = self.postgres.as_ref() {
            postgres
                .revoke_session_by_hash(&token_hash, now_ms())
                .await
                .map_err(|error| BrowserAuthError::Upstream(error.to_string()))?
        } else {
            sqlx::query(
                "UPDATE enterprise_sessions SET revoked_at = ?
                 WHERE token_hash = ? AND revoked_at IS NULL",
            )
            .bind(now_ms())
            .bind(&token_hash)
            .execute(&self.control)
            .await?
            .rows_affected()
                > 0
        };
        let logout_url = match (
            stored.as_ref(),
            id_token,
            config.end_session_endpoint.as_deref(),
            config.post_logout_redirect_uri.as_deref(),
        ) {
            (Some(_session), Some(id_token), Some(endpoint), Some(redirect_uri)) => self
                .logout_url(&config.client_id, endpoint, redirect_uri, &id_token)
                .await
                .ok(),
            _ => None,
        };
        Ok(LogoutResult {
            revoked,
            cookie: clear_cookie,
            logout_url,
        })
    }

    #[allow(dead_code)]
    pub async fn consume_logout_state(&self, state: &str) -> Result<(), BrowserAuthError> {
        if state.trim().is_empty() {
            return Err(BrowserAuthError::Invalid(
                "logout_state_invalid".to_string(),
            ));
        }
        let state_hash = sha256_hex(state.as_bytes());
        let expires_at: Option<i64> = sqlx::query_scalar(
            "DELETE FROM enterprise_logout_flows WHERE state_hash = ?
             RETURNING expires_at",
        )
        .bind(&state_hash)
        .fetch_optional(&self.control)
        .await?;
        if expires_at.is_none_or(|expires_at| expires_at <= now_ms()) {
            return Err(BrowserAuthError::Invalid(
                "logout_state_invalid".to_string(),
            ));
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn clear_session_cookie(&self) -> String {
        let secure = self
            .config()
            .await
            .ok()
            .flatten()
            .is_some_and(|config| config.cookie_secure);
        clear_cookie(secure)
    }

    #[allow(dead_code)]
    pub async fn sso_configured(&self) -> Result<bool, BrowserAuthError> {
        Ok(self.config().await?.is_some())
    }

    #[cfg(test)]
    pub async fn validate_csrf(
        &self,
        cookie_header: Option<&str>,
        csrf_header: Option<&str>,
    ) -> Result<bool, BrowserAuthError> {
        let Some(principal) = self.session(cookie_header).await? else {
            return Ok(false);
        };
        let Some(csrf_header) = csrf_header else {
            return Ok(false);
        };
        Ok(csrf_header == principal.csrf_token)
    }

    pub async fn revoke_tenant_sessions(&self, tenant_id: &str) -> Result<usize, BrowserAuthError> {
        if let Some(postgres) = self.postgres.as_ref() {
            return postgres
                .revoke_tenant_sessions(tenant_id, now_ms())
                .await
                .map_err(|error| BrowserAuthError::Upstream(error.to_string()));
        }
        let revoked = sqlx::query(
            "UPDATE enterprise_sessions SET revoked_at = ?
             WHERE tenant_id = ? AND revoked_at IS NULL",
        )
        .bind(now_ms())
        .bind(tenant_id)
        .execute(&self.control)
        .await?;
        Ok(revoked.rows_affected() as usize)
    }

    async fn stored_session(
        &self,
        cookie_header: Option<&str>,
    ) -> Result<Option<StoredSession>, BrowserAuthError> {
        let Some(token) = cookie_value(cookie_header, SESSION_COOKIE) else {
            return Ok(None);
        };
        self.stored_session_by_hash(&sha256_hex(token.as_bytes()))
            .await
    }

    async fn stored_session_by_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<StoredSession>, BrowserAuthError> {
        if let Some(postgres) = self.postgres.as_ref()
            && let Some(record) = postgres
                .session_by_hash(token_hash)
                .await
                .map_err(|error| BrowserAuthError::Upstream(error.to_string()))?
        {
            return Ok(Some(stored_from_record(record)));
        }
        Ok(sqlx::query_as(
            "SELECT id, csrf_token, tenant_id, subject, email, display_name, role,
                    platform_admin, expires_at, revoked_at,
                    refresh_token_ciphertext, refresh_token_version,
                    refresh_token_expires_at, id_token_ciphertext,
                    access_token_expires_at
             FROM enterprise_sessions WHERE token_hash = ?",
        )
        .bind(token_hash)
        .fetch_optional(&self.control)
        .await?)
    }

    #[allow(dead_code)]
    async fn revoke_session_id(&self, session_id: &str) -> Result<(), BrowserAuthError> {
        if let Some(postgres) = self.postgres.as_ref() {
            postgres
                .revoke_session_by_id(session_id, now_ms())
                .await
                .map_err(|error| BrowserAuthError::Upstream(error.to_string()))?;
            return Ok(());
        }
        sqlx::query(
            "UPDATE enterprise_sessions SET revoked_at = ?
             WHERE id = ? AND revoked_at IS NULL",
        )
        .bind(now_ms())
        .bind(session_id)
        .execute(&self.control)
        .await?;
        Ok(())
    }

    async fn logout_url(
        &self,
        client_id: &str,
        endpoint: &str,
        redirect_uri: &str,
        id_token: &str,
    ) -> Result<String, BrowserAuthError> {
        let state = random_token()?;
        let state_hash = sha256_hex(state.as_bytes());
        let now = now_ms();
        sqlx::query(
            "INSERT INTO enterprise_logout_flows (state_hash, created_at, expires_at)
             VALUES (?, ?, ?)",
        )
        .bind(&state_hash)
        .bind(now)
        .bind(now.saturating_add(FLOW_TTL_MS))
        .execute(&self.control)
        .await?;
        let mut url = Url::parse(endpoint)
            .map_err(|_| BrowserAuthError::Invalid("logout_endpoint_invalid".to_string()))?;
        url.set_fragment(None);
        url.query_pairs_mut()
            .append_pair("id_token_hint", id_token)
            .append_pair("client_id", client_id)
            .append_pair("post_logout_redirect_uri", redirect_uri)
            .append_pair("state", &state);
        Ok(url.to_string())
    }

    fn encryption_key(&self) -> Result<Zeroizing<Vec<u8>>, BrowserAuthError> {
        let directory = self.ops.data_dir.join(KEY_DIRECTORY);
        fs::create_dir_all(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        }
        let path = directory.join(KEY_FILE);
        if let Ok(metadata) = fs::symlink_metadata(&path)
            && metadata.file_type().is_symlink()
        {
            return Err(BrowserAuthError::Crypto(
                "browser auth key must not be a symbolic link".to_string(),
            ));
        }
        match File::open(&path) {
            Ok(file) => read_encryption_key(file, &path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut bytes = [0u8; 32];
                getrandom::getrandom(&mut bytes)
                    .map_err(|error| BrowserAuthError::Crypto(error.to_string()))?;
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                match options.open(&path) {
                    Ok(mut file) => {
                        file.write_all(&bytes)?;
                        file.sync_all()?;
                        Ok(Zeroizing::new(bytes.to_vec()))
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        read_encryption_key(File::open(&path)?, &path)
                    }
                    Err(error) => Err(error.into()),
                }
            }
            Err(error) => Err(error.into()),
        }
    }

    fn encrypt_secret(&self, plaintext: &str, context: &str) -> Result<String, BrowserAuthError> {
        let key = self.encryption_key()?;
        let unbound = UnboundKey::new(&AES_256_GCM, &key)
            .map_err(|_| BrowserAuthError::Crypto("invalid encryption key".to_string()))?;
        let key = LessSafeKey::new(unbound);
        let mut nonce_bytes = [0u8; 12];
        getrandom::getrandom(&mut nonce_bytes)
            .map_err(|error| BrowserAuthError::Crypto(error.to_string()))?;
        let nonce = Nonce::assume_unique_for_key(nonce_bytes);
        let mut ciphertext = plaintext.as_bytes().to_vec();
        key.seal_in_place_append_tag(nonce, Aad::from(context.as_bytes()), &mut ciphertext)
            .map_err(|_| BrowserAuthError::Crypto("token encryption failed".to_string()))?;
        Ok(format!(
            "{SECRET_ENVELOPE_VERSION}:{}:{}",
            URL_SAFE_NO_PAD.encode(nonce_bytes),
            URL_SAFE_NO_PAD.encode(ciphertext)
        ))
    }

    fn decrypt_secret(&self, envelope: &str, context: &str) -> Result<String, BrowserAuthError> {
        let mut parts = envelope.split(':');
        if parts.next() != Some(SECRET_ENVELOPE_VERSION) {
            return Err(BrowserAuthError::Crypto(
                "unsupported token envelope".to_string(),
            ));
        }
        let nonce = parts
            .next()
            .and_then(|value| URL_SAFE_NO_PAD.decode(value).ok())
            .filter(|value| value.len() == 12)
            .ok_or_else(|| BrowserAuthError::Crypto("invalid token nonce".to_string()))?;
        let mut ciphertext = parts
            .next()
            .and_then(|value| URL_SAFE_NO_PAD.decode(value).ok())
            .ok_or_else(|| BrowserAuthError::Crypto("invalid token ciphertext".to_string()))?;
        if parts.next().is_some() {
            return Err(BrowserAuthError::Crypto(
                "invalid token envelope".to_string(),
            ));
        }
        let key = self.encryption_key()?;
        let unbound = UnboundKey::new(&AES_256_GCM, &key)
            .map_err(|_| BrowserAuthError::Crypto("invalid encryption key".to_string()))?;
        let key = LessSafeKey::new(unbound);
        let nonce_bytes: [u8; 12] = nonce
            .try_into()
            .map_err(|_| BrowserAuthError::Crypto("invalid token nonce".to_string()))?;
        let plaintext = key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce_bytes),
                Aad::from(context.as_bytes()),
                &mut ciphertext,
            )
            .map_err(|_| BrowserAuthError::Crypto("token authentication failed".to_string()))?;
        String::from_utf8(plaintext.to_vec())
            .map_err(|_| BrowserAuthError::Crypto("decrypted token is not UTF-8".to_string()))
    }

    async fn config(&self) -> Result<Option<BrowserConfig>, BrowserAuthError> {
        let value = self
            .ops
            .get_oidc_config()
            .await
            .map_err(browser_ops_error)?;
        Ok(BrowserConfig::from_value(&value))
    }

    async fn take_flow(&self, state: &str) -> Result<StoredFlow, BrowserAuthError> {
        let state_hash = sha256_hex(state.as_bytes());
        let row: Option<(String, String, String, i64)> = sqlx::query_as(
            "SELECT code_verifier, nonce, redirect, expires_at
             FROM enterprise_auth_flows WHERE state_hash = ?",
        )
        .bind(&state_hash)
        .fetch_optional(&self.control)
        .await?;
        let Some((code_verifier, nonce, redirect, expires_at)) = row else {
            return Err(BrowserAuthError::Invalid("state_invalid".to_string()));
        };
        let deleted = sqlx::query("DELETE FROM enterprise_auth_flows WHERE state_hash = ?")
            .bind(&state_hash)
            .execute(&self.control)
            .await?;
        if deleted.rows_affected() == 0 {
            return Err(BrowserAuthError::Invalid("state_invalid".to_string()));
        }
        if expires_at <= now_ms() {
            return Err(BrowserAuthError::Invalid("state_invalid".to_string()));
        }
        Ok(StoredFlow {
            code_verifier,
            nonce,
            redirect,
        })
    }

    async fn ensure_active_tenant(&self, tenant_id: &str) -> Result<(), BrowserAuthError> {
        if let Some(postgres) = self.postgres.as_ref() {
            let active = postgres
                .tenant(tenant_id)
                .await
                .map_err(browser_control_error)?
                .is_some_and(|tenant| tenant.status == "active");
            if !active {
                return Err(BrowserAuthError::Forbidden("tenant_inactive".to_string()));
            }
            return Ok(());
        }
        let status: Option<String> = sqlx::query_scalar(
            "SELECT status FROM enterprise_tenants WHERE id = ? AND status = 'active'",
        )
        .bind(tenant_id)
        .fetch_optional(&self.control)
        .await?;
        if status.is_none() {
            return Err(BrowserAuthError::Forbidden("tenant_inactive".to_string()));
        }
        Ok(())
    }

    async fn find_member(
        &self,
        tenant_id: &str,
        subject: &str,
        email: Option<&str>,
    ) -> Result<Option<MemberRecord>, BrowserAuthError> {
        if let Some(postgres) = self.postgres.as_ref() {
            let member = postgres
                .scim_member_for_identity(tenant_id, subject, email)
                .await
                .map_err(browser_control_error)?;
            return Ok(member.map(|member| MemberRecord {
                display_name: member.display_name,
                role: member.role,
                status: member.status,
            }));
        }
        let row = sqlx::query(
            "SELECT display_name, role, status FROM enterprise_members
             WHERE tenant_id = ?
               AND (subject = ?
                    OR (? IS NOT NULL AND email IS NOT NULL
                        AND lower(email) = lower(?)))
             ORDER BY CASE WHEN subject = ? THEN 0 ELSE 1 END
             LIMIT 1",
        )
        .bind(tenant_id)
        .bind(subject)
        .bind(email)
        .bind(email)
        .bind(subject)
        .fetch_optional(&self.control)
        .await?;
        Ok(row.map(|row| MemberRecord {
            display_name: row.get("display_name"),
            role: row.get("role"),
            status: row.get("status"),
        }))
    }

    #[allow(clippy::too_many_arguments)]
    async fn insert_session(
        &self,
        tenant_id: &str,
        subject: &str,
        email: Option<&str>,
        display_name: Option<&str>,
        role: &str,
        platform_admin: bool,
        ttl_seconds: i64,
        grant: TokenGrant,
    ) -> Result<CreatedSession, BrowserAuthError> {
        let token = random_token()?;
        let csrf_token = random_token()?;
        let token_hash = sha256_hex(token.as_bytes());
        let id = format!("sess_{}", Uuid::now_v7());
        let now = now_ms();
        let expires_at = now
            .saturating_add(ttl_seconds.max(1).saturating_mul(1000))
            .min(grant.refresh_token_expires_at.unwrap_or(i64::MAX));
        let refresh_token_ciphertext = grant
            .refresh_token
            .as_deref()
            .map(|token| self.encrypt_secret(token, &format!("refresh-token:{id}")))
            .transpose()?;
        let refresh_token_version = i64::from(refresh_token_ciphertext.is_some());
        let id_token_ciphertext = grant
            .id_token
            .as_deref()
            .map(|token| self.encrypt_secret(token, &format!("id-token:{id}")))
            .transpose()?;
        if let Some(postgres) = self.postgres.as_ref() {
            postgres
                .create_session(&SessionRecord {
                    id: id.clone(),
                    token_hash: token_hash.clone(),
                    csrf_token: csrf_token.clone(),
                    tenant_id: tenant_id.to_string(),
                    subject: subject.to_string(),
                    email: email.map(str::to_string),
                    display_name: display_name.map(str::to_string),
                    role: role.to_string(),
                    platform_admin,
                    created_at: now,
                    expires_at,
                    revoked_at: None,
                    last_seen_at: Some(now),
                    refresh_token_ciphertext: refresh_token_ciphertext.clone(),
                    refresh_token_version: i32::try_from(refresh_token_version).unwrap_or_default(),
                    refresh_token_expires_at: grant.refresh_token_expires_at,
                    id_token_ciphertext: id_token_ciphertext.clone(),
                    access_token_expires_at: Some(grant.access_token_expires_at),
                    last_refresh_at: None,
                })
                .await
                .map_err(|error| BrowserAuthError::Upstream(error.to_string()))?;
            return Ok(CreatedSession {
                id,
                token,
                csrf_token,
                expires_at,
                access_token_expires_at: Some(grant.access_token_expires_at),
                refresh_available: refresh_token_ciphertext.is_some(),
            });
        }
        sqlx::query(
            "INSERT INTO enterprise_sessions
             (id, token_hash, csrf_token, tenant_id, subject, email, display_name, role,
              platform_admin, created_at, expires_at, revoked_at, last_seen_at,
              refresh_token_ciphertext, refresh_token_version,
              refresh_token_expires_at, id_token_ciphertext,
              access_token_expires_at, last_refresh_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, ?, ?, ?, ?, ?, ?, NULL)",
        )
        .bind(&id)
        .bind(&token_hash)
        .bind(&csrf_token)
        .bind(tenant_id)
        .bind(subject)
        .bind(email)
        .bind(display_name)
        .bind(role)
        .bind(platform_admin as i64)
        .bind(now)
        .bind(expires_at)
        .bind(now)
        .bind(&refresh_token_ciphertext)
        .bind(refresh_token_version)
        .bind(grant.refresh_token_expires_at)
        .bind(&id_token_ciphertext)
        .bind(grant.access_token_expires_at)
        .execute(&self.control)
        .await?;
        Ok(CreatedSession {
            id,
            token,
            csrf_token,
            expires_at,
            access_token_expires_at: Some(grant.access_token_expires_at),
            refresh_available: refresh_token_ciphertext.is_some(),
        })
    }
}

fn read_encryption_key(
    mut file: File,
    path: &std::path::Path,
) -> Result<Zeroizing<Vec<u8>>, BrowserAuthError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    let metadata = file.metadata()?;
    if metadata.len() != 32 {
        return Err(BrowserAuthError::Crypto(format!(
            "browser auth key must contain exactly 32 bytes: {}",
            path.display()
        )));
    }
    let mut key = Zeroizing::new(vec![0u8; 32]);
    file.read_exact(&mut key)?;
    Ok(key)
}

fn upstream_client() -> Result<reqwest::Client, BrowserAuthError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| BrowserAuthError::Upstream(error.to_string()))
}

fn parse_token_grant(document: &Value, now: i64) -> Result<TokenGrant, BrowserAuthError> {
    let access_token = document
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            BrowserAuthError::Upstream("token response has no access_token".to_string())
        })?;
    if access_token.len() > 65_536 {
        return Err(BrowserAuthError::Upstream(
            "token response access_token is too large".to_string(),
        ));
    }
    if let Some(token_type) = document.get("token_type").and_then(Value::as_str)
        && !token_type.eq_ignore_ascii_case("bearer")
    {
        return Err(BrowserAuthError::Upstream(
            "token response token_type is not Bearer".to_string(),
        ));
    }
    let expires_in = document
        .get("expires_in")
        .and_then(Value::as_i64)
        .filter(|value| (1..=MAX_ACCESS_TOKEN_TTL).contains(value))
        .ok_or_else(|| {
            BrowserAuthError::Upstream(
                "token response expires_in must be a positive integer".to_string(),
            )
        })?;
    let refresh_expires_in = document
        .get("refresh_expires_in")
        .map(|value| {
            value
                .as_i64()
                .filter(|value| (1..=MAX_REFRESH_TOKEN_TTL).contains(value))
                .ok_or_else(|| {
                    BrowserAuthError::Upstream(
                        "token response refresh_expires_in is invalid".to_string(),
                    )
                })
        })
        .transpose()?;
    let id_token = optional_nonempty_string(document, "id_token")?;
    let refresh_token = optional_nonempty_string(document, "refresh_token")?;
    Ok(TokenGrant {
        id_token,
        refresh_token,
        access_token_expires_at: now.saturating_add(expires_in.saturating_mul(1000)),
        refresh_token_expires_at: refresh_expires_in
            .map(|seconds| now.saturating_add(seconds.saturating_mul(1000))),
    })
}

fn optional_nonempty_string(
    document: &Value,
    field: &str,
) -> Result<Option<String>, BrowserAuthError> {
    match document.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if !value.is_empty() && value.len() <= 65_536 => {
            Ok(Some(value.clone()))
        }
        Some(_) => Err(BrowserAuthError::Upstream(format!(
            "token response {field} is invalid"
        ))),
    }
}

fn stored_from_record(record: SessionRecord) -> StoredSession {
    StoredSession {
        id: record.id,
        csrf_token: record.csrf_token,
        tenant_id: record.tenant_id,
        subject: record.subject,
        email: record.email,
        display_name: record.display_name,
        role: record.role,
        platform_admin: i64::from(record.platform_admin),
        expires_at: record.expires_at,
        revoked_at: record.revoked_at,
        refresh_token_ciphertext: record.refresh_token_ciphertext,
        refresh_token_version: i64::from(record.refresh_token_version),
        refresh_token_expires_at: record.refresh_token_expires_at,
        id_token_ciphertext: record.id_token_ciphertext,
        access_token_expires_at: record.access_token_expires_at,
    }
}

fn principal_from_stored(stored: StoredSession, role: String) -> SessionPrincipal {
    SessionPrincipal {
        session_id: stored.id,
        csrf_token: stored.csrf_token,
        tenant_id: stored.tenant_id,
        subject: stored.subject,
        email: stored.email,
        display_name: stored.display_name,
        role,
        platform_admin: stored.platform_admin != 0,
        expires_at: stored.expires_at,
        access_token_expires_at: stored.access_token_expires_at,
        refresh_available: stored.refresh_token_ciphertext.is_some()
            && stored.refresh_token_version > 0
            && stored
                .refresh_token_expires_at
                .is_none_or(|expires_at| expires_at > now_ms()),
    }
}

#[allow(dead_code)]
pub fn clear_cookie(secure: bool) -> String {
    let mut cookie = format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0");
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

fn browser_control_error(error: ControlError) -> BrowserAuthError {
    match error {
        ControlError::Db(error) => BrowserAuthError::Db(error),
        ControlError::Json(error) => BrowserAuthError::Json(error),
        ControlError::Invalid(message) => BrowserAuthError::Invalid(message),
        ControlError::NotFound(message) => BrowserAuthError::NotFound(message),
    }
}

fn browser_ops_error(error: OpsError) -> BrowserAuthError {
    match error {
        OpsError::Db(error) => BrowserAuthError::Db(error),
        OpsError::Json(error) => BrowserAuthError::Json(error),
        OpsError::Io(error) => BrowserAuthError::Upstream(error.to_string()),
        OpsError::Invalid(message) => BrowserAuthError::Invalid(message),
        OpsError::NotFound(message) => BrowserAuthError::NotFound(message),
        OpsError::Upstream(message) => BrowserAuthError::Upstream(message),
        OpsError::Forbidden(message) => BrowserAuthError::Forbidden(message),
    }
}

fn validate_redirect(redirect: Option<&str>) -> Result<String, BrowserAuthError> {
    let redirect = redirect.unwrap_or("/");
    if redirect.is_empty()
        || !redirect.starts_with('/')
        || redirect.starts_with("//")
        || redirect.contains('\\')
        || redirect.chars().any(char::is_control)
    {
        return Err(BrowserAuthError::Invalid("redirect_invalid".to_string()));
    }
    Ok(redirect.to_string())
}

fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn random_token() -> Result<String, BrowserAuthError> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes)
        .map_err(|error| BrowserAuthError::Upstream(error.to_string()))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn session_cookie(token: &str, ttl_seconds: i64, cookie_secure: bool) -> String {
    let mut cookie =
        format!("{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={ttl_seconds}");
    if cookie_secure {
        cookie.push_str("; Secure");
    }
    cookie
}

fn cookie_value<'a>(cookie_header: Option<&'a str>, name: &str) -> Option<&'a str> {
    for part in cookie_header?.split(';') {
        let part = part.trim();
        if let Some((key, value)) = part.split_once('=')
            && key == name
            && !value.is_empty()
        {
            return Some(value);
        }
    }
    None
}

fn is_platform_admin(subjects: &[String], subject: &str, email: Option<&str>) -> bool {
    subjects.iter().any(|entry| {
        entry == subject || email.is_some_and(|email| entry.eq_ignore_ascii_case(email))
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::json;
    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;

    async fn test_auth() -> BrowserAuth {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let ops = OpsService::new(
            pool.clone(),
            std::env::temp_dir().join(format!("hpa-browser-{}", Uuid::now_v7())),
        );
        ops.ensure_schema().await.unwrap();
        let auth = BrowserAuth::new(pool.clone(), ops);
        auth.ensure_schema().await.unwrap();
        sqlx::query("CREATE TABLE enterprise_tenants (id TEXT PRIMARY KEY, status TEXT NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE enterprise_members (
                id TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL,
                subject TEXT NOT NULL,
                display_name TEXT NOT NULL,
                email TEXT,
                role TEXT NOT NULL,
                status TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO enterprise_tenants (id, status) VALUES ('acme', 'active')")
            .execute(&pool)
            .await
            .unwrap();
        auth
    }

    async fn configure_local(auth: &BrowserAuth) {
        auth.ops
            .set_browser_oidc_config(
                "https://issuer.example/",
                "hpa",
                "http://127.0.0.1:9/jwks",
                vec!["*".to_string()],
                "client-1",
                "http://127.0.0.1:9/authorize",
                "http://127.0.0.1:9/token",
                "http://127.0.0.1:9/auth/oidc/callback",
                "openid email profile",
                vec!["admin@example.com".to_string()],
                false,
                3600,
                "",
                "",
            )
            .await
            .unwrap();
    }

    fn encode(value: &Value) -> String {
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(value).unwrap())
    }

    fn ed_token(signing: &SigningKey, header: Value, claims: Value) -> String {
        let input = format!("{}.{}", encode(&header), encode(&claims));
        let signature = signing.sign(input.as_bytes());
        format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature.to_bytes()))
    }

    fn idp_server(jwks: String, id_token: String) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for _ in 0..4 {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let mut buffer = [0u8; 8192];
                let read = stream.read(&mut buffer).unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..read]);
                let body = if request.starts_with("GET /jwks") {
                    jwks.clone()
                } else {
                    json!({
                        "access_token": "access-token",
                        "token_type": "Bearer",
                        "expires_in": 600,
                        "refresh_token": "refresh-token",
                        "id_token": id_token
                    })
                    .to_string()
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        port
    }

    #[test]
    fn pkce_s256_matches_rfc_vector() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn redirect_validator_rejects_escapes() {
        assert_eq!(validate_redirect(None).unwrap(), "/");
        assert_eq!(validate_redirect(Some("/")).unwrap(), "/");
        assert_eq!(
            validate_redirect(Some("/enterprise?tab=members")).unwrap(),
            "/enterprise?tab=members"
        );
        assert!(validate_redirect(Some("//evil.example")).is_err());
        assert!(validate_redirect(Some("https://evil.example")).is_err());
        assert!(validate_redirect(Some("/\\evil.example")).is_err());
        assert!(validate_redirect(Some("/ok\nSet-Cookie: x")).is_err());
    }

    fn test_grant() -> TokenGrant {
        TokenGrant {
            id_token: None,
            refresh_token: None,
            access_token_expires_at: now_ms() + 600_000,
            refresh_token_expires_at: None,
        }
    }

    #[tokio::test]
    async fn browser_tokens_are_encrypted_and_authenticated_at_rest() {
        let auth = test_auth().await;
        let grant = TokenGrant {
            id_token: Some("sensitive-id-token".to_string()),
            refresh_token: Some("sensitive-refresh-token".to_string()),
            access_token_expires_at: now_ms() + 600_000,
            refresh_token_expires_at: Some(now_ms() + 86_400_000),
        };
        let session = auth
            .insert_session(
                "acme",
                "sub-crypto",
                Some("crypto@example.com"),
                Some("Crypto User"),
                "developer",
                false,
                3600,
                grant,
            )
            .await
            .unwrap();
        let (refresh_ciphertext, id_ciphertext): (String, String) = sqlx::query_as(
            "SELECT refresh_token_ciphertext, id_token_ciphertext
             FROM enterprise_sessions WHERE id = ?",
        )
        .bind(&session.id)
        .fetch_one(&auth.control)
        .await
        .unwrap();
        assert_ne!(refresh_ciphertext, "sensitive-refresh-token");
        assert_ne!(id_ciphertext, "sensitive-id-token");
        assert!(!refresh_ciphertext.contains("sensitive-refresh-token"));
        assert_eq!(
            auth.decrypt_secret(
                &refresh_ciphertext,
                &format!("refresh-token:{}", session.id)
            )
            .unwrap(),
            "sensitive-refresh-token"
        );
        assert!(
            auth.decrypt_secret(&id_ciphertext, &format!("refresh-token:{}", session.id))
                .is_err()
        );
        let (prefix, ciphertext) = refresh_ciphertext.rsplit_once(':').unwrap();
        let mut tampered = ciphertext.as_bytes().to_vec();
        tampered[0] = if tampered[0] == b'A' { b'B' } else { b'A' };
        assert!(
            auth.decrypt_secret(
                &format!("{prefix}:{}", String::from_utf8(tampered).unwrap()),
                &format!("refresh-token:{}", session.id)
            )
            .is_err()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(auth.ops.data_dir.join(KEY_DIRECTORY).join(KEY_FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[tokio::test]
    async fn flow_is_one_time_and_expires() {
        let auth = test_auth().await;
        configure_local(&auth).await;
        let start = auth.begin_login(Some("/enterprise")).await.unwrap();
        let url = Url::parse(&start.location).unwrap();
        let params: HashMap<String, String> = url.query_pairs().into_owned().collect();
        assert_eq!(
            params.get("response_type").map(String::as_str),
            Some("code")
        );
        assert_eq!(
            params.get("client_id").map(String::as_str),
            Some("client-1")
        );
        assert_eq!(
            params.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        let state = params.get("state").unwrap().clone();
        let challenge = params.get("code_challenge").unwrap().clone();
        let flow = auth.take_flow(&state).await.unwrap();
        assert_eq!(pkce_challenge(&flow.code_verifier), challenge);
        assert_eq!(flow.redirect, "/enterprise");
        assert!(auth.take_flow(&state).await.is_err());

        let expired_state = "expired-state";
        sqlx::query(
            "INSERT INTO enterprise_auth_flows
             (state_hash, code_verifier, nonce, redirect, created_at, expires_at)
             VALUES (?, 'verifier', 'nonce', '/', ?, ?)",
        )
        .bind(sha256_hex(expired_state.as_bytes()))
        .bind(now_ms() - 1_000)
        .bind(now_ms() - 1)
        .execute(&auth.control)
        .await
        .unwrap();
        assert!(auth.take_flow(expired_state).await.is_err());
    }

    #[tokio::test]
    async fn sessions_support_lookup_csrf_expiry_and_revoke() {
        let auth = test_auth().await;
        let created = auth
            .insert_session(
                "acme",
                "sub-1",
                Some("user@example.com"),
                Some("User One"),
                "developer",
                false,
                3600,
                test_grant(),
            )
            .await
            .unwrap();
        let cookie = format!("{SESSION_COOKIE}={}", created.token);
        let principal = auth.session(Some(&cookie)).await.unwrap().unwrap();
        assert_eq!(principal.tenant_id, "acme");
        assert_eq!(principal.role, "developer");
        assert_eq!(principal.display_name.as_deref(), Some("User One"));
        assert_eq!(principal.csrf_token, created.csrf_token);
        assert!(!principal.platform_admin);
        assert!(principal.expires_at > now_ms());
        assert!(
            auth.validate_csrf(Some(&cookie), Some(&created.csrf_token))
                .await
                .unwrap()
        );
        assert!(
            !auth
                .validate_csrf(Some(&cookie), Some("wrong"))
                .await
                .unwrap()
        );
        assert!(!auth.validate_csrf(Some(&cookie), None).await.unwrap());
        assert!(!auth.validate_csrf(None, Some("x")).await.unwrap());

        sqlx::query("UPDATE enterprise_sessions SET expires_at = ? WHERE id = ?")
            .bind(now_ms() - 1)
            .bind(&created.id)
            .execute(&auth.control)
            .await
            .unwrap();
        assert!(auth.session(Some(&cookie)).await.unwrap().is_none());

        let created = auth
            .insert_session(
                "acme",
                "sub-2",
                None,
                None,
                "viewer",
                false,
                3600,
                test_grant(),
            )
            .await
            .unwrap();
        let cookie = format!("{SESSION_COOKIE}={}", created.token);
        assert_eq!(auth.revoke_tenant_sessions("acme").await.unwrap(), 2);
        assert!(auth.session(Some(&cookie)).await.unwrap().is_none());
        assert!(!auth.logout(Some(&cookie)).await.unwrap().revoked);

        let created = auth
            .insert_session(
                "acme",
                "sub-3",
                None,
                None,
                "viewer",
                false,
                3600,
                test_grant(),
            )
            .await
            .unwrap();
        let cookie = format!("{SESSION_COOKIE}={}", created.token);
        assert!(auth.logout(Some(&cookie)).await.unwrap().revoked);
        assert!(auth.session(Some(&cookie)).await.unwrap().is_none());
        assert!(!auth.logout(None).await.unwrap().revoked);
    }

    #[tokio::test]
    async fn sessions_follow_member_and_tenant_status() {
        let auth = test_auth().await;
        sqlx::query(
            "INSERT INTO enterprise_members
             (id, tenant_id, subject, display_name, email, role, status)
             VALUES ('m1', 'acme', 'sub-1', 'Directory Name', 'user@example.com',
                     'auditor', 'active')",
        )
        .execute(&auth.control)
        .await
        .unwrap();
        let created = auth
            .insert_session(
                "acme",
                "sub-1",
                Some("USER@example.com"),
                Some("Token Name"),
                "developer",
                false,
                3600,
                test_grant(),
            )
            .await
            .unwrap();
        let cookie = format!("{SESSION_COOKIE}={}", created.token);
        let principal = auth.session(Some(&cookie)).await.unwrap().unwrap();
        assert_eq!(principal.role, "auditor");
        assert_eq!(principal.display_name.as_deref(), Some("Token Name"));

        sqlx::query("UPDATE enterprise_members SET status = 'disabled' WHERE id = 'm1'")
            .execute(&auth.control)
            .await
            .unwrap();
        assert!(matches!(
            auth.session(Some(&cookie)).await,
            Err(BrowserAuthError::Forbidden(_))
        ));

        sqlx::query("UPDATE enterprise_members SET status = 'active' WHERE id = 'm1'")
            .execute(&auth.control)
            .await
            .unwrap();
        sqlx::query("UPDATE enterprise_tenants SET status = 'disabled' WHERE id = 'acme'")
            .execute(&auth.control)
            .await
            .unwrap();
        assert!(matches!(
            auth.session(Some(&cookie)).await,
            Err(BrowserAuthError::Forbidden(_))
        ));
    }

    #[tokio::test]
    async fn complete_login_creates_session_and_consumes_state() {
        let auth = test_auth().await;
        sqlx::query(
            "INSERT INTO enterprise_members
             (id, tenant_id, subject, display_name, email, role, status)
             VALUES ('m1', 'acme', 'sub-1', 'Directory Name', 'user@example.com',
                     'auditor', 'active')",
        )
        .execute(&auth.control)
        .await
        .unwrap();
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let jwks = json!({"keys": [{
            "kty": "OKP",
            "crv": "Ed25519",
            "kid": "primary",
            "x": URL_SAFE_NO_PAD.encode(signing.verifying_key().to_bytes())
        }]})
        .to_string();
        let now = now_ms() / 1000;
        let id_token = ed_token(
            &signing,
            json!({"alg": "EdDSA", "typ": "JWT", "kid": "primary"}),
            json!({
                "iss": "https://issuer.example/",
                "aud": "hpa",
                "sub": "sub-1",
                "tenant_id": "acme",
                "role": "viewer",
                "email": "USER@example.com",
                "name": "Claim Name",
                "nonce": "flow-nonce",
                "exp": now + 3600
            }),
        );
        let port = idp_server(jwks, id_token);
        auth.ops
            .set_browser_oidc_config(
                "https://issuer.example/",
                "hpa",
                &format!("http://127.0.0.1:{port}/jwks"),
                vec!["*".to_string()],
                "client-1",
                &format!("http://127.0.0.1:{port}/authorize"),
                &format!("http://127.0.0.1:{port}/token"),
                &format!("http://127.0.0.1:{port}/auth/oidc/callback"),
                "openid email profile",
                vec!["user@example.com".to_string()],
                true,
                3600,
                "",
                "",
            )
            .await
            .unwrap();
        let state = "flow-state";
        sqlx::query(
            "INSERT INTO enterprise_auth_flows
             (state_hash, code_verifier, nonce, redirect, created_at, expires_at)
             VALUES (?, 'flow-verifier', 'flow-nonce', '/enterprise', ?, ?)",
        )
        .bind(sha256_hex(state.as_bytes()))
        .bind(now_ms())
        .bind(now_ms() + 600_000)
        .execute(&auth.control)
        .await
        .unwrap();
        let result = auth.complete_login("code-1", state).await.unwrap();
        assert_eq!(result.redirect, "/enterprise");
        assert_eq!(result.principal.tenant_id, "acme");
        assert_eq!(result.principal.role, "auditor");
        assert_eq!(
            result.principal.display_name.as_deref(),
            Some("Directory Name")
        );
        assert!(result.principal.platform_admin);
        assert!(result.cookie.contains("HttpOnly"));
        assert!(result.cookie.contains("SameSite=Lax"));
        assert!(result.cookie.contains("Path=/"));
        assert!(result.cookie.contains("; Secure"));
        assert!(result.principal.refresh_available);
        assert!(result.principal.access_token_expires_at.is_some());
        let principal = auth.session(Some(&result.cookie)).await.unwrap().unwrap();
        assert_eq!(principal.subject, "sub-1");
        assert!(auth.complete_login("code-1", state).await.is_err());
    }
}
