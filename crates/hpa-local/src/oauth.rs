//! Built-in OAuth 2.1 authorization server.
//!
//! Scope of this module: authorization code + PKCE (S256) for public and
//! confidential clients, dynamic client registration, refresh token rotation
//! with reuse detection, token revocation/introspection and a rotating JWKS.

use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::Signer;
use ring::signature::KeyPair;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::enterprise_ops::oidc_verify;

pub const SCOPE_READ: &str = "mcp:read";
pub const SCOPE_EXECUTE: &str = "mcp:execute";
pub const SCOPE_OFFLINE: &str = "offline_access";
pub const ALLOWED_SCOPES: [&str; 3] = [SCOPE_READ, SCOPE_EXECUTE, SCOPE_OFFLINE];

const REQUEST_TTL_MS: i64 = 600_000;
const CODE_TTL_MS: i64 = 120_000;
const ACCESS_TTL_SECONDS: i64 = 3_600;
const REFRESH_TTL_MS: i64 = 30 * 24 * 3_600 * 1_000;
const RETIRED_KEY_TTL_MS: i64 = 24 * 3_600 * 1_000;
const MAX_REDIRECT_URI: usize = 2_048;
const MAX_CLIENT_NAME: usize = 120;

#[derive(Debug, thiserror::Error)]
pub enum OauthError {
    #[error("invalid_request: {0}")]
    InvalidRequest(String),
    #[error("invalid_client: {0}")]
    InvalidClient(String),
    #[error("invalid_grant: {0}")]
    InvalidGrant(String),
    #[error("invalid_scope: {0}")]
    InvalidScope(String),
    #[error("unauthorized_client: {0}")]
    UnauthorizedClient(String),
    #[error("unsupported_grant_type: {0}")]
    UnsupportedGrantType(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("crypto error: {0}")]
    Crypto(String),
}

impl OauthError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidRequest(_) => "invalid_request",
            Self::InvalidClient(_) => "invalid_client",
            Self::InvalidGrant(_) => "invalid_grant",
            Self::InvalidScope(_) => "invalid_scope",
            Self::UnauthorizedClient(_) => "unauthorized_client",
            Self::UnsupportedGrantType(_) => "unsupported_grant_type",
            Self::NotFound(_) => "not_found",
            Self::Conflict(_) => "conflict",
            Self::Db(_) | Self::Json(_) => "server_error",
            Self::Crypto(_) => "server_error",
        }
    }

    pub fn description(&self) -> String {
        match self {
            Self::InvalidRequest(message)
            | Self::InvalidClient(message)
            | Self::InvalidGrant(message)
            | Self::InvalidScope(message)
            | Self::UnauthorizedClient(message)
            | Self::UnsupportedGrantType(message)
            | Self::NotFound(message)
            | Self::Conflict(message)
            | Self::Crypto(message) => message.clone(),
            Self::Db(error) => error.to_string(),
            Self::Json(error) => error.to_string(),
        }
    }

    pub fn is_server_error(&self) -> bool {
        matches!(self, Self::Db(_) | Self::Json(_) | Self::Crypto(_))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SigningAlg {
    Es256,
    EdDsa,
}

impl SigningAlg {
    pub fn from_env() -> Self {
        match std::env::var("HPA_OAUTH_SIGNING_ALG")
            .unwrap_or_default()
            .to_ascii_uppercase()
            .as_str()
        {
            "EDDSA" | "ED25519" => Self::EdDsa,
            _ => Self::Es256,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Es256 => "ES256",
            Self::EdDsa => "EdDSA",
        }
    }
}

#[derive(Clone, Debug)]
pub struct OauthClient {
    pub id: String,
    pub name: String,
    pub redirect_uris: Vec<String>,
    pub grant_types: Vec<String>,
    pub scopes: Vec<String>,
    pub auth_method: String,
    pub origin: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub disabled_at: Option<i64>,
    pub last_used_at: Option<i64>,
    pub has_secret: bool,
}

#[derive(Clone, Debug)]
pub struct OauthPendingRequest {
    pub id: String,
    pub client_id: String,
    pub client_name: String,
    pub redirect_uri: String,
    pub scope: String,
    pub state: Option<String>,
    pub resource: Option<String>,
    pub tenant_id: String,
    pub subject: String,
    pub expires_at: i64,
}

#[derive(Clone, Debug)]
pub struct OauthTokenPair {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub scope: String,
}

#[derive(Clone, Debug)]
pub struct OauthAccessToken {
    pub jti: String,
    pub client_id: String,
    pub tenant_id: String,
    pub subject: String,
    pub role: String,
    pub scopes: Vec<String>,
    pub audience: String,
    pub expires_at: i64,
}

#[derive(Clone, Debug)]
pub struct OauthTokenRow {
    pub id: String,
    pub kind: String,
    pub client_id: String,
    pub tenant_id: String,
    pub subject: String,
    pub scope: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub revoked_at: Option<i64>,
    pub family_id: String,
}

#[derive(Clone, Debug)]
struct StoredKey {
    kid: String,
    alg: SigningAlg,
    secret: Vec<u8>,
    public_jwk: Value,
}

#[derive(Clone)]
pub struct OauthService {
    control: SqlitePool,
    base_url: String,
    alg: SigningAlg,
}

impl OauthService {
    pub fn new(control: SqlitePool, base_url: String) -> Self {
        Self {
            control,
            base_url: base_url.trim_end_matches('/').to_string(),
            alg: SigningAlg::from_env(),
        }
    }

    pub fn issuer(&self) -> &str {
        &self.base_url
    }

    pub fn signing_alg_name(&self) -> &'static str {
        self.alg.name()
    }

    /// Best-effort check used for clearer error messages: a compact JWT whose
    /// unverified `iss` claim is this server was issued by the built-in AS.
    pub fn is_own_access_token(&self, token: &str) -> bool {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 3 {
            return false;
        }
        let Ok(bytes) = URL_SAFE_NO_PAD.decode(parts[1]) else {
            return false;
        };
        let Ok(claims) = serde_json::from_slice::<Value>(&bytes) else {
            return false;
        };
        claims.get("iss").and_then(Value::as_str) == Some(self.issuer())
    }

    /// Verifies a client secret without issuing tokens (used by `/oauth/revoke`).
    pub async fn verify_client_secret(
        &self,
        client_id: &str,
        secret: Option<&str>,
    ) -> Result<bool, OauthError> {
        let Some(client) = self.client(client_id).await? else {
            return Ok(false);
        };
        match self.authenticate_client(client_id, secret, &client).await {
            Ok(()) => Ok(true),
            Err(OauthError::InvalidClient(_)) => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub fn resource(&self) -> String {
        format!("{}/mcp", self.base_url)
    }

    pub fn audience(&self) -> String {
        self.resource()
    }

    pub async fn ensure_schema(&self) -> Result<(), OauthError> {
        let statements = [
            "CREATE TABLE IF NOT EXISTS oauth_clients (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                redirect_uris TEXT NOT NULL,
                grant_types TEXT NOT NULL,
                scopes TEXT NOT NULL,
                auth_method TEXT NOT NULL,
                secret_hash TEXT,
                origin TEXT NOT NULL,
                tenant_id TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                disabled_at INTEGER,
                last_used_at INTEGER
            )",
            "CREATE TABLE IF NOT EXISTS oauth_signing_keys (
                kid TEXT PRIMARY KEY,
                alg TEXT NOT NULL,
                secret TEXT NOT NULL,
                public_jwk TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                retired_at INTEGER,
                deleted_at INTEGER
            )",
            "CREATE TABLE IF NOT EXISTS oauth_requests (
                id TEXT PRIMARY KEY,
                client_id TEXT NOT NULL,
                redirect_uri TEXT NOT NULL,
                scope TEXT NOT NULL,
                state TEXT,
                resource TEXT,
                code_challenge TEXT NOT NULL,
                tenant_id TEXT NOT NULL,
                subject TEXT NOT NULL,
                role TEXT NOT NULL,
                platform_admin INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                consumed_at INTEGER,
                decision TEXT
            )",
            "CREATE TABLE IF NOT EXISTS oauth_codes (
                code_hash TEXT PRIMARY KEY,
                request_id TEXT NOT NULL,
                client_id TEXT NOT NULL,
                redirect_uri TEXT NOT NULL,
                scope TEXT NOT NULL,
                code_challenge TEXT NOT NULL,
                resource TEXT,
                tenant_id TEXT NOT NULL,
                subject TEXT NOT NULL,
                role TEXT NOT NULL,
                platform_admin INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                consumed_at INTEGER
            )",
            "CREATE TABLE IF NOT EXISTS oauth_tokens (
                id TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                token_hash TEXT NOT NULL UNIQUE,
                jti TEXT,
                family_id TEXT NOT NULL,
                client_id TEXT NOT NULL,
                tenant_id TEXT NOT NULL,
                subject TEXT NOT NULL,
                role TEXT NOT NULL,
                platform_admin INTEGER NOT NULL,
                scope TEXT NOT NULL,
                audience TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                revoked_at INTEGER,
                revoked_reason TEXT,
                rotated_to TEXT,
                used_at INTEGER
            )",
            "CREATE TABLE IF NOT EXISTS oauth_consents (
                client_id TEXT NOT NULL,
                tenant_id TEXT NOT NULL,
                subject TEXT NOT NULL,
                scope TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (client_id, tenant_id, subject)
            )",
            "CREATE INDEX IF NOT EXISTS oauth_tokens_client ON oauth_tokens (client_id, created_at DESC)",
            "CREATE UNIQUE INDEX IF NOT EXISTS oauth_tokens_jti ON oauth_tokens (jti) WHERE jti IS NOT NULL",
        ];
        for statement in statements {
            sqlx::query(statement).execute(&self.control).await?;
        }
        self.ensure_signing_key().await?;
        Ok(())
    }

    pub async fn metadata(&self) -> Result<Value, OauthError> {
        let key = self.active_key().await?;
        Ok(json!({
            "issuer": self.issuer(),
            "authorization_endpoint": format!("{}/oauth/authorize", self.base_url),
            "token_endpoint": format!("{}/oauth/token", self.base_url),
            "jwks_uri": format!("{}/oauth/jwks", self.base_url),
            "registration_endpoint": format!("{}/oauth/register", self.base_url),
            "revocation_endpoint": format!("{}/oauth/revoke", self.base_url),
            "introspection_endpoint": format!("{}/oauth/introspect", self.base_url),
            "scopes_supported": ALLOWED_SCOPES,
            "response_types_supported": ["code"],
            "response_modes_supported": ["query"],
            "grant_types_supported": ["authorization_code", "refresh_token"],
            "token_endpoint_auth_methods_supported": [
                "none",
                "client_secret_post",
                "client_secret_basic"
            ],
            "code_challenge_methods_supported": ["S256"],
            "id_token_signing_alg_values_supported": [key.alg.name()],
            "authorization_response_iss_parameter_supported": true,
            "service_documentation": format!("{}/settings", self.base_url),
        }))
    }

    pub async fn jwks(&self) -> Result<Value, OauthError> {
        let now = now_ms();
        let rows = sqlx::query(
            "SELECT public_jwk FROM oauth_signing_keys
             WHERE deleted_at IS NULL AND (retired_at IS NULL OR retired_at > ?)
             ORDER BY created_at DESC",
        )
        .bind(now - RETIRED_KEY_TTL_MS)
        .fetch_all(&self.control)
        .await?;
        let keys = rows
            .iter()
            .filter_map(|row| {
                let raw: String = row.get("public_jwk");
                serde_json::from_str::<Value>(&raw).ok()
            })
            .collect::<Vec<_>>();
        Ok(json!({"keys": keys}))
    }

    async fn ensure_signing_key(&self) -> Result<(), OauthError> {
        let now = now_ms();
        sqlx::query(
            "UPDATE oauth_signing_keys SET deleted_at = ?
             WHERE deleted_at IS NULL AND retired_at IS NOT NULL AND retired_at <= ?",
        )
        .bind(now)
        .bind(now - RETIRED_KEY_TTL_MS)
        .execute(&self.control)
        .await?;
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT kid FROM oauth_signing_keys WHERE retired_at IS NULL AND deleted_at IS NULL LIMIT 1",
        )
        .fetch_optional(&self.control)
        .await?;
        if existing.is_some() {
            return Ok(());
        }
        self.create_key(self.alg).await?;
        Ok(())
    }

    async fn create_key(&self, alg: SigningAlg) -> Result<StoredKey, OauthError> {
        let kid = format!("k{}", &sha256_hex(Uuid::now_v7().as_bytes())[..16]);
        let now = now_ms();
        let (secret, public_jwk) = match alg {
            SigningAlg::Es256 => {
                let rng = ring::rand::SystemRandom::new();
                let document = ring::signature::EcdsaKeyPair::generate_pkcs8(
                    &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
                    &rng,
                )
                .map_err(|error| OauthError::Crypto(error.to_string()))?;
                let pair = ring::signature::EcdsaKeyPair::from_pkcs8(
                    &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
                    document.as_ref(),
                    &rng,
                )
                .map_err(|error| OauthError::Crypto(error.to_string()))?;
                let public = pair.public_key().as_ref().to_vec();
                if public.len() != 65 {
                    return Err(OauthError::Crypto(
                        "unexpected P-256 key length".to_string(),
                    ));
                }
                let jwk = json!({
                    "kty": "EC",
                    "crv": "P-256",
                    "x": URL_SAFE_NO_PAD.encode(&public[1..33]),
                    "y": URL_SAFE_NO_PAD.encode(&public[33..]),
                    "kid": kid,
                    "use": "sig",
                    "alg": "ES256"
                });
                (document.as_ref().to_vec(), jwk)
            }
            SigningAlg::EdDsa => {
                let mut seed = [0u8; 32];
                getrandom::getrandom(&mut seed)
                    .map_err(|error| OauthError::Crypto(error.to_string()))?;
                let signing = ed25519_dalek::SigningKey::from_bytes(&seed);
                let public = signing.verifying_key().to_bytes();
                let jwk = json!({
                    "kty": "OKP",
                    "crv": "Ed25519",
                    "x": URL_SAFE_NO_PAD.encode(public),
                    "kid": kid,
                    "use": "sig",
                    "alg": "EdDSA"
                });
                (seed.to_vec(), jwk)
            }
        };
        sqlx::query(
            "INSERT INTO oauth_signing_keys (kid, alg, secret, public_jwk, created_at, retired_at, deleted_at)
             VALUES (?, ?, ?, ?, ?, NULL, NULL)",
        )
        .bind(&kid)
        .bind(alg.name())
        .bind(URL_SAFE_NO_PAD.encode(&secret))
        .bind(public_jwk.to_string())
        .bind(now)
        .execute(&self.control)
        .await?;
        Ok(StoredKey {
            kid,
            alg,
            secret,
            public_jwk,
        })
    }

    pub async fn rotate_signing_key(&self) -> Result<Value, OauthError> {
        let now = now_ms();
        sqlx::query(
            "UPDATE oauth_signing_keys SET retired_at = ?
             WHERE retired_at IS NULL AND deleted_at IS NULL",
        )
        .bind(now)
        .execute(&self.control)
        .await?;
        let key = self.create_key(self.alg).await?;
        Ok(json!({
            "kid": key.kid,
            "alg": key.alg.name(),
            "rotatedAt": now,
        }))
    }

    async fn active_key(&self) -> Result<StoredKey, OauthError> {
        let row = sqlx::query(
            "SELECT kid, alg, secret, public_jwk FROM oauth_signing_keys
             WHERE retired_at IS NULL AND deleted_at IS NULL
             ORDER BY created_at DESC LIMIT 1",
        )
        .fetch_optional(&self.control)
        .await?;
        let Some(row) = row else {
            return self.create_key(self.alg).await;
        };
        key_from_row(&row)
    }

    async fn key_by_kid(&self, kid: &str) -> Result<Option<StoredKey>, OauthError> {
        let row = sqlx::query(
            "SELECT kid, alg, secret, public_jwk FROM oauth_signing_keys
             WHERE kid = ? AND deleted_at IS NULL",
        )
        .bind(kid)
        .fetch_optional(&self.control)
        .await?;
        match row {
            Some(row) => Ok(Some(key_from_row(&row)?)),
            None => Ok(None),
        }
    }

    fn sign(&self, key: &StoredKey, claims: &Value) -> Result<String, OauthError> {
        let header = json!({
            "alg": key.alg.name(),
            "typ": "at+jwt",
            "kid": key.kid,
        });
        let input = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header)?),
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims)?)
        );
        let signature = match key.alg {
            SigningAlg::Es256 => {
                let rng = ring::rand::SystemRandom::new();
                let pair = ring::signature::EcdsaKeyPair::from_pkcs8(
                    &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
                    &key.secret,
                    &rng,
                )
                .map_err(|error| OauthError::Crypto(error.to_string()))?;
                pair.sign(&rng, input.as_bytes())
                    .map_err(|error| OauthError::Crypto(error.to_string()))?
                    .as_ref()
                    .to_vec()
            }
            SigningAlg::EdDsa => {
                let seed: [u8; 32] =
                    key.secret.as_slice().try_into().map_err(|_| {
                        OauthError::Crypto("Ed25519 key length is invalid".to_string())
                    })?;
                let signing = ed25519_dalek::SigningKey::from_bytes(&seed);
                signing.sign(input.as_bytes()).to_bytes().to_vec()
            }
        };
        Ok(format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature)))
    }

    pub async fn verify_access_token(&self, token: &str) -> Result<OauthAccessToken, OauthError> {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 3 {
            return Err(OauthError::InvalidRequest(
                "token is not a compact JWT".to_string(),
            ));
        }
        let header: Value = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(parts[0])
                .map_err(|_| OauthError::InvalidRequest("token header is invalid".to_string()))?,
        )?;
        let kid = header
            .get("kid")
            .and_then(Value::as_str)
            .ok_or_else(|| OauthError::InvalidRequest("token has no kid".to_string()))?;
        let alg = header
            .get("alg")
            .and_then(Value::as_str)
            .ok_or_else(|| OauthError::InvalidRequest("token has no alg".to_string()))?;
        let key = self
            .key_by_kid(kid)
            .await?
            .ok_or_else(|| OauthError::InvalidRequest("token kid is not known".to_string()))?;
        if key.alg.name() != alg {
            return Err(OauthError::InvalidRequest(
                "token alg does not match its key".to_string(),
            ));
        }
        let document = json!({"keys": [key.public_jwk]});
        let keys = oidc_verify::parse_jwks(&document)
            .map_err(|error| OauthError::Crypto(error.to_string()))?;
        let signature = URL_SAFE_NO_PAD
            .decode(parts[2])
            .map_err(|_| OauthError::InvalidRequest("token signature is invalid".to_string()))?;
        oidc_verify::verify_signature(
            &keys[0].key,
            alg,
            format!("{}.{}", parts[0], parts[1]).as_bytes(),
            &signature,
        )
        .map_err(|error| OauthError::InvalidRequest(error.to_string()))?;
        let claims: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).map_err(|_| {
                OauthError::InvalidRequest("token payload is invalid".to_string())
            })?)?;
        let object = claims.as_object().ok_or_else(|| {
            OauthError::InvalidRequest("token payload is not an object".to_string())
        })?;
        let now = now_ms() / 1000;
        let exp = object
            .get("exp")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        if exp <= now {
            return Err(OauthError::InvalidRequest("token has expired".to_string()));
        }
        if object.get("iss").and_then(Value::as_str) != Some(self.issuer()) {
            return Err(OauthError::InvalidRequest(
                "token issuer does not match".to_string(),
            ));
        }
        let audience = object.get("aud").and_then(Value::as_str).unwrap_or("");
        if audience != self.audience() {
            return Err(OauthError::InvalidRequest(
                "token audience does not match".to_string(),
            ));
        }
        let jti = object
            .get("jti")
            .and_then(Value::as_str)
            .ok_or_else(|| OauthError::InvalidRequest("token has no jti".to_string()))?;
        let row = sqlx::query(
            "SELECT revoked_at, expires_at FROM oauth_tokens WHERE jti = ? AND kind = 'access'",
        )
        .bind(jti)
        .fetch_optional(&self.control)
        .await?;
        let Some(row) = row else {
            return Err(OauthError::InvalidRequest(
                "token is not registered".to_string(),
            ));
        };
        let revoked_at: Option<i64> = row.get("revoked_at");
        let stored_expires: i64 = row.get("expires_at");
        if revoked_at.is_some() || stored_expires <= now_ms() {
            return Err(OauthError::InvalidRequest("token is revoked".to_string()));
        }
        let scope = object.get("scope").and_then(Value::as_str).unwrap_or("");
        Ok(OauthAccessToken {
            jti: jti.to_string(),
            client_id: object
                .get("client_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            tenant_id: object
                .get("tenant_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            subject: object
                .get("sub")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            role: object
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("viewer")
                .to_string(),
            scopes: split_scopes(scope),
            audience: audience.to_string(),
            expires_at: exp,
        })
    }

    // ---- clients -------------------------------------------------------

    pub async fn register_client(
        &self,
        name: &str,
        redirect_uris: &[String],
        auth_method: &str,
        scopes: &[String],
        origin: &str,
    ) -> Result<(OauthClient, Option<String>), OauthError> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > MAX_CLIENT_NAME {
            return Err(OauthError::InvalidRequest(
                "client name must be 1-120 characters".to_string(),
            ));
        }
        if redirect_uris.is_empty() || redirect_uris.len() > 8 {
            return Err(OauthError::InvalidRequest(
                "client needs between 1 and 8 redirect URIs".to_string(),
            ));
        }
        for uri in redirect_uris {
            validate_redirect_uri(uri)?;
        }
        let auth_method = match auth_method {
            "" | "none" => "none",
            "client_secret_post" | "client_secret_basic" => auth_method,
            _ => {
                return Err(OauthError::InvalidRequest(
                    "token_endpoint_auth_method is not supported".to_string(),
                ));
            }
        };
        let scopes = normalize_scopes(scopes)?;
        let id = format!(
            "hpa_client_{}",
            &sha256_hex(Uuid::now_v7().as_bytes())[..24]
        );
        let secret = if auth_method == "none" {
            None
        } else {
            Some(format!("hpa_cs_{}", random_token()?))
        };
        let secret_hash = secret.as_deref().map(|value| sha256_hex(value.as_bytes()));
        let now = now_ms();
        sqlx::query(
            "INSERT INTO oauth_clients
             (id, name, redirect_uris, grant_types, scopes, auth_method, secret_hash, origin,
              tenant_id, created_at, updated_at, disabled_at, last_used_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, NULL, ?, ?, NULL, NULL)",
        )
        .bind(&id)
        .bind(name)
        .bind(serde_json::to_string(redirect_uris)?)
        .bind(serde_json::to_string(&[
            "authorization_code",
            "refresh_token",
        ])?)
        .bind(serde_json::to_string(&scopes)?)
        .bind(auth_method)
        .bind(secret_hash)
        .bind(origin)
        .bind(now)
        .bind(now)
        .execute(&self.control)
        .await?;
        let client = self
            .client(&id)
            .await?
            .ok_or_else(|| OauthError::NotFound("client was not persisted".to_string()))?;
        Ok((client, secret))
    }

    pub async fn client(&self, id: &str) -> Result<Option<OauthClient>, OauthError> {
        let row = sqlx::query(
            "SELECT id, name, redirect_uris, grant_types, scopes, auth_method, secret_hash,
                    origin, created_at, updated_at, disabled_at, last_used_at
             FROM oauth_clients WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.control)
        .await?;
        Ok(row.as_ref().map(client_from_row))
    }

    pub async fn list_clients(&self) -> Result<Vec<OauthClient>, OauthError> {
        let rows = sqlx::query(
            "SELECT id, name, redirect_uris, grant_types, scopes, auth_method, secret_hash,
                    origin, created_at, updated_at, disabled_at, last_used_at
             FROM oauth_clients ORDER BY created_at DESC",
        )
        .fetch_all(&self.control)
        .await?;
        Ok(rows.iter().map(client_from_row).collect())
    }

    pub async fn disable_client(&self, id: &str) -> Result<(), OauthError> {
        let now = now_ms();
        let updated =
            sqlx::query("UPDATE oauth_clients SET disabled_at = ?, updated_at = ? WHERE id = ?")
                .bind(now)
                .bind(now)
                .bind(id)
                .execute(&self.control)
                .await?;
        if updated.rows_affected() == 0 {
            return Err(OauthError::NotFound(format!("client `{id}` was not found")));
        }
        self.revoke_client_tokens(id, "client_disabled").await?;
        sqlx::query("DELETE FROM oauth_consents WHERE client_id = ?")
            .bind(id)
            .execute(&self.control)
            .await?;
        Ok(())
    }

    pub async fn rotate_client_secret(&self, id: &str) -> Result<String, OauthError> {
        let Some(client) = self.client(id).await? else {
            return Err(OauthError::NotFound(format!("client `{id}` was not found")));
        };
        if client.disabled_at.is_some() {
            return Err(OauthError::Conflict(
                "client is disabled; enable a new registration instead".to_string(),
            ));
        }
        if client.auth_method == "none" {
            return Err(OauthError::Conflict(
                "public clients do not use a client secret".to_string(),
            ));
        }
        let secret = format!("hpa_cs_{}", random_token()?);
        sqlx::query("UPDATE oauth_clients SET secret_hash = ?, updated_at = ? WHERE id = ?")
            .bind(sha256_hex(secret.as_bytes()))
            .bind(now_ms())
            .bind(id)
            .execute(&self.control)
            .await?;
        Ok(secret)
    }

    async fn authenticate_client(
        &self,
        client_id: &str,
        client_secret: Option<&str>,
        client: &OauthClient,
    ) -> Result<(), OauthError> {
        if client.disabled_at.is_some() {
            return Err(OauthError::UnauthorizedClient(
                "client is disabled".to_string(),
            ));
        }
        match client.auth_method.as_str() {
            "none" => Ok(()),
            "client_secret_post" | "client_secret_basic" => {
                let Some(secret) = client_secret.filter(|value| !value.is_empty()) else {
                    return Err(OauthError::InvalidClient(
                        "client authentication is required".to_string(),
                    ));
                };
                let row: Option<String> =
                    sqlx::query_scalar("SELECT secret_hash FROM oauth_clients WHERE id = ?")
                        .bind(client_id)
                        .fetch_optional(&self.control)
                        .await?;
                let Some(stored) = row else {
                    return Err(OauthError::InvalidClient("client is unknown".to_string()));
                };
                if !constant_time_eq(&sha256_hex(secret.as_bytes()), &stored) {
                    return Err(OauthError::InvalidClient(
                        "client secret does not match".to_string(),
                    ));
                }
                Ok(())
            }
            other => Err(OauthError::InvalidClient(format!(
                "unsupported client authentication method `{other}`"
            ))),
        }
    }

    // ---- authorization requests ---------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub async fn create_request(
        &self,
        client: &OauthClient,
        redirect_uri: &str,
        scope: &str,
        state: Option<&str>,
        resource: Option<&str>,
        code_challenge: &str,
        owner: &OauthOwner,
    ) -> Result<OauthPendingRequest, OauthError> {
        let id = format!("hpa_req_{}", random_token()?);
        let now = now_ms();
        sqlx::query(
            "INSERT INTO oauth_requests
             (id, client_id, redirect_uri, scope, state, resource, code_challenge, tenant_id,
              subject, role, platform_admin, created_at, expires_at, consumed_at, decision)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL)",
        )
        .bind(&id)
        .bind(&client.id)
        .bind(redirect_uri)
        .bind(scope)
        .bind(state)
        .bind(resource)
        .bind(code_challenge)
        .bind(&owner.tenant_id)
        .bind(&owner.subject)
        .bind(&owner.role)
        .bind(owner.platform_admin as i64)
        .bind(now)
        .bind(now + REQUEST_TTL_MS)
        .execute(&self.control)
        .await?;
        Ok(OauthPendingRequest {
            id,
            client_id: client.id.clone(),
            client_name: client.name.clone(),
            redirect_uri: redirect_uri.to_string(),
            scope: scope.to_string(),
            state: state.map(str::to_string),
            resource: resource.map(str::to_string),
            tenant_id: owner.tenant_id.clone(),
            subject: owner.subject.clone(),
            expires_at: now + REQUEST_TTL_MS,
        })
    }

    pub async fn pending_request(&self, id: &str) -> Result<OauthPendingRequest, OauthError> {
        let row = sqlx::query(
            "SELECT r.id, r.client_id, c.name AS client_name, r.redirect_uri, r.scope, r.state,
                    r.resource, r.tenant_id, r.subject, r.expires_at, r.consumed_at
             FROM oauth_requests r JOIN oauth_clients c ON c.id = r.client_id
             WHERE r.id = ?",
        )
        .bind(id)
        .fetch_optional(&self.control)
        .await?;
        let Some(row) = row else {
            return Err(OauthError::NotFound(
                "authorization request was not found".to_string(),
            ));
        };
        if row.get::<Option<i64>, _>("consumed_at").is_some() {
            return Err(OauthError::InvalidRequest(
                "authorization request was already used".to_string(),
            ));
        }
        if row.get::<i64, _>("expires_at") <= now_ms() {
            return Err(OauthError::InvalidRequest(
                "authorization request has expired".to_string(),
            ));
        }
        Ok(OauthPendingRequest {
            id: row.get("id"),
            client_id: row.get("client_id"),
            client_name: row.get("client_name"),
            redirect_uri: row.get("redirect_uri"),
            scope: row.get("scope"),
            state: row.get("state"),
            resource: row.get("resource"),
            tenant_id: row.get("tenant_id"),
            subject: row.get("subject"),
            expires_at: row.get("expires_at"),
        })
    }

    pub async fn has_consent(
        &self,
        client_id: &str,
        tenant_id: &str,
        subject: &str,
        scope: &str,
    ) -> Result<bool, OauthError> {
        let row: Option<String> = sqlx::query_scalar(
            "SELECT scope FROM oauth_consents WHERE client_id = ? AND tenant_id = ? AND subject = ?",
        )
        .bind(client_id)
        .bind(tenant_id)
        .bind(subject)
        .fetch_optional(&self.control)
        .await?;
        let Some(granted) = row else {
            return Ok(false);
        };
        let granted = split_scopes(&granted);
        Ok(split_scopes(scope)
            .iter()
            .all(|scope| granted.iter().any(|entry| entry == scope)))
    }

    /// Applies the resource owner's decision. Returns the browser redirect URL.
    pub async fn decide(
        &self,
        request_id: &str,
        owner: &OauthOwner,
        allow: bool,
        remember: bool,
    ) -> Result<String, OauthError> {
        let request = self.pending_request(request_id).await?;
        if request.tenant_id != owner.tenant_id || request.subject != owner.subject {
            return Err(OauthError::InvalidRequest(
                "authorization request belongs to another session".to_string(),
            ));
        }
        let now = now_ms();
        let decision = if allow { "allow" } else { "deny" };
        let consumed = sqlx::query(
            "UPDATE oauth_requests SET consumed_at = ?, decision = ?
             WHERE id = ? AND consumed_at IS NULL",
        )
        .bind(now)
        .bind(decision)
        .bind(request_id)
        .execute(&self.control)
        .await?;
        if consumed.rows_affected() == 0 {
            return Err(OauthError::InvalidRequest(
                "authorization request was already used".to_string(),
            ));
        }
        if !allow {
            return Ok(redirect_with(
                &request,
                None,
                Some(("error", "access_denied")),
            ));
        }
        if remember {
            sqlx::query(
                "INSERT INTO oauth_consents (client_id, tenant_id, subject, scope, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?)
                 ON CONFLICT(client_id, tenant_id, subject)
                 DO UPDATE SET scope = excluded.scope, updated_at = excluded.updated_at",
            )
            .bind(&request.client_id)
            .bind(&request.tenant_id)
            .bind(&request.subject)
            .bind(&request.scope)
            .bind(now)
            .bind(now)
            .execute(&self.control)
            .await?;
        }
        let challenge: String =
            sqlx::query_scalar("SELECT code_challenge FROM oauth_requests WHERE id = ?")
                .bind(request_id)
                .fetch_one(&self.control)
                .await?;
        let code = format!("hpa_ac_{}", random_token()?);
        sqlx::query(
            "INSERT INTO oauth_codes
             (code_hash, request_id, client_id, redirect_uri, scope, code_challenge, resource,
              tenant_id, subject, role, platform_admin, created_at, expires_at, consumed_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL)",
        )
        .bind(sha256_hex(code.as_bytes()))
        .bind(&request.id)
        .bind(&request.client_id)
        .bind(&request.redirect_uri)
        .bind(&request.scope)
        .bind(&challenge)
        .bind(&request.resource)
        .bind(&request.tenant_id)
        .bind(&request.subject)
        .bind(&owner.role)
        .bind(owner.platform_admin as i64)
        .bind(now)
        .bind(now + CODE_TTL_MS)
        .execute(&self.control)
        .await?;
        Ok(redirect_with(&request, Some(&code), None))
    }

    // ---- token endpoint -------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub async fn exchange_code(
        &self,
        code: &str,
        client_id: &str,
        client_secret: Option<&str>,
        redirect_uri: &str,
        code_verifier: &str,
    ) -> Result<OauthTokenPair, OauthError> {
        let client = self
            .client(client_id)
            .await?
            .ok_or_else(|| OauthError::InvalidClient("client is unknown".to_string()))?;
        self.authenticate_client(client_id, client_secret, &client)
            .await?;
        if code_verifier.len() < 43 || code_verifier.len() > 128 {
            return Err(OauthError::InvalidGrant(
                "code_verifier must be 43-128 characters".to_string(),
            ));
        }
        let hash = sha256_hex(code.as_bytes());
        let row = sqlx::query(
            "SELECT client_id, redirect_uri, scope, code_challenge, resource, tenant_id, subject,
                    role, platform_admin, expires_at, consumed_at
             FROM oauth_codes WHERE code_hash = ?",
        )
        .bind(&hash)
        .fetch_optional(&self.control)
        .await?;
        let Some(row) = row else {
            return Err(OauthError::InvalidGrant(
                "authorization code is not valid".to_string(),
            ));
        };
        if row.get::<Option<i64>, _>("consumed_at").is_some() {
            return Err(OauthError::InvalidGrant(
                "authorization code was already used".to_string(),
            ));
        }
        if row.get::<i64, _>("expires_at") <= now_ms() {
            return Err(OauthError::InvalidGrant(
                "authorization code has expired".to_string(),
            ));
        }
        if row.get::<String, _>("client_id") != client_id {
            return Err(OauthError::InvalidGrant(
                "authorization code was issued to another client".to_string(),
            ));
        }
        if row.get::<String, _>("redirect_uri") != redirect_uri {
            return Err(OauthError::InvalidGrant(
                "redirect_uri does not match the authorization request".to_string(),
            ));
        }
        let challenge: String = row.get("code_challenge");
        if URL_SAFE_NO_PAD.encode(Sha256::digest(code_verifier.as_bytes())) != challenge {
            return Err(OauthError::InvalidGrant(
                "code_verifier does not match the code challenge".to_string(),
            ));
        }
        let consumed = sqlx::query(
            "UPDATE oauth_codes SET consumed_at = ? WHERE code_hash = ? AND consumed_at IS NULL",
        )
        .bind(now_ms())
        .bind(&hash)
        .execute(&self.control)
        .await?;
        if consumed.rows_affected() == 0 {
            return Err(OauthError::InvalidGrant(
                "authorization code was already used".to_string(),
            ));
        }
        let scope: String = row.get("scope");
        let resource: Option<String> = row.get("resource");
        let audience = resource.unwrap_or_else(|| self.audience());
        self.issue_pair(
            client_id,
            &row.get::<String, _>("tenant_id"),
            &row.get::<String, _>("subject"),
            &row.get::<String, _>("role"),
            row.get::<i64, _>("platform_admin") != 0,
            &scope,
            &audience,
            None,
        )
        .await
    }

    pub async fn exchange_refresh(
        &self,
        refresh_token: &str,
        client_id: &str,
        client_secret: Option<&str>,
        requested_scope: Option<&str>,
    ) -> Result<OauthTokenPair, OauthError> {
        let client = self
            .client(client_id)
            .await?
            .ok_or_else(|| OauthError::InvalidClient("client is unknown".to_string()))?;
        self.authenticate_client(client_id, client_secret, &client)
            .await?;
        let hash = sha256_hex(refresh_token.as_bytes());
        let row = sqlx::query(
            "SELECT id, family_id, client_id, tenant_id, subject, role, platform_admin, scope,
                    audience, expires_at, revoked_at, rotated_to
             FROM oauth_tokens WHERE token_hash = ? AND kind = 'refresh'",
        )
        .bind(&hash)
        .fetch_optional(&self.control)
        .await?;
        let Some(row) = row else {
            return Err(OauthError::InvalidGrant(
                "refresh token is not valid".to_string(),
            ));
        };
        let stored_client: String = row.get("client_id");
        if stored_client != client_id {
            return Err(OauthError::InvalidGrant(
                "refresh token was issued to another client".to_string(),
            ));
        }
        let family_id: String = row.get("family_id");
        if row.get::<Option<i64>, _>("revoked_at").is_some()
            || row.get::<Option<String>, _>("rotated_to").is_some()
        {
            // Reuse of a rotated token invalidates the whole family.
            self.revoke_family(&family_id, "refresh_token_reuse")
                .await?;
            return Err(OauthError::InvalidGrant(
                "refresh token was already rotated".to_string(),
            ));
        }
        if row.get::<i64, _>("expires_at") <= now_ms() {
            self.revoke_family(&family_id, "refresh_token_expired")
                .await?;
            return Err(OauthError::InvalidGrant(
                "refresh token has expired".to_string(),
            ));
        }
        let granted: String = row.get("scope");
        let scope = match requested_scope.filter(|value| !value.trim().is_empty()) {
            None => granted.clone(),
            Some(requested) => {
                let requested = normalize_scopes(&split_scopes(requested))?;
                let granted_set = split_scopes(&granted);
                if !requested
                    .iter()
                    .all(|scope| granted_set.iter().any(|entry| entry == scope))
                {
                    return Err(OauthError::InvalidScope(
                        "refresh scope exceeds the original grant".to_string(),
                    ));
                }
                requested.join(" ")
            }
        };
        let id: String = row.get("id");
        sqlx::query(
            "UPDATE oauth_tokens SET used_at = ?, revoked_at = ?, revoked_reason = ? WHERE id = ?",
        )
        .bind(now_ms())
        .bind(now_ms())
        .bind("rotated")
        .bind(&id)
        .execute(&self.control)
        .await?;
        let audience: String = row.get("audience");
        let pair = self
            .issue_pair(
                client_id,
                &row.get::<String, _>("tenant_id"),
                &row.get::<String, _>("subject"),
                &row.get::<String, _>("role"),
                row.get::<i64, _>("platform_admin") != 0,
                &scope,
                &audience,
                Some(&family_id),
            )
            .await?;
        sqlx::query("UPDATE oauth_tokens SET rotated_to = ? WHERE id = ?")
            .bind(&pair.refresh_token)
            .bind(&id)
            .execute(&self.control)
            .await?;
        Ok(pair)
    }

    #[allow(clippy::too_many_arguments)]
    async fn issue_pair(
        &self,
        client_id: &str,
        tenant_id: &str,
        subject: &str,
        role: &str,
        platform_admin: bool,
        scope: &str,
        audience: &str,
        family_id: Option<&str>,
    ) -> Result<OauthTokenPair, OauthError> {
        let key = self.active_key().await?;
        let now = now_ms();
        let issued_at = now / 1000;
        let expires_at = issued_at + ACCESS_TTL_SECONDS;
        let jti = Uuid::now_v7().to_string();
        let claims = json!({
            "iss": self.issuer(),
            "aud": audience,
            "sub": subject,
            "client_id": client_id,
            "tenant_id": tenant_id,
            "role": role,
            "platform_admin": platform_admin,
            "scope": scope,
            "iat": issued_at,
            "exp": expires_at,
            "jti": jti,
        });
        let access_token = self.sign(&key, &claims)?;
        let family = match family_id {
            Some(family) => family.to_string(),
            None => format!("fam_{}", random_token()?),
        };
        let refresh_token = format!("hpa_rt_{}", random_token()?);
        sqlx::query(
            "INSERT INTO oauth_tokens
             (id, kind, token_hash, jti, family_id, client_id, tenant_id, subject, role,
              platform_admin, scope, audience, created_at, expires_at, revoked_at,
              revoked_reason, rotated_to, used_at)
             VALUES (?, 'access', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL, NULL, NULL)",
        )
        .bind(format!("oat_{}", Uuid::now_v7()))
        .bind(sha256_hex(access_token.as_bytes()))
        .bind(&jti)
        .bind(&family)
        .bind(client_id)
        .bind(tenant_id)
        .bind(subject)
        .bind(role)
        .bind(platform_admin as i64)
        .bind(scope)
        .bind(audience)
        .bind(now)
        .bind(expires_at * 1000)
        .execute(&self.control)
        .await?;
        sqlx::query(
            "INSERT INTO oauth_tokens
             (id, kind, token_hash, jti, family_id, client_id, tenant_id, subject, role,
              platform_admin, scope, audience, created_at, expires_at, revoked_at,
              revoked_reason, rotated_to, used_at)
             VALUES (?, 'refresh', ?, NULL, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL, NULL, NULL)",
        )
        .bind(format!("ort_{}", Uuid::now_v7()))
        .bind(sha256_hex(refresh_token.as_bytes()))
        .bind(&family)
        .bind(client_id)
        .bind(tenant_id)
        .bind(subject)
        .bind(role)
        .bind(platform_admin as i64)
        .bind(scope)
        .bind(audience)
        .bind(now)
        .bind(now + REFRESH_TTL_MS)
        .execute(&self.control)
        .await?;
        sqlx::query("UPDATE oauth_clients SET last_used_at = ?, updated_at = ? WHERE id = ?")
            .bind(now)
            .bind(now)
            .bind(client_id)
            .execute(&self.control)
            .await?;
        Ok(OauthTokenPair {
            access_token,
            refresh_token,
            expires_in: ACCESS_TTL_SECONDS,
            scope: scope.to_string(),
        })
    }

    // ---- revocation / introspection -------------------------------------

    pub async fn revoke(&self, token: &str, client_id: Option<&str>) -> Result<bool, OauthError> {
        let hash = sha256_hex(token.as_bytes());
        let row = sqlx::query(
            "SELECT id, kind, family_id, client_id FROM oauth_tokens WHERE token_hash = ?",
        )
        .bind(&hash)
        .fetch_optional(&self.control)
        .await?;
        let Some(row) = row else {
            return Ok(false);
        };
        let stored_client: String = row.get("client_id");
        if let Some(client_id) = client_id
            && stored_client != client_id
        {
            return Ok(false);
        }
        let family: String = row.get("family_id");
        self.revoke_family(&family, "revoked").await?;
        Ok(true)
    }

    async fn revoke_family(&self, family_id: &str, reason: &str) -> Result<(), OauthError> {
        sqlx::query(
            "UPDATE oauth_tokens SET revoked_at = ?, revoked_reason = ?
             WHERE family_id = ? AND revoked_at IS NULL",
        )
        .bind(now_ms())
        .bind(reason)
        .bind(family_id)
        .execute(&self.control)
        .await?;
        Ok(())
    }

    pub async fn revoke_client_tokens(
        &self,
        client_id: &str,
        reason: &str,
    ) -> Result<usize, OauthError> {
        let revoked = sqlx::query(
            "UPDATE oauth_tokens SET revoked_at = ?, revoked_reason = ?
             WHERE client_id = ? AND revoked_at IS NULL",
        )
        .bind(now_ms())
        .bind(reason)
        .bind(client_id)
        .execute(&self.control)
        .await?;
        Ok(revoked.rows_affected() as usize)
    }

    pub async fn introspect(&self, token: &str) -> Result<Value, OauthError> {
        if token.split('.').count() == 3 {
            match self.verify_access_token(token).await {
                Ok(access) => {
                    return Ok(json!({
                        "active": true,
                        "scope": access.scopes.join(" "),
                        "client_id": access.client_id,
                        "sub": access.subject,
                        "aud": access.audience,
                        "tenant_id": access.tenant_id,
                        "role": access.role,
                        "iat": access.expires_at - ACCESS_TTL_SECONDS,
                        "exp": access.expires_at,
                        "token_type": "Bearer",
                    }));
                }
                Err(_) => return Ok(json!({"active": false})),
            }
        }
        let hash = sha256_hex(token.as_bytes());
        let row = sqlx::query(
            "SELECT client_id, tenant_id, subject, scope, audience, expires_at, revoked_at
             FROM oauth_tokens WHERE token_hash = ?",
        )
        .bind(&hash)
        .fetch_optional(&self.control)
        .await?;
        let Some(row) = row else {
            return Ok(json!({"active": false}));
        };
        let revoked: Option<i64> = row.get("revoked_at");
        let expires_at: i64 = row.get("expires_at");
        if revoked.is_some() || expires_at <= now_ms() {
            return Ok(json!({"active": false}));
        }
        Ok(json!({
            "active": true,
            "scope": row.get::<String, _>("scope"),
            "client_id": row.get::<String, _>("client_id"),
            "sub": row.get::<String, _>("subject"),
            "aud": row.get::<String, _>("audience"),
            "tenant_id": row.get::<String, _>("tenant_id"),
            "exp": expires_at / 1000,
            "token_type": "refresh_token",
        }))
    }

    pub async fn list_tokens(
        &self,
        client_id: Option<&str>,
    ) -> Result<Vec<OauthTokenRow>, OauthError> {
        let rows =
            match client_id {
                Some(client_id) => sqlx::query(
                    "SELECT id, kind, client_id, tenant_id, subject, scope, created_at, expires_at,
                            revoked_at, family_id
                     FROM oauth_tokens WHERE client_id = ? ORDER BY created_at DESC LIMIT 200",
                )
                .bind(client_id)
                .fetch_all(&self.control)
                .await?,
                None => sqlx::query(
                    "SELECT id, kind, client_id, tenant_id, subject, scope, created_at, expires_at,
                            revoked_at, family_id
                     FROM oauth_tokens ORDER BY created_at DESC LIMIT 200",
                )
                .fetch_all(&self.control)
                .await?,
            };
        Ok(rows
            .iter()
            .map(|row| OauthTokenRow {
                id: row.get("id"),
                kind: row.get("kind"),
                client_id: row.get("client_id"),
                tenant_id: row.get("tenant_id"),
                subject: row.get("subject"),
                scope: row.get("scope"),
                created_at: row.get("created_at"),
                expires_at: row.get("expires_at"),
                revoked_at: row.get("revoked_at"),
                family_id: row.get("family_id"),
            })
            .collect())
    }

    pub async fn revoke_token_id(&self, id: &str) -> Result<(), OauthError> {
        let row: Option<String> =
            sqlx::query_scalar("SELECT family_id FROM oauth_tokens WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.control)
                .await?;
        let Some(family) = row else {
            return Err(OauthError::NotFound(format!("token `{id}` was not found")));
        };
        self.revoke_family(&family, "revoked_by_admin").await?;
        Ok(())
    }

    pub async fn purge_expired(&self) -> Result<(), OauthError> {
        let now = now_ms();
        sqlx::query("DELETE FROM oauth_codes WHERE expires_at < ?")
            .bind(now - 3_600_000)
            .execute(&self.control)
            .await?;
        sqlx::query("DELETE FROM oauth_requests WHERE expires_at < ?")
            .bind(now - 3_600_000)
            .execute(&self.control)
            .await?;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct OauthOwner {
    pub tenant_id: String,
    pub subject: String,
    pub role: String,
    pub platform_admin: bool,
    pub authenticated: bool,
}

pub fn split_scopes(raw: &str) -> Vec<String> {
    raw.split_whitespace().map(str::to_string).collect()
}

pub fn normalize_scopes(scopes: &[String]) -> Result<Vec<String>, OauthError> {
    let mut result: Vec<String> = Vec::new();
    for scope in scopes {
        let scope = scope.trim();
        if scope.is_empty() {
            continue;
        }
        if !ALLOWED_SCOPES.contains(&scope) {
            return Err(OauthError::InvalidScope(format!(
                "scope `{scope}` is not supported"
            )));
        }
        if !result.iter().any(|entry| entry == scope) {
            result.push(scope.to_string());
        }
    }
    if result.is_empty() {
        result.push(SCOPE_READ.to_string());
    }
    Ok(result)
}

pub fn validate_redirect_uri(uri: &str) -> Result<(), OauthError> {
    if uri.is_empty() || uri.len() > MAX_REDIRECT_URI {
        return Err(OauthError::InvalidRequest(
            "redirect_uri length is invalid".to_string(),
        ));
    }
    if uri.contains('#') {
        return Err(OauthError::InvalidRequest(
            "redirect_uri must not contain a fragment".to_string(),
        ));
    }
    let Ok(url) = reqwest::Url::parse(uri) else {
        return Err(OauthError::InvalidRequest(
            "redirect_uri is not an absolute URL".to_string(),
        ));
    };
    if url.username() != "" || url.password().is_some() {
        return Err(OauthError::InvalidRequest(
            "redirect_uri must not contain userinfo".to_string(),
        ));
    }
    match url.scheme() {
        "https" => Ok(()),
        "http" => {
            let host = url.host_str().unwrap_or_default();
            if matches!(host, "127.0.0.1" | "localhost" | "::1" | "[::1]") {
                Ok(())
            } else {
                Err(OauthError::InvalidRequest(
                    "http redirect_uri is only allowed on loopback hosts".to_string(),
                ))
            }
        }
        _ => Err(OauthError::InvalidRequest(
            "redirect_uri must use https or loopback http".to_string(),
        )),
    }
}

pub fn validate_pkce_challenge(challenge: &str) -> Result<(), OauthError> {
    if challenge.len() < 43 || challenge.len() > 128 {
        return Err(OauthError::InvalidRequest(
            "code_challenge must be 43-128 characters".to_string(),
        ));
    }
    if !challenge
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~'))
    {
        return Err(OauthError::InvalidRequest(
            "code_challenge contains invalid characters".to_string(),
        ));
    }
    Ok(())
}

fn redirect_with(
    request: &OauthPendingRequest,
    code: Option<&str>,
    error: Option<(&str, &str)>,
) -> String {
    let mut url = match reqwest::Url::parse(&request.redirect_uri) {
        Ok(url) => url,
        Err(_) => return request.redirect_uri.clone(),
    };
    {
        let mut pairs = url.query_pairs_mut();
        if let Some(code) = code {
            pairs.append_pair("code", code);
        }
        if let Some((key, value)) = error {
            pairs.append_pair(key, value);
        }
        if let Some(state) = request.state.as_deref() {
            pairs.append_pair("state", state);
        }
    }
    url.to_string()
}

fn client_from_row(row: &sqlx::sqlite::SqliteRow) -> OauthClient {
    let json_list = |raw: Option<String>| -> Vec<String> {
        raw.and_then(|value| serde_json::from_str::<Vec<String>>(&value).ok())
            .unwrap_or_default()
    };
    OauthClient {
        id: row.get("id"),
        name: row.get("name"),
        redirect_uris: json_list(row.get("redirect_uris")),
        grant_types: json_list(row.get("grant_types")),
        scopes: json_list(row.get("scopes")),
        auth_method: row.get("auth_method"),
        origin: row.get("origin"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        disabled_at: row.get("disabled_at"),
        last_used_at: row.get("last_used_at"),
        has_secret: row
            .get::<Option<String>, _>("secret_hash")
            .is_some_and(|value| !value.is_empty()),
    }
}

fn key_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<StoredKey, OauthError> {
    let alg: String = row.get("alg");
    let alg = match alg.as_str() {
        "ES256" => SigningAlg::Es256,
        "EdDSA" => SigningAlg::EdDsa,
        other => {
            return Err(OauthError::Crypto(format!(
                "stored signing key uses unsupported algorithm `{other}`"
            )));
        }
    };
    let secret: String = row.get("secret");
    let secret = URL_SAFE_NO_PAD
        .decode(secret)
        .map_err(|_| OauthError::Crypto("stored signing key is not base64url".to_string()))?;
    let public_jwk: String = row.get("public_jwk");
    Ok(StoredKey {
        kid: row.get("kid"),
        alg,
        secret,
        public_jwk: serde_json::from_str(&public_jwk)?,
    })
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.bytes().zip(right.bytes()) {
        diff |= a ^ b;
    }
    diff == 0
}

pub fn random_token() -> Result<String, OauthError> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes)
        .map_err(|error| OauthError::Crypto(format!("cannot read randomness: {error}")))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_uris_are_restricted_to_https_or_loopback() {
        assert!(validate_redirect_uri("https://client.example/callback").is_ok());
        assert!(validate_redirect_uri("http://127.0.0.1:6277/callback").is_ok());
        assert!(validate_redirect_uri("http://localhost:3000/cb").is_ok());
        assert!(validate_redirect_uri("http://[::1]:3000/cb").is_ok());
        assert!(validate_redirect_uri("http://client.example/callback").is_err());
        assert!(validate_redirect_uri("https://client.example/cb#fragment").is_err());
        assert!(validate_redirect_uri("https://user:pw@client.example/cb").is_err());
        assert!(validate_redirect_uri("/callback").is_err());
        assert!(validate_redirect_uri("").is_err());
    }

    #[test]
    fn scopes_are_normalized_and_unknown_values_rejected() {
        assert_eq!(
            normalize_scopes(&["mcp:read".to_string()]).unwrap(),
            vec!["mcp:read".to_string()]
        );
        assert_eq!(
            normalize_scopes(&[
                "mcp:execute".to_string(),
                "mcp:execute".to_string(),
                "offline_access".to_string()
            ])
            .unwrap(),
            vec!["mcp:execute".to_string(), "offline_access".to_string()]
        );
        assert_eq!(normalize_scopes(&[]).unwrap(), vec![SCOPE_READ.to_string()]);
        assert!(normalize_scopes(&["admin:all".to_string()]).is_err());
    }

    #[test]
    fn pkce_challenge_lengths_are_enforced() {
        assert!(validate_pkce_challenge(&"a".repeat(43)).is_ok());
        assert!(validate_pkce_challenge(&"a".repeat(128)).is_ok());
        assert!(validate_pkce_challenge(&"a".repeat(42)).is_err());
        assert!(validate_pkce_challenge(&"a".repeat(129)).is_err());
        assert!(validate_pkce_challenge("not base64 url!").is_err());
    }

    #[test]
    fn scope_splitting_handles_whitespace() {
        assert_eq!(
            split_scopes("mcp:read  mcp:execute "),
            vec!["mcp:read".to_string(), "mcp:execute".to_string()]
        );
        assert!(split_scopes("   ").is_empty());
    }
}
