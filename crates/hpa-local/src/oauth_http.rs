//! HTTP surface of the built-in OAuth 2.1 authorization server.

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{delete, get, post};
use axum::{Form, Json, Router};
use base64::Engine;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api::ApiError;
use crate::enterprise::EnterpriseState;
use crate::oauth::{
    ALLOWED_SCOPES, OauthClient, OauthError, OauthTokenRow, normalize_scopes, split_scopes,
    validate_pkce_challenge, validate_redirect_uri,
};

pub fn router() -> Router<EnterpriseState> {
    Router::new()
        .route(
            "/.well-known/oauth-authorization-server",
            get(authorization_server_metadata),
        )
        .route("/oauth/jwks", get(jwks))
        .route("/oauth/authorize", get(authorize))
        .route("/oauth/register", post(register))
        .route("/oauth/token", post(token))
        .route("/oauth/revoke", post(revoke))
        .route("/oauth/introspect", post(introspect))
        .route("/api/oauth/requests/{id}", get(read_request))
        .route("/api/oauth/requests/{id}/decision", post(decide_request))
        .route("/api/enterprise/oauth/overview", get(oauth_overview))
        .route(
            "/api/enterprise/oauth/clients",
            get(list_clients).post(create_client),
        )
        .route("/api/enterprise/oauth/clients/{id}", delete(disable_client))
        .route(
            "/api/enterprise/oauth/clients/{id}/secret",
            post(rotate_secret),
        )
        .route("/api/enterprise/oauth/tokens", get(list_tokens))
        .route("/api/enterprise/oauth/tokens/{id}", delete(revoke_token))
        .route(
            "/api/enterprise/oauth/keys/rotate",
            post(rotate_signing_key),
        )
}

// ---- discovery ---------------------------------------------------------

async fn authorization_server_metadata(State(state): State<EnterpriseState>) -> Response {
    match state.oauth().metadata().await {
        Ok(document) => Json(document).into_response(),
        Err(error) => protocol_error(error),
    }
}

async fn jwks(State(state): State<EnterpriseState>) -> Response {
    match state.oauth().jwks().await {
        Ok(document) => {
            let mut response = Json(document).into_response();
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                header::HeaderValue::from_static("public, max-age=300"),
            );
            response
        }
        Err(error) => protocol_error(error),
    }
}

// ---- authorization endpoint -------------------------------------------

#[derive(Debug, Deserialize)]
struct AuthorizeQuery {
    #[serde(default)]
    response_type: Option<String>,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    redirect_uri: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    code_challenge: Option<String>,
    #[serde(default)]
    code_challenge_method: Option<String>,
    #[serde(default)]
    resource: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    request: Option<String>,
    #[serde(default)]
    request_uri: Option<String>,
    #[serde(default)]
    response_mode: Option<String>,
}

async fn authorize(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
    uri: Uri,
    Query(query): Query<AuthorizeQuery>,
) -> Response {
    if headers.contains_key("dpop") {
        return error_page(
            "invalid_request",
            "DPoP is not supported by the built-in authorization server",
        );
    }
    if query.request.is_some() || query.request_uri.is_some() {
        return error_page(
            "invalid_request",
            "JAR / request_uri is not supported by the built-in authorization server",
        );
    }
    let Some(client_id) = query.client_id.as_deref().filter(|value| !value.is_empty()) else {
        return error_page(
            "invalid_request",
            "client_id が必要です / client_id is required",
        );
    };
    let client = match state.oauth().client(client_id).await {
        Ok(Some(client)) => client,
        Ok(None) => return error_page("invalid_client", "クライアントが登録されていません"),
        Err(error) => return protocol_error(error),
    };
    if client.disabled_at.is_some() {
        return error_page("unauthorized_client", "クライアントは無効化されています");
    }
    let Some(redirect_uri) = query
        .redirect_uri
        .as_deref()
        .filter(|value| !value.is_empty())
    else {
        return error_page("invalid_request", "redirect_uri が必要です");
    };
    if !client
        .redirect_uris
        .iter()
        .any(|entry| entry == redirect_uri)
    {
        // Never redirect to an unregistered URI.
        return error_page(
            "invalid_request",
            "redirect_uri がクライアント登録と一致しません",
        );
    }
    let state_param = query.state.clone();
    let denial = |code: &str, description: &str| {
        redirect_error(redirect_uri, state_param.as_deref(), code, description)
    };
    if query
        .response_mode
        .as_deref()
        .is_some_and(|mode| mode != "query")
    {
        return denial("invalid_request", "only response_mode=query is supported");
    }
    if query.response_type.as_deref() != Some("code") {
        return denial(
            "unsupported_response_type",
            "response_type は code のみ対応です",
        );
    }
    let Some(challenge) = query
        .code_challenge
        .as_deref()
        .filter(|value| !value.is_empty())
    else {
        return denial("invalid_request", "PKCE の code_challenge は必須です");
    };
    if query.code_challenge_method.as_deref() != Some("S256") {
        return denial(
            "invalid_request",
            "code_challenge_method は S256 のみ対応です",
        );
    }
    if let Err(error) = validate_pkce_challenge(challenge) {
        return denial("invalid_request", &error.description());
    }
    let requested = match scopes_from_request(query.scope.as_deref()) {
        Ok(scopes) => scopes,
        Err(error) => return denial(error.code(), &error.description()),
    };
    if let Some(reason) = scope_not_allowed(&client, &requested) {
        return denial("invalid_scope", &reason);
    }
    if let Some(resource) = query.resource.as_deref()
        && !resource_allowed(&state, resource)
    {
        return denial("invalid_target", "resource がこのサーバーの対象外です");
    }
    let scope = requested.join(" ");

    let owner = match state.oauth_owner(&headers).await {
        Ok(Some(owner)) => owner,
        Ok(None) => {
            let original = uri
                .path_and_query()
                .map(|value| value.as_str().to_string())
                .unwrap_or_else(|| uri.path().to_string());
            return login_redirect(&state, &original);
        }
        Err(error) => return error.into_response(),
    };

    let request = match state
        .oauth()
        .create_request(
            &client,
            redirect_uri,
            &scope,
            query.state.as_deref(),
            query.resource.as_deref(),
            challenge,
            &owner,
        )
        .await
    {
        Ok(request) => request,
        Err(error) => return denial(error.code(), &error.description()),
    };

    let force_consent = query.prompt.as_deref() == Some("consent");
    if !force_consent {
        match state
            .oauth()
            .has_consent(&client.id, &owner.tenant_id, &owner.subject, &scope)
            .await
        {
            Ok(true) => match state.oauth().decide(&request.id, &owner, true, false).await {
                Ok(location) => {
                    state
                        .record_audit(
                            Some(&owner.tenant_id),
                            Some(&owner.subject),
                            "oauth",
                            "oauth.authorize",
                            "oauth_client",
                            "allow",
                            None,
                            json!({
                                "clientId": client.id,
                                "scope": scope,
                                "reusedConsent": true,
                            }),
                        )
                        .await;
                    return Redirect::to(&location).into_response();
                }
                Err(error) => return denial(error.code(), &error.description()),
            },
            Ok(false) => {}
            Err(error) => return protocol_error(error),
        }
    }

    Redirect::to(&format!("/consent?request={}", request.id)).into_response()
}

async fn read_request(
    State(state): State<EnterpriseState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let owner = require_owner(&state, &headers).await?;
    let request = state
        .oauth()
        .pending_request(&id)
        .await
        .map_err(oauth_api_error)?;
    if request.tenant_id != owner.tenant_id || request.subject != owner.subject {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "OAUTH_REQUEST_FORBIDDEN",
            "this authorization request belongs to another session",
        ));
    }
    Ok(Json(json!({
        "id": request.id,
        "clientId": request.client_id,
        "clientName": request.client_name,
        "redirectUri": request.redirect_uri,
        "redirectHost": host_of(&request.redirect_uri),
        "scope": request.scope,
        "scopes": split_scopes(&request.scope),
        "resource": request.resource,
        "tenantId": request.tenant_id,
        "subject": request.subject,
        "expiresAt": request.expires_at,
        "authenticated": owner.authenticated,
    })))
}

#[derive(Debug, Deserialize)]
struct DecisionRequest {
    #[serde(default)]
    decision: Option<String>,
    #[serde(default)]
    remember: Option<bool>,
}

async fn decide_request(
    State(state): State<EnterpriseState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
    Json(body): Json<DecisionRequest>,
) -> Result<Json<Value>, ApiError> {
    let owner = require_owner(&state, &headers).await?;
    let allow = match body.decision.as_deref() {
        Some("allow") => true,
        Some("deny") => false,
        _ => {
            return Err(ApiError::bad_request(
                "OAUTH_DECISION_INVALID",
                "decision must be `allow` or `deny`",
            ));
        }
    };
    let location = state
        .oauth()
        .decide(&id, &owner, allow, body.remember.unwrap_or(true))
        .await
        .map_err(oauth_api_error)?;
    state
        .record_audit(
            Some(&owner.tenant_id),
            Some(&owner.subject),
            "oauth",
            "oauth.consent",
            "oauth_client",
            if allow { "allow" } else { "deny" },
            None,
            json!({
                "requestId": id,
                "remember": body.remember.unwrap_or(true),
            }),
        )
        .await;
    Ok(Json(json!({"redirect": location})))
}

// ---- dynamic client registration --------------------------------------

#[derive(Debug, Deserialize)]
struct RegisterRequest {
    #[serde(default)]
    client_name: Option<String>,
    #[serde(default)]
    redirect_uris: Option<Vec<String>>,
    #[serde(default)]
    token_endpoint_auth_method: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    grant_types: Option<Vec<String>>,
}

async fn register(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
    Json(body): Json<RegisterRequest>,
) -> Response {
    if headers.contains_key("dpop") {
        return protocol_error(OauthError::InvalidRequest(
            "DPoP is not supported by the built-in authorization server".to_string(),
        ));
    }
    let redirect_uris = body.redirect_uris.unwrap_or_default();
    if let Some(grant_types) = body.grant_types.as_deref() {
        let unsupported = grant_types
            .iter()
            .any(|grant| !matches!(grant.as_str(), "authorization_code" | "refresh_token"));
        if unsupported {
            return protocol_error(OauthError::InvalidRequest(
                "grant_types must be authorization_code and refresh_token".to_string(),
            ));
        }
    }
    let scopes = match scopes_from_request(body.scope.as_deref()) {
        Ok(scopes) => scopes,
        Err(error) => return protocol_error(error),
    };
    let name = body
        .client_name
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "Unnamed MCP client".to_string());
    let auth_method = body
        .token_endpoint_auth_method
        .unwrap_or_else(|| "none".to_string());
    match state
        .oauth()
        .register_client(&name, &redirect_uris, &auth_method, &scopes, "dynamic")
        .await
    {
        Ok((client, secret)) => {
            state
                .record_audit(
                    None,
                    Some(&client.id),
                    "oauth",
                    "oauth.client.register",
                    "oauth_client",
                    "allow",
                    None,
                    json!({"name": client.name, "redirectUris": client.redirect_uris}),
                )
                .await;
            let mut body = json!({
                "client_id": client.id,
                "client_id_issued_at": client.created_at / 1000,
                "client_name": client.name,
                "redirect_uris": client.redirect_uris,
                "grant_types": client.grant_types,
                "scope": client.scopes.join(" "),
                "token_endpoint_auth_method": client.auth_method,
            });
            if let Some(secret) = secret {
                body["client_secret"] = Value::String(secret);
                body["client_secret_expires_at"] = Value::from(0);
            }
            (StatusCode::CREATED, Json(body)).into_response()
        }
        Err(error) => protocol_error(error),
    }
}

// ---- token / revoke / introspect --------------------------------------

#[derive(Debug, Deserialize)]
struct TokenRequest {
    #[serde(default)]
    grant_type: Option<String>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    redirect_uri: Option<String>,
    #[serde(default)]
    code_verifier: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    client_secret: Option<String>,
}

async fn token(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
    Form(form): Form<TokenRequest>,
) -> Response {
    if headers.contains_key("dpop") {
        return protocol_error(OauthError::InvalidRequest(
            "DPoP is not supported by the built-in authorization server".to_string(),
        ));
    }
    let (basic_id, basic_secret) = basic_credentials(&headers);
    let client_id = form.client_id.clone().or(basic_id).unwrap_or_default();
    if client_id.trim().is_empty() {
        return protocol_error(OauthError::InvalidClient(
            "client_id is required".to_string(),
        ));
    }
    let client_secret = form.client_secret.clone().or(basic_secret);
    let outcome = match form.grant_type.as_deref() {
        Some("authorization_code") => {
            match (
                form.code.as_deref(),
                form.redirect_uri.as_deref(),
                form.code_verifier.as_deref(),
            ) {
                (Some(code), Some(redirect_uri), Some(code_verifier)) => {
                    state
                        .oauth()
                        .exchange_code(
                            code,
                            &client_id,
                            client_secret.as_deref(),
                            redirect_uri,
                            code_verifier,
                        )
                        .await
                }
                _ => Err(OauthError::InvalidRequest(
                    "code, redirect_uri and code_verifier are required".to_string(),
                )),
            }
        }
        Some("refresh_token") => match form.refresh_token.as_deref() {
            Some(refresh_token) => {
                state
                    .oauth()
                    .exchange_refresh(
                        refresh_token,
                        &client_id,
                        client_secret.as_deref(),
                        form.scope.as_deref(),
                    )
                    .await
            }
            None => Err(OauthError::InvalidRequest(
                "refresh_token is required".to_string(),
            )),
        },
        Some(other) => Err(OauthError::UnsupportedGrantType(format!(
            "grant_type `{other}` is not supported"
        ))),
        None => Err(OauthError::InvalidRequest(
            "grant_type is required".to_string(),
        )),
    };
    match outcome {
        Ok(pair) => {
            state
                .record_audit(
                    None,
                    Some(&client_id),
                    "oauth",
                    "oauth.token",
                    "oauth_client",
                    "allow",
                    None,
                    json!({
                        "grantType": form.grant_type,
                        "scope": pair.scope,
                    }),
                )
                .await;
            let mut response = Json(json!({
                "access_token": pair.access_token,
                "token_type": "Bearer",
                "expires_in": pair.expires_in,
                "refresh_token": pair.refresh_token,
                "scope": pair.scope,
            }))
            .into_response();
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                header::HeaderValue::from_static("no-store"),
            );
            response
                .headers_mut()
                .insert(header::PRAGMA, header::HeaderValue::from_static("no-cache"));
            response
        }
        Err(error) => {
            state
                .record_audit(
                    None,
                    Some(&client_id),
                    "oauth",
                    "oauth.token",
                    "oauth_client",
                    "deny",
                    Some(&error.description()),
                    json!({"grantType": form.grant_type, "error": error.code()}),
                )
                .await;
            protocol_error(error)
        }
    }
}

#[derive(Debug, Deserialize)]
struct RevokeRequest {
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    client_secret: Option<String>,
}

async fn revoke(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
    Form(form): Form<RevokeRequest>,
) -> Response {
    if headers.contains_key("dpop") {
        return protocol_error(OauthError::InvalidRequest(
            "DPoP is not supported by the built-in authorization server".to_string(),
        ));
    }
    let Some(token) = form.token.as_deref().filter(|value| !value.is_empty()) else {
        return protocol_error(OauthError::InvalidRequest("token is required".to_string()));
    };
    let (basic_id, _) = basic_credentials(&headers);
    let client_id = form.client_id.clone().or(basic_id);
    if let Some(client_id) = client_id.as_deref() {
        let Some(client) = state.oauth().client(client_id).await.ok().flatten() else {
            return protocol_error(OauthError::InvalidClient("client is unknown".to_string()));
        };
        if client.auth_method != "none"
            && !state
                .oauth()
                .verify_client_secret(client_id, form.client_secret.as_deref())
                .await
                .unwrap_or(false)
        {
            return protocol_error(OauthError::InvalidClient(
                "client authentication failed".to_string(),
            ));
        }
    }
    match state.oauth().revoke(token, client_id.as_deref()).await {
        Ok(_) => StatusCode::OK.into_response(),
        Err(error) => protocol_error(error),
    }
}

#[derive(Debug, Deserialize)]
struct IntrospectRequest {
    #[serde(default)]
    token: Option<String>,
}

async fn introspect(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
    Form(form): Form<IntrospectRequest>,
) -> Response {
    if headers.contains_key("dpop") {
        return protocol_error(OauthError::InvalidRequest(
            "DPoP is not supported by the built-in authorization server".to_string(),
        ));
    }
    let Some(token) = form.token.as_deref().filter(|value| !value.is_empty()) else {
        return protocol_error(OauthError::InvalidRequest("token is required".to_string()));
    };
    match state.oauth().introspect(token).await {
        Ok(document) => Json(document).into_response(),
        Err(error) => protocol_error(error),
    }
}

// ---- administration ----------------------------------------------------

async fn oauth_overview(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    state.oauth_admin(&headers).await?;
    let clients = state
        .oauth()
        .list_clients()
        .await
        .map_err(oauth_api_error)?;
    let tokens = state
        .oauth()
        .list_tokens(None)
        .await
        .map_err(oauth_api_error)?;
    let active = tokens
        .iter()
        .filter(|token| token.revoked_at.is_none() && token.expires_at > crate::oauth::now_ms())
        .count();
    Ok(Json(json!({
        "issuer": state.oauth().issuer(),
        "resource": state.oauth().resource(),
        "jwksUri": format!("{}/oauth/jwks", state.oauth().issuer()),
        "authorizationEndpoint": format!("{}/oauth/authorize", state.oauth().issuer()),
        "tokenEndpoint": format!("{}/oauth/token", state.oauth().issuer()),
        "registrationEndpoint": format!("{}/oauth/register", state.oauth().issuer()),
        "scopes": ALLOWED_SCOPES,
        "clients": clients.iter().map(client_value).collect::<Vec<_>>(),
        "clientCount": clients.len(),
        "tokenCount": tokens.len(),
        "activeTokenCount": active,
        "signingAlg": state.oauth().signing_alg_name(),
    })))
}

#[derive(Debug, Deserialize)]
struct CreateClientRequest {
    name: String,
    #[serde(default)]
    redirect_uris: Vec<String>,
    #[serde(default)]
    token_endpoint_auth_method: Option<String>,
    #[serde(default)]
    scope: Option<String>,
}

async fn create_client(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
    Json(body): Json<CreateClientRequest>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    state.oauth_admin(&headers).await?;
    let scopes = scopes_from_request(body.scope.as_deref()).map_err(oauth_api_error)?;
    let auth_method = body
        .token_endpoint_auth_method
        .unwrap_or_else(|| "none".to_string());
    for uri in &body.redirect_uris {
        validate_redirect_uri(uri).map_err(oauth_api_error)?;
    }
    let (client, secret) = state
        .oauth()
        .register_client(
            &body.name,
            &body.redirect_uris,
            &auth_method,
            &scopes,
            "manual",
        )
        .await
        .map_err(oauth_api_error)?;
    state
        .record_audit(
            None,
            Some(&client.id),
            "oauth",
            "oauth.client.create",
            "oauth_client",
            "allow",
            None,
            json!({"name": client.name, "origin": "manual"}),
        )
        .await;
    let mut value = client_value(&client);
    if let Some(secret) = secret {
        value["clientSecret"] = Value::String(secret);
    }
    Ok((StatusCode::CREATED, Json(value)))
}

async fn list_clients(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    state.oauth_admin(&headers).await?;
    let clients = state
        .oauth()
        .list_clients()
        .await
        .map_err(oauth_api_error)?;
    Ok(Json(json!({
        "clients": clients.iter().map(client_value).collect::<Vec<_>>(),
    })))
}

async fn disable_client(
    State(state): State<EnterpriseState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    state.oauth_admin(&headers).await?;
    state
        .oauth()
        .disable_client(&id)
        .await
        .map_err(oauth_api_error)?;
    state
        .record_audit(
            None,
            Some(&id),
            "oauth",
            "oauth.client.disable",
            "oauth_client",
            "allow",
            None,
            json!({}),
        )
        .await;
    Ok(Json(json!({"disabled": id})))
}

async fn rotate_secret(
    State(state): State<EnterpriseState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    state.oauth_admin(&headers).await?;
    let secret = state
        .oauth()
        .rotate_client_secret(&id)
        .await
        .map_err(oauth_api_error)?;
    state
        .record_audit(
            None,
            Some(&id),
            "oauth",
            "oauth.client.secret",
            "oauth_client",
            "allow",
            None,
            json!({}),
        )
        .await;
    Ok(Json(json!({"clientId": id, "clientSecret": secret})))
}

async fn list_tokens(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    state.oauth_admin(&headers).await?;
    let tokens = state
        .oauth()
        .list_tokens(None)
        .await
        .map_err(oauth_api_error)?;
    Ok(Json(json!({
        "tokens": tokens.iter().map(token_value).collect::<Vec<_>>(),
    })))
}

async fn revoke_token(
    State(state): State<EnterpriseState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    state.oauth_admin(&headers).await?;
    state
        .oauth()
        .revoke_token_id(&id)
        .await
        .map_err(oauth_api_error)?;
    state
        .record_audit(
            None,
            Some(&id),
            "oauth",
            "oauth.token.revoke",
            "oauth_token",
            "allow",
            None,
            json!({}),
        )
        .await;
    Ok(Json(json!({"revoked": id})))
}

async fn rotate_signing_key(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    state.oauth_admin(&headers).await?;
    let key = state
        .oauth()
        .rotate_signing_key()
        .await
        .map_err(oauth_api_error)?;
    state
        .record_audit(
            None,
            None,
            "oauth",
            "oauth.key.rotate",
            "oauth_signing_key",
            "allow",
            None,
            key.clone(),
        )
        .await;
    Ok(Json(key))
}

// ---- helpers -----------------------------------------------------------

async fn require_owner(
    state: &EnterpriseState,
    headers: &HeaderMap,
) -> Result<crate::oauth::OauthOwner, ApiError> {
    match state.oauth_owner(headers).await? {
        Some(owner) => Ok(owner),
        None => Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "SESSION_REQUIRED",
            "sign in before approving an authorization request",
        )),
    }
}

fn scopes_from_request(raw: Option<&str>) -> Result<Vec<String>, OauthError> {
    match raw.map(str::trim).filter(|value| !value.is_empty()) {
        None => Ok(vec![crate::oauth::SCOPE_READ.to_string()]),
        Some(raw) => normalize_scopes(&split_scopes(raw)),
    }
}

fn scope_not_allowed(client: &OauthClient, scopes: &[String]) -> Option<String> {
    scopes
        .iter()
        .find(|scope| !client.scopes.iter().any(|entry| entry == *scope))
        .map(|scope| format!("scope `{scope}` is not registered for this client"))
}

fn resource_allowed(state: &EnterpriseState, resource: &str) -> bool {
    let expected = state.oauth().resource();
    resource == expected || resource == state.oauth().issuer()
}

fn client_value(client: &OauthClient) -> Value {
    json!({
        "id": client.id,
        "name": client.name,
        "redirectUris": client.redirect_uris,
        "grantTypes": client.grant_types,
        "scopes": client.scopes,
        "tokenEndpointAuthMethod": client.auth_method,
        "hasSecret": client.has_secret,
        "origin": client.origin,
        "createdAt": client.created_at,
        "updatedAt": client.updated_at,
        "disabledAt": client.disabled_at,
        "lastUsedAt": client.last_used_at,
    })
}

fn token_value(token: &OauthTokenRow) -> Value {
    json!({
        "id": token.id,
        "kind": token.kind,
        "clientId": token.client_id,
        "tenantId": token.tenant_id,
        "subject": token.subject,
        "scope": token.scope,
        "createdAt": token.created_at,
        "expiresAt": token.expires_at,
        "revokedAt": token.revoked_at,
        "familyId": token.family_id,
    })
}

fn host_of(uri: &str) -> String {
    reqwest::Url::parse(uri)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_default()
}

fn login_redirect(state: &EnterpriseState, original: &str) -> Response {
    let Ok(mut url) = reqwest::Url::parse(&format!("{}/login", state.oauth().issuer())) else {
        return error_page("login_required", "ログインが必要です");
    };
    url.query_pairs_mut().append_pair("redirect", original);
    let query = url.query().unwrap_or_default().to_string();
    Redirect::to(&format!("/login?{query}")).into_response()
}

fn redirect_error(
    redirect_uri: &str,
    state: Option<&str>,
    code: &str,
    description: &str,
) -> Response {
    let Ok(mut url) = reqwest::Url::parse(redirect_uri) else {
        return error_page(code, description);
    };
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("error", code);
        pairs.append_pair("error_description", description);
        if let Some(state) = state {
            pairs.append_pair("state", state);
        }
    }
    Redirect::to(url.as_str()).into_response()
}

fn protocol_error(error: OauthError) -> Response {
    let status = match &error {
        OauthError::InvalidClient(_) | OauthError::UnauthorizedClient(_) => {
            StatusCode::UNAUTHORIZED
        }
        _ if error.is_server_error() => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::BAD_REQUEST,
    };
    let mut body = json!({
        "error": error.code(),
        "error_description": error.description(),
    });
    if status == StatusCode::UNAUTHORIZED {
        body["error"] = Value::String("invalid_client".to_string());
    }
    let mut response = (status, Json(body)).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}

fn oauth_api_error(error: OauthError) -> ApiError {
    let code = error.code().to_ascii_uppercase().replace('-', "_");
    match &error {
        OauthError::NotFound(message) => ApiError::not_found(message.clone()),
        OauthError::Conflict(message) => {
            ApiError::conflict(&format!("OAUTH_{code}"), message.clone())
        }
        _ if error.is_server_error() => ApiError::internal(error.description()),
        _ => ApiError::bad_request(&format!("OAUTH_{code}"), error.description()),
    }
}

fn error_page(code: &str, description: &str) -> Response {
    let body = format!(
        "<!doctype html><html lang=\"ja\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>hpa · 認可エラー</title>\
         <style>body{{font-family:ui-sans-serif,system-ui,-apple-system,\"Hiragino Sans\",sans-serif;\
         margin:0;min-height:100vh;display:grid;place-items:center;background:#f6f7f9;color:#111}}\
         main{{max-width:520px;padding:28px 24px;background:#fff;border:1px solid #e5e7eb;\
         border-radius:10px}}h1{{font-size:16px;margin:0 0 8px}}p{{font-size:13px;line-height:1.6;\
         margin:0;color:#4b5563}}code{{font-size:12px;color:#b45309;background:#fffbeb;\
         border:1px solid #fde68a;border-radius:4px;padding:1px 5px}}</style></head>\
         <body><main><h1>認可リクエストを処理できません</h1>\
         <p><code>{code}</code></p><p>{description}</p></main></body></html>"
    );
    (StatusCode::BAD_REQUEST, Html(body)).into_response()
}

fn basic_credentials(headers: &HeaderMap) -> (Option<String>, Option<String>) {
    let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
    else {
        return (None, None);
    };
    let Some(encoded) = value.strip_prefix("Basic ") else {
        return (None, None);
    };
    let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(encoded.trim()) else {
        return (None, None);
    };
    let Ok(text) = String::from_utf8(decoded) else {
        return (None, None);
    };
    match text.split_once(':') {
        Some((id, secret)) => (Some(percent_decode(id)), Some(percent_decode(secret))),
        None => (Some(percent_decode(&text)), None),
    }
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
        {
            out.push(high * 16 + low);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
