use std::collections::BTreeSet;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use sqlx::{AssertSqlSafe, Row, Sqlite, SqlitePool, Transaction};
use uuid::Uuid;

use hpa_local::postgres_control::{ControlError, ScimGroupRecord, ScimMemberRecord};

use super::{EnterpriseState, Principal};

const USER_SCHEMA: &str = "urn:ietf:params:scim:schemas:core:2.0:User";
const GROUP_SCHEMA: &str = "urn:ietf:params:scim:schemas:core:2.0:Group";
const LIST_SCHEMA: &str = "urn:ietf:params:scim:api:messages:2.0:ListResponse";
const PATCH_SCHEMA: &str = "urn:ietf:params:scim:api:messages:2.0:PatchOp";
const ERROR_SCHEMA: &str = "urn:ietf:params:scim:api:messages:2.0:Error";
const ROLE_SCHEMA: &str = "urn:hpa:params:scim:schemas:extension:role:1.0";
const MAX_PAGE_SIZE: i64 = 200;

pub(super) fn router() -> Router<EnterpriseState> {
    Router::new()
        .route(
            "/scim/v2/ServiceProviderConfig",
            get(service_provider_config),
        )
        .route("/scim/v2/ResourceTypes", get(resource_types))
        .route("/scim/v2/Schemas", get(schemas))
        .route("/scim/v2/Users", get(list_users).post(create_user))
        .route(
            "/scim/v2/Users/{id}",
            get(get_user)
                .put(replace_user)
                .patch(patch_user)
                .delete(delete_user),
        )
        .route("/scim/v2/Groups", get(list_groups).post(create_group))
        .route(
            "/scim/v2/Groups/{id}",
            get(get_group)
                .put(replace_group)
                .patch(patch_group)
                .delete(delete_group),
        )
}

#[derive(Debug)]
struct ScimError {
    status: StatusCode,
    scim_type: Option<&'static str>,
    detail: String,
}

impl ScimError {
    fn new(status: StatusCode, scim_type: Option<&'static str>, detail: impl Into<String>) -> Self {
        Self {
            status,
            scim_type,
            detail: detail.into(),
        }
    }

    fn invalid(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, Some("invalidValue"), detail)
    }

    fn invalid_syntax(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, Some("invalidSyntax"), detail)
    }

    fn invalid_filter(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, Some("invalidFilter"), detail)
    }

    fn unauthorized(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, None, detail)
    }

    fn forbidden(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, None, detail)
    }

    fn not_found(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, None, detail)
    }

    fn conflict(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, Some("uniqueness"), detail)
    }

    fn precondition(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::PRECONDITION_FAILED, None, detail)
    }

    fn database(error: sqlx::Error) -> Self {
        if is_unique_violation(&error) {
            return Self::conflict("SCIM resource conflicts with an existing value");
        }
        tracing::error!(error = %error, "SCIM database operation failed");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            None,
            "SCIM storage operation failed",
        )
    }

    fn from_control(error: ControlError) -> Self {
        match error {
            ControlError::Db(error) => Self::database(error),
            ControlError::Invalid(message)
                if message == "member belongs to SCIM Groups mapped to different roles" =>
            {
                Self::new(StatusCode::CONFLICT, Some("invalidValue"), message)
            }
            ControlError::Invalid(message) => Self::invalid(message),
            ControlError::NotFound(message) => Self::not_found(message),
            ControlError::Json(error) => {
                tracing::error!(error = %error, "SCIM JSON operation failed");
                Self::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    None,
                    "SCIM storage operation failed",
                )
            }
        }
    }

    fn from_api(error: crate::api::ApiError) -> Self {
        Self::new(error.status, None, error.message)
    }
}

impl IntoResponse for ScimError {
    fn into_response(self) -> Response {
        let mut body = json!({
            "schemas": [ERROR_SCHEMA],
            "detail": self.detail,
            "status": self.status.as_u16().to_string(),
        });
        if let Some(scim_type) = self.scim_type {
            body["scimType"] = Value::String(scim_type.to_string());
        }
        let mut response = scim_response(self.status, body, None);
        if self.status == StatusCode::UNAUTHORIZED {
            response.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                HeaderValue::from_static("Bearer realm=\"scim\""),
            );
        }
        response
    }
}

fn is_unique_violation(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .is_some_and(|database| database.is_unique_violation())
}

fn scim_response(status: StatusCode, body: Value, etag: Option<&str>) -> Response {
    let mut response = (status, Json(body)).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/scim+json"),
    );
    if let Some(etag) = etag.and_then(|value| HeaderValue::from_str(value).ok()) {
        response.headers_mut().insert(header::ETAG, etag);
    }
    response
}

fn scim_json_body(payload: Result<Json<Value>, JsonRejection>) -> Result<Value, ScimError> {
    payload
        .map(|Json(value)| value)
        .map_err(|error| ScimError::invalid_syntax(format!("invalid SCIM JSON: {error}")))
}

fn no_content() -> Response {
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/scim+json"),
    );
    response
}

async fn authorize(state: &EnterpriseState, headers: &HeaderMap) -> Result<Principal, ScimError> {
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| ScimError::unauthorized("SCIM requires an Authorization Bearer token"))?;
    let token = authorization
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|value| value.starts_with("hpa_ent_"))
        .ok_or_else(|| {
            ScimError::forbidden("SCIM provisioning requires a tenant-scoped hpa_ent_ bearer token")
        })?;
    if token.is_empty() {
        return Err(ScimError::unauthorized("SCIM bearer token is empty"));
    }
    let principal = state
        .authenticate(headers)
        .await
        .map_err(ScimError::from_api)?
        .ok_or_else(|| ScimError::unauthorized("SCIM requires a tenant bearer token"))?;
    if !principal.platform_admin && !matches!(principal.role.as_str(), "owner" | "admin") {
        return Err(ScimError::forbidden(
            "SCIM provisioning requires the owner or admin tenant role",
        ));
    }
    Ok(principal)
}

async fn service_provider_config(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
) -> Result<Response, ScimError> {
    authorize(&state, &headers).await?;
    Ok(scim_response(
        StatusCode::OK,
        json!({
            "schemas": ["urn:ietf:params:scim:schemas:core:2.0:ServiceProviderConfig"],
            "documentationUri": "/docs/08-platform/scim.md",
            "patch": {"supported": true},
            "bulk": {"supported": false, "maxOperations": 0, "maxPayloadSize": 0},
            "filter": {"supported": true, "maxResults": MAX_PAGE_SIZE},
            "changePassword": {"supported": false},
            "sort": {"supported": false},
            "etag": {"supported": true},
            "authenticationSchemes": [{
                "type": "oauthbearertoken",
                "name": "Tenant bearer token",
                "description": "Tenant-scoped owner/admin token or platform-admin token",
                "primary": true
            }],
            "meta": {
                "resourceType": "ServiceProviderConfig",
                "location": "/scim/v2/ServiceProviderConfig"
            }
        }),
        None,
    ))
}

async fn resource_types(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
) -> Result<Response, ScimError> {
    authorize(&state, &headers).await?;
    Ok(scim_response(
        StatusCode::OK,
        json!({
            "schemas": [LIST_SCHEMA],
            "totalResults": 2,
            "itemsPerPage": 2,
            "startIndex": 1,
            "Resources": [
                {
                    "id": "User",
                    "name": "User",
                    "endpoint": "/Users",
                    "schema": USER_SCHEMA,
                    "schemaExtensions": [{"schema": ROLE_SCHEMA, "required": false}],
                    "meta": {"resourceType": "ResourceType", "location": "/scim/v2/ResourceTypes/User"}
                },
                {
                    "id": "Group",
                    "name": "Group",
                    "endpoint": "/Groups",
                    "schema": GROUP_SCHEMA,
                    "schemaExtensions": [{"schema": ROLE_SCHEMA, "required": false}],
                    "meta": {"resourceType": "ResourceType", "location": "/scim/v2/ResourceTypes/Group"}
                }
            ]
        }),
        None,
    ))
}

async fn schemas(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
) -> Result<Response, ScimError> {
    authorize(&state, &headers).await?;
    Ok(scim_response(
        StatusCode::OK,
        json!({
            "schemas": [LIST_SCHEMA],
            "totalResults": 3,
            "itemsPerPage": 3,
            "startIndex": 1,
            "Resources": [
                {
                    "id": USER_SCHEMA,
                    "name": "User",
                    "description": "SCIM core User",
                    "attributes": [
                        {"name": "userName", "type": "string", "multiValued": false, "required": true, "caseExact": false, "mutability": "readWrite", "returned": "default", "uniqueness": "server"},
                        {"name": "externalId", "type": "string", "multiValued": false, "required": false, "caseExact": true, "mutability": "readWrite", "returned": "default", "uniqueness": "server"},
                        {"name": "displayName", "type": "string", "multiValued": false, "required": false, "caseExact": false, "mutability": "readWrite", "returned": "default"},
                        {"name": "name", "type": "complex", "multiValued": false, "required": false, "mutability": "readWrite", "returned": "default", "subAttributes": [
                            {"name": "formatted", "type": "string", "multiValued": false, "required": false, "mutability": "readWrite", "returned": "default"},
                            {"name": "givenName", "type": "string", "multiValued": false, "required": false, "mutability": "readWrite", "returned": "default"},
                            {"name": "familyName", "type": "string", "multiValued": false, "required": false, "mutability": "readWrite", "returned": "default"}
                        ]},
                        {"name": "emails", "type": "complex", "multiValued": true, "required": false, "mutability": "readWrite", "returned": "default", "subAttributes": [
                            {"name": "value", "type": "string", "multiValued": false, "required": true, "mutability": "readWrite", "returned": "default"},
                            {"name": "type", "type": "string", "multiValued": false, "required": false, "mutability": "readWrite", "returned": "default"},
                            {"name": "primary", "type": "boolean", "multiValued": false, "required": false, "mutability": "readWrite", "returned": "default"}
                        ]},
                        {"name": "active", "type": "boolean", "multiValued": false, "required": false, "mutability": "readWrite", "returned": "default"},
                        {"name": "groups", "type": "complex", "multiValued": true, "required": false, "mutability": "readOnly", "returned": "default"}
                    ]
                },
                {
                    "id": GROUP_SCHEMA,
                    "name": "Group",
                    "description": "SCIM core Group with HPA role extension",
                    "attributes": [
                        {"name": "displayName", "type": "string", "multiValued": false, "required": true, "caseExact": false, "mutability": "readWrite", "returned": "default", "uniqueness": "server"},
                        {"name": "externalId", "type": "string", "multiValued": false, "required": false, "caseExact": true, "mutability": "readWrite", "returned": "default", "uniqueness": "server"},
                        {"name": "members", "type": "complex", "multiValued": true, "required": false, "mutability": "readWrite", "returned": "default", "subAttributes": [
                            {"name": "value", "type": "string", "multiValued": false, "required": true, "mutability": "readWrite", "returned": "default"},
                            {"name": "display", "type": "string", "multiValued": false, "required": false, "mutability": "readOnly", "returned": "default"},
                            {"name": "$ref", "type": "reference", "multiValued": false, "required": false, "mutability": "readOnly", "returned": "default"}
                        ]}
                    ]
                },
                {
                    "id": ROLE_SCHEMA,
                    "name": "HPA role extension",
                    "description": "Maps a Group to one fixed HPA tenant role",
                    "attributes": [
                        {"name": "role", "type": "string", "multiValued": false, "required": false}
                    ]
                }
            ]
        }),
        None,
    ))
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListQuery {
    filter: Option<String>,
    start_index: Option<i64>,
    count: Option<i64>,
}

enum UserFilter {
    Id(String),
    UserName(String),
    ExternalId(String),
    Active(bool),
}

enum GroupFilter {
    Id(String),
    DisplayName(String),
    ExternalId(String),
}

fn parse_eq_filter(filter: Option<&str>) -> Result<Option<(&str, String)>, ScimError> {
    let Some(filter) = filter.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let lower = filter.to_ascii_lowercase();
    let Some(position) = lower.find(" eq ") else {
        return Err(ScimError::invalid_filter(
            "only exact `attribute eq value` filters are supported",
        ));
    };
    if lower[position + 4..].contains(" and ") || lower[position + 4..].contains(" or ") {
        return Err(ScimError::invalid_filter(
            "compound SCIM filters are not supported",
        ));
    }
    let attribute = filter[..position].trim();
    if attribute.is_empty()
        || !attribute
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'.')
    {
        return Err(ScimError::invalid_filter("invalid filter attribute"));
    }
    let raw = filter[position + 4..].trim();
    if raw.is_empty() {
        return Err(ScimError::invalid_filter("filter value is required"));
    }
    let value = if raw.starts_with('"') {
        if !raw.ends_with('"') || raw.len() < 2 {
            return Err(ScimError::invalid_filter(
                "unterminated quoted filter value",
            ));
        }
        raw[1..raw.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")
    } else {
        raw.to_string()
    };
    Ok(Some((attribute, value)))
}

fn user_filter(filter: Option<&str>) -> Result<Option<UserFilter>, ScimError> {
    let Some((attribute, value)) = parse_eq_filter(filter)? else {
        return Ok(None);
    };
    match attribute.to_ascii_lowercase().as_str() {
        "id" => Ok(Some(UserFilter::Id(value))),
        "username" => Ok(Some(UserFilter::UserName(value))),
        "externalid" => Ok(Some(UserFilter::ExternalId(value))),
        "active" => match value.to_ascii_lowercase().as_str() {
            "true" => Ok(Some(UserFilter::Active(true))),
            "false" => Ok(Some(UserFilter::Active(false))),
            _ => Err(ScimError::invalid_filter(
                "active filter must be true or false",
            )),
        },
        _ => Err(ScimError::invalid_filter(
            "supported User filters are id, userName, externalId and active",
        )),
    }
}

fn group_filter(filter: Option<&str>) -> Result<Option<GroupFilter>, ScimError> {
    let Some((attribute, value)) = parse_eq_filter(filter)? else {
        return Ok(None);
    };
    match attribute.to_ascii_lowercase().as_str() {
        "id" => Ok(Some(GroupFilter::Id(value))),
        "displayname" => Ok(Some(GroupFilter::DisplayName(value))),
        "externalid" => Ok(Some(GroupFilter::ExternalId(value))),
        _ => Err(ScimError::invalid_filter(
            "supported Group filters are id, displayName and externalId",
        )),
    }
}

fn page(query: &ListQuery) -> (i64, i64, i64) {
    let start = query.start_index.unwrap_or(1).max(1);
    let count = query.count.unwrap_or(100).clamp(0, MAX_PAGE_SIZE);
    (start, count, start - 1)
}

#[derive(Debug)]
struct MemberRecord {
    id: String,
    tenant_id: String,
    subject: String,
    user_name: Option<String>,
    external_id: Option<String>,
    display_name: String,
    email: Option<String>,
    role: String,
    base_role: String,
    scim_json: Value,
    status: String,
    created_at: String,
    updated_at: String,
}

fn member_record(row: &sqlx::sqlite::SqliteRow) -> MemberRecord {
    MemberRecord {
        id: row.get("id"),
        tenant_id: row.get("tenant_id"),
        subject: row.get("subject"),
        user_name: row.get("user_name"),
        external_id: row.get("external_id"),
        display_name: row.get("display_name"),
        email: row.get("email"),
        role: row.get("role"),
        base_role: row.get("base_role"),
        scim_json: serde_json::from_str::<Value>(&row.get::<String, _>("scim_json"))
            .ok()
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({})),
        status: row.get("status"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

impl From<ScimMemberRecord> for MemberRecord {
    fn from(record: ScimMemberRecord) -> Self {
        Self {
            id: record.id,
            tenant_id: record.tenant_id,
            subject: record.subject,
            user_name: record.user_name,
            external_id: record.external_id,
            display_name: record.display_name,
            email: record.email,
            role: record.role,
            base_role: record.base_role,
            scim_json: record.scim_json,
            status: record.status,
            created_at: record.created_at,
            updated_at: record.updated_at,
        }
    }
}

const MEMBER_COLUMNS: &str = "id, tenant_id, subject, user_name, external_id, display_name, \
                              email, role, base_role, scim_json, status, created_at, updated_at";

async fn load_member(
    state: &EnterpriseState,
    tenant_id: &str,
    id: &str,
) -> Result<Option<MemberRecord>, ScimError> {
    if let Some(control) = state.control_store() {
        let member = control
            .scim_member_by_id(tenant_id, id)
            .await
            .map_err(ScimError::from_control)?;
        return Ok(member.map(MemberRecord::from));
    }
    let sql =
        format!("SELECT {MEMBER_COLUMNS} FROM enterprise_members WHERE tenant_id = ? AND id = ?");
    let row = sqlx::query(AssertSqlSafe(sql))
        .bind(tenant_id)
        .bind(id)
        .fetch_optional(&state.control)
        .await
        .map_err(ScimError::database)?;
    Ok(row.as_ref().map(member_record))
}

async fn user_resource(state: &EnterpriseState, member: &MemberRecord) -> Result<Value, ScimError> {
    let groups = if let Some(control) = state.control_store() {
        control
            .scim_member_groups(&member.tenant_id, &member.id)
            .await
            .map_err(ScimError::from_control)?
    } else {
        let rows = sqlx::query(
            "SELECT g.id, g.display_name
             FROM enterprise_scim_groups g
             JOIN enterprise_scim_group_members gm
               ON gm.group_id = g.id AND gm.tenant_id = g.tenant_id
             WHERE gm.tenant_id = ? AND gm.member_id = ?
             ORDER BY lower(g.display_name)",
        )
        .bind(&member.tenant_id)
        .bind(&member.id)
        .fetch_all(&state.control)
        .await
        .map_err(ScimError::database)?;
        rows.iter()
            .map(|row| {
                Ok((
                    row.get::<String, _>("id"),
                    row.get::<String, _>("display_name"),
                ))
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()
            .map_err(ScimError::database)?
    };
    let mut resource = member.scim_json.clone();
    let object = resource
        .as_object_mut()
        .ok_or_else(|| ScimError::database(sqlx::Error::Protocol("invalid SCIM JSON".into())))?;
    object.insert("schemas".to_string(), json!([USER_SCHEMA, ROLE_SCHEMA]));
    object.insert("id".to_string(), Value::String(member.id.clone()));
    object.insert(
        "userName".to_string(),
        Value::String(
            member
                .user_name
                .clone()
                .unwrap_or_else(|| member.subject.clone()),
        ),
    );
    if let Some(external_id) = &member.external_id {
        object.insert("externalId".to_string(), Value::String(external_id.clone()));
    } else {
        object.remove("externalId");
    }
    object.insert(
        "displayName".to_string(),
        Value::String(member.display_name.clone()),
    );
    object.insert("active".to_string(), Value::Bool(member.status == "active"));
    if object.get("emails").is_none()
        && let Some(email) = &member.email
    {
        object.insert(
            "emails".to_string(),
            json!([{"value": email, "primary": true}]),
        );
    }
    if !groups.is_empty() {
        object.insert(
            "groups".to_string(),
            Value::Array(
                groups
                    .iter()
                    .map(|(id, display_name)| {
                        json!({
                            "value": id,
                            "display": display_name,
                            "$ref": format!("/scim/v2/Groups/{id}")
                        })
                    })
                    .collect(),
            ),
        );
    } else {
        object.remove("groups");
    }
    object.insert(
        ROLE_SCHEMA.to_string(),
        json!({"role": member.base_role, "effectiveRole": member.role}),
    );
    object.insert(
        "meta".to_string(),
        json!({
            "resourceType": "User",
            "created": member.created_at,
            "lastModified": member.updated_at,
            "location": format!("/scim/v2/Users/{}", member.id),
            "version": version_for(&member.id, &member.updated_at)
        }),
    );
    Ok(resource)
}

async fn list_users(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Response, ScimError> {
    let principal = authorize(&state, &headers).await?;
    let filter = user_filter(query.filter.as_deref())?;
    let (start, count, offset) = page(&query);
    if let Some(control) = state.control_store() {
        let members = control
            .scim_list_members(&principal.tenant_id)
            .await
            .map_err(ScimError::from_control)?
            .into_iter()
            .map(MemberRecord::from)
            .filter(|member| match &filter {
                None => true,
                Some(UserFilter::Id(value)) => member.id == *value,
                Some(UserFilter::UserName(value)) => member
                    .user_name
                    .as_deref()
                    .unwrap_or(&member.subject)
                    .eq_ignore_ascii_case(value),
                Some(UserFilter::ExternalId(value)) => {
                    member.external_id.as_deref() == Some(value.as_str())
                }
                Some(UserFilter::Active(value)) => (member.status == "active") == *value,
            })
            .collect::<Vec<_>>();
        let total = members.len() as i64;
        let skip = usize::try_from(offset).unwrap_or(usize::MAX);
        let take = usize::try_from(count).unwrap_or(usize::MAX);
        let mut resources = Vec::new();
        if count != 0 {
            for member in members.into_iter().skip(skip).take(take) {
                resources.push(user_resource(&state, &member).await?);
            }
        }
        return Ok(scim_response(
            StatusCode::OK,
            json!({
                "schemas": [LIST_SCHEMA],
                "totalResults": total,
                "itemsPerPage": resources.len(),
                "startIndex": start,
                "Resources": resources
            }),
            None,
        ));
    }
    let (where_clause, bind_value) = match filter {
        None => (" WHERE tenant_id = ?".to_string(), None),
        Some(UserFilter::Id(value)) => (" WHERE tenant_id = ? AND id = ?".to_string(), Some(value)),
        Some(UserFilter::UserName(value)) => (
            " WHERE tenant_id = ? AND lower(COALESCE(user_name, subject)) = lower(?)".to_string(),
            Some(value),
        ),
        Some(UserFilter::ExternalId(value)) => (
            " WHERE tenant_id = ? AND external_id = ?".to_string(),
            Some(value),
        ),
        Some(UserFilter::Active(value)) => (
            " WHERE tenant_id = ? AND status = ?".to_string(),
            Some(if value { "active" } else { "disabled" }.to_string()),
        ),
    };
    let count_sql = format!("SELECT COUNT(*) FROM enterprise_members{where_clause}");
    let mut count_query =
        sqlx::query_scalar::<_, i64>(AssertSqlSafe(count_sql)).bind(&principal.tenant_id);
    if let Some(value) = &bind_value {
        count_query = count_query.bind(value);
    }
    let total = count_query
        .fetch_one(&state.control)
        .await
        .map_err(ScimError::database)?;
    let rows = if count == 0 {
        Vec::new()
    } else {
        let sql = format!(
            "SELECT {MEMBER_COLUMNS} FROM enterprise_members{where_clause}
             ORDER BY created_at DESC LIMIT ? OFFSET ?"
        );
        let mut rows_query = sqlx::query(AssertSqlSafe(sql)).bind(&principal.tenant_id);
        if let Some(value) = &bind_value {
            rows_query = rows_query.bind(value);
        }
        rows_query
            .bind(count)
            .bind(offset)
            .fetch_all(&state.control)
            .await
            .map_err(ScimError::database)?
    };
    let mut resources = Vec::with_capacity(rows.len());
    for row in &rows {
        let member = member_record(row);
        resources.push(user_resource(&state, &member).await?);
    }
    Ok(scim_response(
        StatusCode::OK,
        json!({
            "schemas": [LIST_SCHEMA],
            "totalResults": total,
            "itemsPerPage": resources.len(),
            "startIndex": start,
            "Resources": resources
        }),
        None,
    ))
}

struct UserInput {
    user_name: String,
    external_id: Option<String>,
    display_name: String,
    email: Option<String>,
    active: bool,
    base_role: String,
    scim_json: Value,
}

fn parse_user_input(value: &Value) -> Result<UserInput, ScimError> {
    let object = value
        .as_object()
        .ok_or_else(|| ScimError::invalid_syntax("User request must be a JSON object"))?;
    require_schema(object, USER_SCHEMA)?;
    let user_name = required_string(object, "userName", 255)?;
    let external_id = optional_string(object, "externalId", 255)?;
    let display_name = optional_string(object, "displayName", 200)?
        .or_else(|| {
            object
                .get("name")
                .and_then(Value::as_object)
                .and_then(|name| name.get("formatted"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| user_name.clone());
    if display_name.len() > 200 {
        return Err(ScimError::invalid(
            "displayName must be at most 200 characters",
        ));
    }
    let emails = sanitize_emails(object.get("emails"))?;
    let email = emails
        .iter()
        .find(|email| email.get("primary").and_then(Value::as_bool) == Some(true))
        .or_else(|| emails.first())
        .and_then(|email| email.get("value"))
        .and_then(Value::as_str)
        .map(str::to_ascii_lowercase);
    if let Some(email) = &email {
        validate_email(email)?;
    }
    let active = match object.get("active") {
        None | Some(Value::Null) => true,
        Some(Value::Bool(value)) => *value,
        Some(_) => return Err(ScimError::invalid("active must be a boolean")),
    };
    let base_role = parse_role_extension(object)?.unwrap_or_else(|| "viewer".to_string());
    let mut attributes = object.clone();
    for field in [
        "schemas",
        "id",
        "userName",
        "externalId",
        "displayName",
        "active",
        "groups",
        "meta",
        "emails",
        ROLE_SCHEMA,
    ] {
        attributes.remove(field);
    }
    if !emails.is_empty() {
        attributes.insert("emails".to_string(), Value::Array(emails));
    }
    Ok(UserInput {
        user_name,
        external_id,
        display_name,
        email,
        active,
        base_role,
        scim_json: Value::Object(attributes),
    })
}

fn required_string(
    object: &Map<String, Value>,
    field: &str,
    max_len: usize,
) -> Result<String, ScimError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .filter(|value| value.len() <= max_len)
        .map(str::to_string)
        .ok_or_else(|| {
            ScimError::invalid(format!(
                "{field} is required and must be at most {max_len} characters"
            ))
        })
}

fn require_schema(object: &Map<String, Value>, expected: &str) -> Result<(), ScimError> {
    let schemas = object
        .get("schemas")
        .and_then(Value::as_array)
        .ok_or_else(|| ScimError::invalid("SCIM resource must declare its schemas"))?;
    if !schemas
        .iter()
        .filter_map(Value::as_str)
        .any(|schema| schema.eq_ignore_ascii_case(expected))
    {
        return Err(ScimError::invalid(format!(
            "SCIM resource must declare schema `{expected}`"
        )));
    }
    Ok(())
}

async fn ensure_email_available(
    pool: &SqlitePool,
    tenant_id: &str,
    email: Option<&str>,
    except_member_id: Option<&str>,
) -> Result<(), ScimError> {
    let Some(email) = email else {
        return Ok(());
    };
    let existing: Option<String> = sqlx::query_scalar(
        "SELECT id FROM enterprise_members
         WHERE tenant_id = ? AND email IS NOT NULL AND lower(email) = lower(?)
           AND (? IS NULL OR id != ?)
         LIMIT 1",
    )
    .bind(tenant_id)
    .bind(email)
    .bind(except_member_id)
    .bind(except_member_id)
    .fetch_optional(pool)
    .await
    .map_err(ScimError::database)?;
    if existing.is_some() {
        return Err(ScimError::conflict(
            "email address is already assigned to another SCIM User",
        ));
    }
    Ok(())
}

fn optional_string(
    object: &Map<String, Value>,
    field: &str,
    max_len: usize,
) -> Result<Option<String>, ScimError> {
    match object.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => {
            let value = value.trim();
            if value.is_empty() {
                return Ok(None);
            }
            if value.len() > max_len {
                return Err(ScimError::invalid(format!(
                    "{field} must be at most {max_len} characters"
                )));
            }
            Ok(Some(value.to_string()))
        }
        Some(_) => Err(ScimError::invalid(format!("{field} must be a string"))),
    }
}

fn validate_email(email: &str) -> Result<(), ScimError> {
    if email.len() > 320 || !email.contains('@') {
        return Err(ScimError::invalid(
            "email value must contain @ and be at most 320 characters",
        ));
    }
    Ok(())
}

fn sanitize_emails(value: Option<&Value>) -> Result<Vec<Value>, ScimError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let entries = value
        .as_array()
        .ok_or_else(|| ScimError::invalid("emails must be an array"))?;
    if entries.len() > 20 {
        return Err(ScimError::invalid("emails must contain at most 20 entries"));
    }
    let mut sanitized = Vec::new();
    for entry in entries {
        let email = entry
            .as_object()
            .and_then(|object| object.get("value"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ScimError::invalid("each email requires a value"))?;
        validate_email(email)?;
        let mut email = email.to_ascii_lowercase();
        if let Some(object) = entry.as_object() {
            email = object
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or(email.as_str())
                .to_string();
        }
        let mut clean = entry.as_object().cloned().unwrap_or_default();
        clean.insert("value".to_string(), Value::String(email));
        sanitized.push(Value::Object(clean));
    }
    Ok(sanitized)
}

fn parse_role_extension(object: &Map<String, Value>) -> Result<Option<String>, ScimError> {
    let Some(extension) = object.get(ROLE_SCHEMA) else {
        return Ok(None);
    };
    let role = extension
        .as_object()
        .and_then(|extension| extension.get("role"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ScimError::invalid("role extension requires a role string"))?;
    validate_role(role)?;
    Ok(Some(role.to_string()))
}

fn validate_role(role: &str) -> Result<(), ScimError> {
    if matches!(
        role,
        "owner"
            | "admin"
            | "developer"
            | "publisher"
            | "operator"
            | "approver"
            | "auditor"
            | "viewer"
    ) {
        Ok(())
    } else {
        Err(ScimError::invalid("unknown role"))
    }
}

fn ensure_role_assignment_allowed(
    principal: &Principal,
    role: &str,
    existing_role: Option<&str>,
) -> Result<(), ScimError> {
    if principal.platform_admin || principal.role == "owner" {
        return Ok(());
    }
    if (role == "owner" && existing_role != Some("owner"))
        || (existing_role == Some("owner") && role != "owner")
    {
        return Err(ScimError::forbidden(
            "only an owner or platform admin can grant or remove the owner role",
        ));
    }
    Ok(())
}

fn ensure_owner_record_change_allowed(
    principal: &Principal,
    roles: &[&str],
) -> Result<(), ScimError> {
    if principal.platform_admin || principal.role == "owner" {
        return Ok(());
    }
    if roles.contains(&"owner") {
        return Err(ScimError::forbidden(
            "only an owner or platform admin can modify an owner User or Group",
        ));
    }
    Ok(())
}

async fn create_user(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Response, ScimError> {
    let principal = authorize(&state, &headers).await?;
    let value = scim_json_body(payload)?;
    let input = parse_user_input(&value)?;
    ensure_role_assignment_allowed(&principal, &input.base_role, None)?;
    let subject = input
        .external_id
        .clone()
        .unwrap_or_else(|| input.user_name.clone());
    let id = format!("entmem_{}", Uuid::now_v7());
    let now = super::now_rfc3339();
    let status = if input.active { "active" } else { "disabled" };
    if let Some(control) = state.control_store() {
        let record = ScimMemberRecord {
            id: id.clone(),
            tenant_id: principal.tenant_id.clone(),
            subject,
            user_name: Some(input.user_name.clone()),
            external_id: input.external_id.clone(),
            display_name: input.display_name.clone(),
            email: input.email.clone(),
            role: input.base_role.clone(),
            base_role: input.base_role.clone(),
            scim_json: input.scim_json.clone(),
            status: status.to_string(),
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        control
            .scim_create_member(&record)
            .await
            .map_err(ScimError::from_control)?;
    } else {
        ensure_email_available(
            &state.control,
            &principal.tenant_id,
            input.email.as_deref(),
            None,
        )
        .await?;
        let inserted = sqlx::query(
            "INSERT INTO enterprise_members
             (id, tenant_id, subject, user_name, external_id, display_name, email, role,
              base_role, scim_json, status, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&principal.tenant_id)
        .bind(&subject)
        .bind(&input.user_name)
        .bind(&input.external_id)
        .bind(&input.display_name)
        .bind(&input.email)
        .bind(&input.base_role)
        .bind(&input.base_role)
        .bind(serde_json::to_string(&input.scim_json).map_err(|error| {
            ScimError::invalid(format!("cannot serialize SCIM attributes: {error}"))
        })?)
        .bind(status)
        .bind(&now)
        .bind(&now)
        .execute(&state.control)
        .await
        .map_err(ScimError::database)?;
        if inserted.rows_affected() != 1 {
            return Err(ScimError::conflict("SCIM User was not created"));
        }
    }
    state
        .audit(
            Some(&principal.tenant_id),
            Some(&principal.token_id),
            "token",
            "scim.user.create",
            "enterprise_member",
            "allow",
            None,
            json!({"memberId": id, "userName": input.user_name, "role": input.base_role}),
        )
        .await;
    let member = load_member(&state, &principal.tenant_id, &id)
        .await?
        .ok_or_else(|| ScimError::not_found("SCIM User was not created"))?;
    let version = version_for(&member.id, &member.updated_at);
    let resource = user_resource(&state, &member).await?;
    let mut response = scim_response(StatusCode::CREATED, resource, Some(&version));
    if let Ok(location) = HeaderValue::from_str(&format!("/scim/v2/Users/{id}")) {
        response.headers_mut().insert(header::LOCATION, location);
    }
    Ok(response)
}

async fn get_user(
    State(state): State<EnterpriseState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ScimError> {
    let principal = authorize(&state, &headers).await?;
    let member = load_member(&state, &principal.tenant_id, &id)
        .await?
        .ok_or_else(|| ScimError::not_found(format!("User `{id}` not found")))?;
    let version = version_for(&member.id, &member.updated_at);
    Ok(scim_response(
        StatusCode::OK,
        user_resource(&state, &member).await?,
        Some(&version),
    ))
}

async fn replace_user(
    State(state): State<EnterpriseState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Response, ScimError> {
    let principal = authorize(&state, &headers).await?;
    let value = scim_json_body(payload)?;
    let member = load_member(&state, &principal.tenant_id, &id)
        .await?
        .ok_or_else(|| ScimError::not_found(format!("User `{id}` not found")))?;
    ensure_if_match(&headers, &member)?;
    ensure_owner_record_change_allowed(
        &principal,
        &[member.base_role.as_str(), member.role.as_str()],
    )?;
    let input = parse_user_input(&value)?;
    ensure_role_assignment_allowed(&principal, &input.base_role, Some(&member.base_role))?;
    update_user(&state, &principal, &member, input).await
}

async fn patch_user(
    State(state): State<EnterpriseState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Response, ScimError> {
    let principal = authorize(&state, &headers).await?;
    let value = scim_json_body(payload)?;
    let member = load_member(&state, &principal.tenant_id, &id)
        .await?
        .ok_or_else(|| ScimError::not_found(format!("User `{id}` not found")))?;
    ensure_if_match(&headers, &member)?;
    ensure_owner_record_change_allowed(
        &principal,
        &[member.base_role.as_str(), member.role.as_str()],
    )?;
    let input = patch_user_input(&member, &value)?;
    ensure_role_assignment_allowed(&principal, &input.base_role, Some(&member.base_role))?;
    update_user(&state, &principal, &member, input).await
}

async fn update_user(
    state: &EnterpriseState,
    principal: &Principal,
    member: &MemberRecord,
    input: UserInput,
) -> Result<Response, ScimError> {
    let subject = input
        .external_id
        .clone()
        .unwrap_or_else(|| input.user_name.clone());
    let status = if input.active { "active" } else { "disabled" };
    if let Some(control) = state.control_store() {
        let record = ScimMemberRecord {
            id: member.id.clone(),
            tenant_id: member.tenant_id.clone(),
            subject,
            user_name: Some(input.user_name.clone()),
            external_id: input.external_id.clone(),
            display_name: input.display_name.clone(),
            email: input.email.clone(),
            role: member.role.clone(),
            base_role: input.base_role.clone(),
            scim_json: input.scim_json.clone(),
            status: status.to_string(),
            created_at: member.created_at.clone(),
            updated_at: super::now_rfc3339(),
        };
        control
            .scim_update_member(&record)
            .await
            .map_err(ScimError::from_control)?;
    } else {
        let mut transaction = state.control.begin().await.map_err(ScimError::database)?;
        ensure_email_available(
            &state.control,
            &principal.tenant_id,
            input.email.as_deref(),
            Some(&member.id),
        )
        .await?;
        sqlx::query(
            "UPDATE enterprise_members
             SET subject = ?, user_name = ?, external_id = ?, display_name = ?, email = ?,
                 base_role = ?, scim_json = ?, status = ?, updated_at = ?
             WHERE tenant_id = ? AND id = ?",
        )
        .bind(&subject)
        .bind(&input.user_name)
        .bind(&input.external_id)
        .bind(&input.display_name)
        .bind(&input.email)
        .bind(&input.base_role)
        .bind(serde_json::to_string(&input.scim_json).map_err(|error| {
            ScimError::invalid(format!("cannot serialize SCIM attributes: {error}"))
        })?)
        .bind(status)
        .bind(super::now_rfc3339())
        .bind(&principal.tenant_id)
        .bind(&member.id)
        .execute(&mut *transaction)
        .await
        .map_err(ScimError::database)?;
        sync_member_role(&mut transaction, &principal.tenant_id, &member.id).await?;
        transaction.commit().await.map_err(ScimError::database)?;
    }
    state
        .audit(
            Some(&principal.tenant_id),
            Some(&principal.token_id),
            "token",
            "scim.user.update",
            "enterprise_member",
            "allow",
            None,
            json!({"memberId": member.id, "userName": input.user_name, "active": input.active}),
        )
        .await;
    let updated = load_member(state, &principal.tenant_id, &member.id)
        .await?
        .ok_or_else(|| ScimError::not_found("SCIM User no longer exists"))?;
    let version = version_for(&updated.id, &updated.updated_at);
    Ok(scim_response(
        StatusCode::OK,
        user_resource(state, &updated).await?,
        Some(&version),
    ))
}

fn patch_user_input(member: &MemberRecord, document: &Value) -> Result<UserInput, ScimError> {
    let operations = patch_operations(document)?;
    let mut attributes = member.scim_json.as_object().cloned().unwrap_or_default();
    let mut user_name = member
        .user_name
        .clone()
        .unwrap_or_else(|| member.subject.clone());
    let mut external_id = member.external_id.clone();
    let mut display_name = member.display_name.clone();
    let mut active = member.status == "active";
    let mut base_role = member.base_role.clone();
    let mut emails = attributes
        .get("emails")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for operation in operations {
        let op = operation
            .get("op")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_ascii_lowercase();
        if !matches!(op.as_str(), "add" | "replace" | "remove") {
            return Err(ScimError::invalid(
                "PATCH op must be add, replace or remove",
            ));
        }
        let path = operation.get("path").and_then(Value::as_str).map(str::trim);
        let value = operation.get("value");
        if path.is_none() {
            let Some(value) = value.and_then(Value::as_object) else {
                return Err(ScimError::invalid_syntax(
                    "PATCH without path requires an object value",
                ));
            };
            apply_user_object(
                value,
                &mut user_name,
                &mut external_id,
                &mut display_name,
                &mut active,
                &mut base_role,
                &mut emails,
                &mut attributes,
            )?;
            continue;
        }
        let path = path.unwrap();
        let canonical = canonical_path(path);
        match canonical.as_str() {
            "username" => {
                if op == "remove" {
                    return Err(ScimError::invalid("userName cannot be removed"));
                }
                user_name = patch_string(value, "userName", 255)?;
            }
            "externalid" => {
                external_id = if op == "remove" {
                    None
                } else {
                    patch_optional_string(value, "externalId", 255)?
                };
            }
            "displayname" => {
                display_name = if op == "remove" {
                    user_name.clone()
                } else {
                    patch_string(value, "displayName", 200)?
                };
            }
            "active" => {
                active = if op == "remove" {
                    false
                } else {
                    value
                        .and_then(Value::as_bool)
                        .ok_or_else(|| ScimError::invalid("active must be a boolean"))?
                };
            }
            "emails" => {
                emails = if op == "remove" {
                    Vec::new()
                } else {
                    sanitize_emails(Some(value.unwrap_or(&Value::Null)))?
                };
            }
            "role" => {
                base_role = if op == "remove" {
                    "viewer".to_string()
                } else {
                    let role = value
                        .and_then(Value::as_str)
                        .ok_or_else(|| ScimError::invalid("role must be a string"))?;
                    validate_role(role)?;
                    role.to_string()
                };
            }
            "groups" | "meta" | "schemas" | "id" => {
                return Err(ScimError::invalid(format!(
                    "attribute `{path}` is read-only"
                )));
            }
            _ => {
                let mut root = Value::Object(attributes.clone());
                if op == "remove" {
                    remove_json_path(&mut root, path)?;
                } else {
                    let Some(value) = value else {
                        return Err(ScimError::invalid("PATCH value is required"));
                    };
                    set_json_path(&mut root, path, value.clone())?;
                }
                attributes = root.as_object().cloned().unwrap_or_default();
            }
        }
    }
    if !emails.is_empty() {
        attributes.insert("emails".to_string(), Value::Array(emails.clone()));
    } else {
        attributes.remove("emails");
    }
    let email = emails
        .iter()
        .find(|email| email.get("primary").and_then(Value::as_bool) == Some(true))
        .or_else(|| emails.first())
        .and_then(|email| email.get("value"))
        .and_then(Value::as_str)
        .map(str::to_ascii_lowercase);
    if let Some(email) = &email {
        validate_email(email)?;
    }
    Ok(UserInput {
        user_name,
        external_id,
        display_name,
        email,
        active,
        base_role,
        scim_json: Value::Object(attributes),
    })
}

#[allow(clippy::too_many_arguments)]
fn apply_user_object(
    object: &Map<String, Value>,
    user_name: &mut String,
    external_id: &mut Option<String>,
    display_name: &mut String,
    active: &mut bool,
    base_role: &mut String,
    emails: &mut Vec<Value>,
    attributes: &mut Map<String, Value>,
) -> Result<(), ScimError> {
    for (field, value) in object {
        match field.as_str() {
            "schemas" | "id" | "groups" | "meta" => {}
            "userName" => *user_name = patch_string(Some(value), "userName", 255)?,
            "externalId" => *external_id = patch_optional_string(Some(value), "externalId", 255)?,
            "displayName" => *display_name = patch_string(Some(value), "displayName", 200)?,
            "active" => {
                *active = value
                    .as_bool()
                    .ok_or_else(|| ScimError::invalid("active must be a boolean"))?
            }
            "emails" => *emails = sanitize_emails(Some(value))?,
            ROLE_SCHEMA => {
                let role = value
                    .as_object()
                    .and_then(|value| value.get("role"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| ScimError::invalid("role extension requires a role"))?;
                validate_role(role)?;
                *base_role = role.to_string();
            }
            _ => {
                attributes.insert(field.clone(), value.clone());
            }
        }
    }
    Ok(())
}

fn patch_operations(document: &Value) -> Result<&[Value], ScimError> {
    let object = document
        .as_object()
        .ok_or_else(|| ScimError::invalid_syntax("PATCH request must be an object"))?;
    if let Some(schemas) = object.get("schemas").and_then(Value::as_array)
        && !schemas
            .iter()
            .filter_map(Value::as_str)
            .any(|schema| schema.eq_ignore_ascii_case(PATCH_SCHEMA))
    {
        return Err(ScimError::invalid(
            "PATCH request must declare the SCIM PatchOp schema",
        ));
    }
    let operations = object
        .get("Operations")
        .or_else(|| object.get("operations"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .filter(|operations| !operations.is_empty())
        .ok_or_else(|| ScimError::invalid("PATCH Operations must be a non-empty array"))?;
    if operations.len() > 100 {
        return Err(ScimError::invalid(
            "PATCH Operations must contain at most 100 entries",
        ));
    }
    Ok(operations)
}

fn canonical_path(path: &str) -> String {
    let lower = path.trim().to_ascii_lowercase();
    if lower == ROLE_SCHEMA.to_ascii_lowercase()
        || lower == format!("{}.role", ROLE_SCHEMA.to_ascii_lowercase())
    {
        return "role".to_string();
    }
    if let Some(rest) = lower.strip_prefix(&format!("{}:", ROLE_SCHEMA.to_ascii_lowercase()))
        && rest == "role"
    {
        return "role".to_string();
    }
    lower
}

fn patch_string(value: Option<&Value>, field: &str, max_len: usize) -> Result<String, ScimError> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .filter(|value| value.len() <= max_len)
        .map(str::to_string)
        .ok_or_else(|| {
            ScimError::invalid(format!(
                "{field} must be a non-empty string of at most {max_len} characters"
            ))
        })
}

fn patch_optional_string(
    value: Option<&Value>,
    field: &str,
    max_len: usize,
) -> Result<Option<String>, ScimError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value.trim().is_empty() => Ok(None),
        Some(Value::String(value)) if value.trim().len() <= max_len => {
            Ok(Some(value.trim().to_string()))
        }
        _ => Err(ScimError::invalid(format!(
            "{field} must be a string of at most {max_len} characters"
        ))),
    }
}

fn set_json_path(root: &mut Value, path: &str, value: Value) -> Result<(), ScimError> {
    let segments: Vec<&str> = path
        .split('.')
        .filter(|segment| !segment.is_empty())
        .collect();
    if segments.is_empty() {
        return Err(ScimError::invalid("PATCH path is empty"));
    }
    let mut current = root;
    for segment in &segments[..segments.len() - 1] {
        let object = current
            .as_object_mut()
            .ok_or_else(|| ScimError::invalid("PATCH path does not address an object"))?;
        current = object
            .entry((*segment).to_string())
            .or_insert_with(|| json!({}));
    }
    let object = current
        .as_object_mut()
        .ok_or_else(|| ScimError::invalid("PATCH path does not address an object"))?;
    object.insert(segments[segments.len() - 1].to_string(), value);
    Ok(())
}

fn remove_json_path(root: &mut Value, path: &str) -> Result<(), ScimError> {
    let segments: Vec<&str> = path
        .split('.')
        .filter(|segment| !segment.is_empty())
        .collect();
    if segments.is_empty() {
        return Err(ScimError::invalid("PATCH path is empty"));
    }
    let mut current = root;
    for segment in &segments[..segments.len() - 1] {
        let Some(object) = current.as_object_mut() else {
            return Ok(());
        };
        let Some(next) = object.get_mut(*segment) else {
            return Ok(());
        };
        current = next;
    }
    if let Some(object) = current.as_object_mut() {
        object.remove(segments[segments.len() - 1]);
    }
    Ok(())
}

async fn delete_user(
    State(state): State<EnterpriseState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ScimError> {
    let principal = authorize(&state, &headers).await?;
    let member = load_member(&state, &principal.tenant_id, &id)
        .await?
        .ok_or_else(|| ScimError::not_found(format!("User `{id}` not found")))?;
    ensure_if_match(&headers, &member)?;
    ensure_owner_record_change_allowed(
        &principal,
        &[member.base_role.as_str(), member.role.as_str()],
    )?;
    if let Some(control) = state.control_store() {
        control
            .scim_delete_member(&principal.tenant_id, &id)
            .await
            .map_err(ScimError::from_control)?;
    } else {
        sqlx::query("DELETE FROM enterprise_members WHERE tenant_id = ? AND id = ?")
            .bind(&principal.tenant_id)
            .bind(&id)
            .execute(&state.control)
            .await
            .map_err(ScimError::database)?;
    }
    state
        .audit(
            Some(&principal.tenant_id),
            Some(&principal.token_id),
            "token",
            "scim.user.delete",
            "enterprise_member",
            "allow",
            None,
            json!({"memberId": id}),
        )
        .await;
    Ok(no_content())
}

#[derive(Debug)]
struct GroupRecord {
    id: String,
    display_name: String,
    external_id: Option<String>,
    role: Option<String>,
    scim_json: Value,
    created_at: String,
    updated_at: String,
}

fn group_record(row: &sqlx::sqlite::SqliteRow) -> GroupRecord {
    GroupRecord {
        id: row.get("id"),
        display_name: row.get("display_name"),
        external_id: row.get("external_id"),
        role: row.get("role"),
        scim_json: serde_json::from_str::<Value>(&row.get::<String, _>("scim_json"))
            .ok()
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({})),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

impl From<ScimGroupRecord> for GroupRecord {
    fn from(record: ScimGroupRecord) -> Self {
        Self {
            id: record.id,
            display_name: record.display_name,
            external_id: record.external_id,
            role: record.role,
            scim_json: record.scim_json,
            created_at: record.created_at,
            updated_at: record.updated_at,
        }
    }
}

const GROUP_COLUMNS: &str =
    "id, display_name, external_id, role, scim_json, created_at, updated_at";

async fn load_group(
    state: &EnterpriseState,
    tenant_id: &str,
    id: &str,
) -> Result<Option<GroupRecord>, ScimError> {
    if let Some(control) = state.control_store() {
        let group = control
            .scim_group_by_id(tenant_id, id)
            .await
            .map_err(ScimError::from_control)?;
        return Ok(group.map(GroupRecord::from));
    }
    let sql = format!(
        "SELECT {GROUP_COLUMNS} FROM enterprise_scim_groups WHERE tenant_id = ? AND id = ?"
    );
    let row = sqlx::query(AssertSqlSafe(sql))
        .bind(tenant_id)
        .bind(id)
        .fetch_optional(&state.control)
        .await
        .map_err(ScimError::database)?;
    Ok(row.as_ref().map(group_record))
}

async fn group_member_rows(
    state: &EnterpriseState,
    tenant_id: &str,
    group_id: &str,
) -> Result<Vec<MemberRecord>, ScimError> {
    if let Some(control) = state.control_store() {
        return Ok(control
            .scim_group_members(tenant_id, group_id)
            .await
            .map_err(ScimError::from_control)?
            .into_iter()
            .map(MemberRecord::from)
            .collect());
    }
    let sql = "SELECT m.id, m.tenant_id, m.subject, m.user_name, m.external_id, m.display_name,
                m.email, m.role, m.base_role, m.scim_json, m.status, m.created_at, m.updated_at
         FROM enterprise_members m
         JOIN enterprise_scim_group_members gm
           ON gm.member_id = m.id AND gm.tenant_id = m.tenant_id
         WHERE gm.tenant_id = ? AND gm.group_id = ?
         ORDER BY lower(m.display_name)"
        .to_string();
    let rows = sqlx::query(AssertSqlSafe(sql))
        .bind(tenant_id)
        .bind(group_id)
        .fetch_all(&state.control)
        .await
        .map_err(ScimError::database)?;
    Ok(rows.iter().map(member_record).collect())
}

async fn group_resource(
    state: &EnterpriseState,
    tenant_id: &str,
    group: &GroupRecord,
) -> Result<Value, ScimError> {
    let members = group_member_rows(state, tenant_id, &group.id).await?;
    let mut resource = group.scim_json.clone();
    let object = resource
        .as_object_mut()
        .ok_or_else(|| ScimError::database(sqlx::Error::Protocol("invalid SCIM JSON".into())))?;
    object.insert("schemas".to_string(), json!([GROUP_SCHEMA, ROLE_SCHEMA]));
    object.insert("id".to_string(), Value::String(group.id.clone()));
    object.insert(
        "displayName".to_string(),
        Value::String(group.display_name.clone()),
    );
    if let Some(external_id) = &group.external_id {
        object.insert("externalId".to_string(), Value::String(external_id.clone()));
    } else {
        object.remove("externalId");
    }
    object.insert(
        "members".to_string(),
        Value::Array(
            members
                .iter()
                .map(|member| {
                    json!({
                        "value": member.id,
                        "display": member.display_name,
                        "type": "User",
                        "$ref": format!("/scim/v2/Users/{}", member.id)
                    })
                })
                .collect(),
        ),
    );
    object.insert(ROLE_SCHEMA.to_string(), json!({"role": group.role}));
    object.insert(
        "meta".to_string(),
        json!({
            "resourceType": "Group",
            "created": group.created_at,
            "lastModified": group.updated_at,
            "location": format!("/scim/v2/Groups/{}", group.id),
            "version": version_for(&group.id, &group.updated_at)
        }),
    );
    Ok(resource)
}

async fn list_groups(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Response, ScimError> {
    let principal = authorize(&state, &headers).await?;
    let filter = group_filter(query.filter.as_deref())?;
    let (start, count, offset) = page(&query);
    if let Some(control) = state.control_store() {
        let groups = control
            .scim_list_groups(&principal.tenant_id)
            .await
            .map_err(ScimError::from_control)?
            .into_iter()
            .map(GroupRecord::from)
            .filter(|group| match &filter {
                None => true,
                Some(GroupFilter::Id(value)) => group.id == *value,
                Some(GroupFilter::DisplayName(value)) => {
                    group.display_name.eq_ignore_ascii_case(value)
                }
                Some(GroupFilter::ExternalId(value)) => {
                    group.external_id.as_deref() == Some(value.as_str())
                }
            })
            .collect::<Vec<_>>();
        let total = groups.len() as i64;
        let skip = usize::try_from(offset).unwrap_or(usize::MAX);
        let take = usize::try_from(count).unwrap_or(usize::MAX);
        let mut resources = Vec::new();
        if count != 0 {
            for group in groups.into_iter().skip(skip).take(take) {
                resources.push(group_resource(&state, &principal.tenant_id, &group).await?);
            }
        }
        return Ok(scim_response(
            StatusCode::OK,
            json!({
                "schemas": [LIST_SCHEMA],
                "totalResults": total,
                "itemsPerPage": resources.len(),
                "startIndex": start,
                "Resources": resources
            }),
            None,
        ));
    }
    let (where_clause, bind_value) = match filter {
        None => (" WHERE tenant_id = ?".to_string(), None),
        Some(GroupFilter::Id(value)) => {
            (" WHERE tenant_id = ? AND id = ?".to_string(), Some(value))
        }
        Some(GroupFilter::DisplayName(value)) => (
            " WHERE tenant_id = ? AND lower(display_name) = lower(?)".to_string(),
            Some(value),
        ),
        Some(GroupFilter::ExternalId(value)) => (
            " WHERE tenant_id = ? AND external_id = ?".to_string(),
            Some(value),
        ),
    };
    let count_sql = format!("SELECT COUNT(*) FROM enterprise_scim_groups{where_clause}");
    let mut count_query =
        sqlx::query_scalar::<_, i64>(AssertSqlSafe(count_sql)).bind(&principal.tenant_id);
    if let Some(value) = &bind_value {
        count_query = count_query.bind(value);
    }
    let total = count_query
        .fetch_one(&state.control)
        .await
        .map_err(ScimError::database)?;
    let rows = if count == 0 {
        Vec::new()
    } else {
        let sql = format!(
            "SELECT {GROUP_COLUMNS} FROM enterprise_scim_groups{where_clause}
             ORDER BY created_at DESC LIMIT ? OFFSET ?"
        );
        let mut rows_query = sqlx::query(AssertSqlSafe(sql)).bind(&principal.tenant_id);
        if let Some(value) = &bind_value {
            rows_query = rows_query.bind(value);
        }
        rows_query
            .bind(count)
            .bind(offset)
            .fetch_all(&state.control)
            .await
            .map_err(ScimError::database)?
    };
    let mut resources = Vec::with_capacity(rows.len());
    for row in &rows {
        let group = group_record(row);
        resources.push(group_resource(&state, &principal.tenant_id, &group).await?);
    }
    Ok(scim_response(
        StatusCode::OK,
        json!({
            "schemas": [LIST_SCHEMA],
            "totalResults": total,
            "itemsPerPage": resources.len(),
            "startIndex": start,
            "Resources": resources
        }),
        None,
    ))
}

struct GroupInput {
    display_name: String,
    external_id: Option<String>,
    role: Option<String>,
    members: BTreeSet<String>,
    scim_json: Value,
}

fn parse_group_input(value: &Value) -> Result<GroupInput, ScimError> {
    let object = value
        .as_object()
        .ok_or_else(|| ScimError::invalid_syntax("Group request must be a JSON object"))?;
    require_schema(object, GROUP_SCHEMA)?;
    let display_name = required_string(object, "displayName", 200)?;
    let external_id = optional_string(object, "externalId", 255)?;
    let role = parse_role_extension(object)?;
    let members = sanitize_members(object.get("members"))?;
    let mut attributes = object.clone();
    for field in [
        "schemas",
        "id",
        "displayName",
        "externalId",
        "members",
        "meta",
        ROLE_SCHEMA,
    ] {
        attributes.remove(field);
    }
    Ok(GroupInput {
        display_name,
        external_id,
        role,
        members,
        scim_json: Value::Object(attributes),
    })
}

fn sanitize_members(value: Option<&Value>) -> Result<BTreeSet<String>, ScimError> {
    let Some(value) = value else {
        return Ok(BTreeSet::new());
    };
    let entries = value
        .as_array()
        .ok_or_else(|| ScimError::invalid("members must be an array"))?;
    if entries.len() > 1000 {
        return Err(ScimError::invalid(
            "group members must contain at most 1000 entries",
        ));
    }
    let mut members = BTreeSet::new();
    for entry in entries {
        let id = entry
            .as_object()
            .and_then(|object| object.get("value"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ScimError::invalid("each group member requires a value"))?;
        if !members.insert(id.to_string()) {
            return Err(ScimError::invalid(
                "group members must not contain duplicates",
            ));
        }
    }
    Ok(members)
}

async fn create_group(
    State(state): State<EnterpriseState>,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Response, ScimError> {
    let principal = authorize(&state, &headers).await?;
    let value = scim_json_body(payload)?;
    let input = parse_group_input(&value)?;
    if let Some(role) = input.role.as_deref() {
        ensure_role_assignment_allowed(&principal, role, None)?;
    }
    let id = format!("scimgrp_{}", Uuid::now_v7());
    let now = super::now_rfc3339();
    if let Some(control) = state.control_store() {
        let record = ScimGroupRecord {
            id: id.clone(),
            tenant_id: principal.tenant_id.clone(),
            display_name: input.display_name.clone(),
            external_id: input.external_id.clone(),
            role: input.role.clone(),
            scim_json: input.scim_json.clone(),
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        control
            .scim_create_group(&record, &input.members)
            .await
            .map_err(ScimError::from_control)?;
    } else {
        let mut transaction = state.control.begin().await.map_err(ScimError::database)?;
        sqlx::query(
            "INSERT INTO enterprise_scim_groups
             (id, tenant_id, display_name, external_id, role, scim_json, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&principal.tenant_id)
        .bind(&input.display_name)
        .bind(&input.external_id)
        .bind(&input.role)
        .bind(serde_json::to_string(&input.scim_json).map_err(|error| {
            ScimError::invalid(format!("cannot serialize SCIM attributes: {error}"))
        })?)
        .bind(&now)
        .bind(&now)
        .execute(&mut *transaction)
        .await
        .map_err(ScimError::database)?;
        replace_group_members(&mut transaction, &principal.tenant_id, &id, &input.members).await?;
        sync_group_members(&mut transaction, &principal.tenant_id, &id).await?;
        transaction.commit().await.map_err(ScimError::database)?;
    }
    state
        .audit(
            Some(&principal.tenant_id),
            Some(&principal.token_id),
            "token",
            "scim.group.create",
            "enterprise_scim_group",
            "allow",
            None,
            json!({"groupId": id, "role": input.role, "members": input.members.len()}),
        )
        .await;
    let group = load_group(&state, &principal.tenant_id, &id)
        .await?
        .ok_or_else(|| ScimError::not_found("SCIM Group was not created"))?;
    let version = version_for(&group.id, &group.updated_at);
    let mut response = scim_response(
        StatusCode::CREATED,
        group_resource(&state, &principal.tenant_id, &group).await?,
        Some(&version),
    );
    if let Ok(location) = HeaderValue::from_str(&format!("/scim/v2/Groups/{id}")) {
        response.headers_mut().insert(header::LOCATION, location);
    }
    Ok(response)
}

async fn get_group(
    State(state): State<EnterpriseState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ScimError> {
    let principal = authorize(&state, &headers).await?;
    let group = load_group(&state, &principal.tenant_id, &id)
        .await?
        .ok_or_else(|| ScimError::not_found(format!("Group `{id}` not found")))?;
    let version = version_for(&group.id, &group.updated_at);
    Ok(scim_response(
        StatusCode::OK,
        group_resource(&state, &principal.tenant_id, &group).await?,
        Some(&version),
    ))
}

async fn replace_group(
    State(state): State<EnterpriseState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Response, ScimError> {
    let principal = authorize(&state, &headers).await?;
    let value = scim_json_body(payload)?;
    let group = load_group(&state, &principal.tenant_id, &id)
        .await?
        .ok_or_else(|| ScimError::not_found(format!("Group `{id}` not found")))?;
    ensure_group_if_match(&headers, &group)?;
    if let Some(role) = group.role.as_deref() {
        ensure_owner_record_change_allowed(&principal, &[role])?;
    }
    let input = parse_group_input(&value)?;
    if let Some(role) = input.role.as_deref() {
        ensure_role_assignment_allowed(&principal, role, group.role.as_deref())?;
    }
    update_group(&state, &principal, &group, input).await
}

async fn patch_group(
    State(state): State<EnterpriseState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Response, ScimError> {
    let principal = authorize(&state, &headers).await?;
    let value = scim_json_body(payload)?;
    let group = load_group(&state, &principal.tenant_id, &id)
        .await?
        .ok_or_else(|| ScimError::not_found(format!("Group `{id}` not found")))?;
    ensure_group_if_match(&headers, &group)?;
    if let Some(role) = group.role.as_deref() {
        ensure_owner_record_change_allowed(&principal, &[role])?;
    }
    let input = patch_group_input(&state, &principal.tenant_id, &group, &value).await?;
    if let Some(role) = input.role.as_deref() {
        ensure_role_assignment_allowed(&principal, role, group.role.as_deref())?;
    }
    update_group(&state, &principal, &group, input).await
}

async fn patch_group_input(
    state: &EnterpriseState,
    tenant_id: &str,
    group: &GroupRecord,
    document: &Value,
) -> Result<GroupInput, ScimError> {
    let operations = patch_operations(document)?;
    let mut display_name = group.display_name.clone();
    let mut external_id = group.external_id.clone();
    let mut role = group.role.clone();
    let mut attributes = group.scim_json.as_object().cloned().unwrap_or_default();
    let mut members: BTreeSet<String> = group_member_rows(state, tenant_id, &group.id)
        .await?
        .into_iter()
        .map(|member| member.id)
        .collect();
    for operation in operations {
        let op = operation
            .get("op")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_ascii_lowercase();
        if !matches!(op.as_str(), "add" | "replace" | "remove") {
            return Err(ScimError::invalid(
                "PATCH op must be add, replace or remove",
            ));
        }
        let path = operation.get("path").and_then(Value::as_str).map(str::trim);
        let value = operation.get("value");
        if path.is_none() {
            let Some(value) = value.and_then(Value::as_object) else {
                return Err(ScimError::invalid_syntax(
                    "PATCH without path requires an object value",
                ));
            };
            for (field, field_value) in value {
                match field.as_str() {
                    "schemas" | "id" | "meta" => {}
                    "displayName" => {
                        display_name = patch_string(Some(field_value), "displayName", 200)?
                    }
                    "externalId" => {
                        external_id = patch_optional_string(Some(field_value), "externalId", 255)?
                    }
                    "members" => {
                        members = sanitize_members(Some(field_value))?;
                    }
                    ROLE_SCHEMA => {
                        role = field_value
                            .as_object()
                            .and_then(|value| value.get("role"))
                            .and_then(Value::as_str)
                            .map(str::to_string);
                        if let Some(role) = &role {
                            validate_role(role)?;
                        }
                    }
                    _ => {
                        attributes.insert(field.clone(), field_value.clone());
                    }
                }
            }
            continue;
        }
        let path = path.unwrap();
        let canonical = canonical_path(path);
        match canonical.as_str() {
            "displayname" => {
                if op == "remove" {
                    return Err(ScimError::invalid("displayName cannot be removed"));
                }
                display_name = patch_string(value, "displayName", 200)?;
            }
            "externalid" => {
                external_id = if op == "remove" {
                    None
                } else {
                    patch_optional_string(value, "externalId", 255)?
                };
            }
            "role" => {
                role = if op == "remove" {
                    None
                } else {
                    let role = value
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .or_else(|| {
                            value
                                .and_then(Value::as_object)
                                .and_then(|value| value.get("role"))
                                .and_then(Value::as_str)
                                .map(str::to_string)
                        })
                        .ok_or_else(|| {
                            ScimError::invalid("role must be a string or role object")
                        })?;
                    validate_role(&role)?;
                    Some(role)
                };
            }
            "members" => match op.as_str() {
                "replace" => members = sanitize_members(value)?,
                "add" => members.extend(sanitize_members(value)?),
                "remove" => {
                    for id in sanitize_members(value)? {
                        members.remove(&id);
                    }
                }
                _ => {}
            },
            path if path.starts_with("members[value eq ") => {
                let id = path
                    .strip_prefix("members[value eq ")
                    .and_then(|value| value.strip_suffix(']'))
                    .map(|value| value.trim().trim_matches('"'))
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| ScimError::invalid("invalid members filter path"))?;
                if op != "remove" {
                    return Err(ScimError::invalid(
                        "filtered members path only supports remove",
                    ));
                }
                members.remove(id);
            }
            "schemas" | "id" | "meta" => {
                return Err(ScimError::invalid(format!(
                    "attribute `{path}` is read-only"
                )));
            }
            _ => {
                let mut root = Value::Object(attributes.clone());
                if op == "remove" {
                    remove_json_path(&mut root, path)?;
                } else {
                    let Some(value) = value else {
                        return Err(ScimError::invalid("PATCH value is required"));
                    };
                    set_json_path(&mut root, path, value.clone())?;
                }
                attributes = root.as_object().cloned().unwrap_or_default();
            }
        }
    }
    Ok(GroupInput {
        display_name,
        external_id,
        role,
        members,
        scim_json: Value::Object(attributes),
    })
}

async fn update_group(
    state: &EnterpriseState,
    principal: &Principal,
    group: &GroupRecord,
    input: GroupInput,
) -> Result<Response, ScimError> {
    let affected_before = group_member_rows(state, &principal.tenant_id, &group.id).await?;
    if let Some(control) = state.control_store() {
        let record = ScimGroupRecord {
            id: group.id.clone(),
            tenant_id: principal.tenant_id.clone(),
            display_name: input.display_name.clone(),
            external_id: input.external_id.clone(),
            role: input.role.clone(),
            scim_json: input.scim_json.clone(),
            created_at: group.created_at.clone(),
            updated_at: super::now_rfc3339(),
        };
        control
            .scim_update_group(&record, &input.members)
            .await
            .map_err(ScimError::from_control)?;
    } else {
        let mut transaction = state.control.begin().await.map_err(ScimError::database)?;
        sqlx::query(
            "UPDATE enterprise_scim_groups
             SET display_name = ?, external_id = ?, role = ?, scim_json = ?, updated_at = ?
             WHERE tenant_id = ? AND id = ?",
        )
        .bind(&input.display_name)
        .bind(&input.external_id)
        .bind(&input.role)
        .bind(serde_json::to_string(&input.scim_json).map_err(|error| {
            ScimError::invalid(format!("cannot serialize SCIM attributes: {error}"))
        })?)
        .bind(super::now_rfc3339())
        .bind(&principal.tenant_id)
        .bind(&group.id)
        .execute(&mut *transaction)
        .await
        .map_err(ScimError::database)?;
        replace_group_members(
            &mut transaction,
            &principal.tenant_id,
            &group.id,
            &input.members,
        )
        .await?;
        let mut affected: BTreeSet<String> = affected_before
            .into_iter()
            .map(|member| member.id)
            .collect();
        affected.extend(input.members.iter().cloned());
        for member_id in affected {
            sync_member_role(&mut transaction, &principal.tenant_id, &member_id).await?;
        }
        transaction.commit().await.map_err(ScimError::database)?;
    }
    state
        .audit(
            Some(&principal.tenant_id),
            Some(&principal.token_id),
            "token",
            "scim.group.update",
            "enterprise_scim_group",
            "allow",
            None,
            json!({"groupId": group.id, "role": input.role, "members": input.members.len()}),
        )
        .await;
    let updated = load_group(state, &principal.tenant_id, &group.id)
        .await?
        .ok_or_else(|| ScimError::not_found("SCIM Group no longer exists"))?;
    let version = version_for(&updated.id, &updated.updated_at);
    Ok(scim_response(
        StatusCode::OK,
        group_resource(state, &principal.tenant_id, &updated).await?,
        Some(&version),
    ))
}

async fn delete_group(
    State(state): State<EnterpriseState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ScimError> {
    let principal = authorize(&state, &headers).await?;
    let group = load_group(&state, &principal.tenant_id, &id)
        .await?
        .ok_or_else(|| ScimError::not_found(format!("Group `{id}` not found")))?;
    ensure_group_if_match(&headers, &group)?;
    if let Some(role) = group.role.as_deref() {
        ensure_owner_record_change_allowed(&principal, &[role])?;
    }
    let affected = group_member_rows(&state, &principal.tenant_id, &id).await?;
    if let Some(control) = state.control_store() {
        control
            .scim_delete_group(&principal.tenant_id, &id)
            .await
            .map_err(ScimError::from_control)?;
    } else {
        let mut transaction = state.control.begin().await.map_err(ScimError::database)?;
        sqlx::query("DELETE FROM enterprise_scim_groups WHERE tenant_id = ? AND id = ?")
            .bind(&principal.tenant_id)
            .bind(&id)
            .execute(&mut *transaction)
            .await
            .map_err(ScimError::database)?;
        for member in affected {
            sync_member_role(&mut transaction, &principal.tenant_id, &member.id).await?;
        }
        transaction.commit().await.map_err(ScimError::database)?;
    }
    state
        .audit(
            Some(&principal.tenant_id),
            Some(&principal.token_id),
            "token",
            "scim.group.delete",
            "enterprise_scim_group",
            "allow",
            None,
            json!({"groupId": id}),
        )
        .await;
    Ok(no_content())
}

async fn replace_group_members(
    transaction: &mut Transaction<'_, Sqlite>,
    tenant_id: &str,
    group_id: &str,
    members: &BTreeSet<String>,
) -> Result<(), ScimError> {
    for member_id in members {
        let exists: Option<String> =
            sqlx::query_scalar("SELECT id FROM enterprise_members WHERE tenant_id = ? AND id = ?")
                .bind(tenant_id)
                .bind(member_id)
                .fetch_optional(&mut **transaction)
                .await
                .map_err(ScimError::database)?;
        if exists.is_none() {
            return Err(ScimError::invalid(format!(
                "group member `{member_id}` does not exist in this tenant"
            )));
        }
    }
    sqlx::query("DELETE FROM enterprise_scim_group_members WHERE tenant_id = ? AND group_id = ?")
        .bind(tenant_id)
        .bind(group_id)
        .execute(&mut **transaction)
        .await
        .map_err(ScimError::database)?;
    for member_id in members {
        sqlx::query(
            "INSERT INTO enterprise_scim_group_members
             (tenant_id, group_id, member_id, created_at)
             VALUES (?, ?, ?, ?)",
        )
        .bind(tenant_id)
        .bind(group_id)
        .bind(member_id)
        .bind(super::now_rfc3339())
        .execute(&mut **transaction)
        .await
        .map_err(ScimError::database)?;
    }
    Ok(())
}

async fn sync_group_members(
    transaction: &mut Transaction<'_, Sqlite>,
    tenant_id: &str,
    group_id: &str,
) -> Result<(), ScimError> {
    let member_ids: Vec<String> = sqlx::query_scalar(
        "SELECT member_id FROM enterprise_scim_group_members
         WHERE tenant_id = ? AND group_id = ?",
    )
    .bind(tenant_id)
    .bind(group_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(ScimError::database)?;
    for member_id in member_ids {
        sync_member_role(transaction, tenant_id, &member_id).await?;
    }
    Ok(())
}

async fn sync_member_role(
    transaction: &mut Transaction<'_, Sqlite>,
    tenant_id: &str,
    member_id: &str,
) -> Result<(), ScimError> {
    let base_role: String = sqlx::query_scalar(
        "SELECT base_role FROM enterprise_members WHERE tenant_id = ? AND id = ?",
    )
    .bind(tenant_id)
    .bind(member_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(ScimError::database)?
    .ok_or_else(|| ScimError::invalid(format!("member `{member_id}` does not exist")))?;
    let roles: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT g.role
         FROM enterprise_scim_groups g
         JOIN enterprise_scim_group_members gm
           ON gm.group_id = g.id AND gm.tenant_id = g.tenant_id
         WHERE gm.tenant_id = ? AND gm.member_id = ? AND g.role IS NOT NULL
         ORDER BY g.role",
    )
    .bind(tenant_id)
    .bind(member_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(ScimError::database)?;
    if roles.len() > 1 {
        return Err(ScimError::new(
            StatusCode::CONFLICT,
            Some("invalidValue"),
            "member belongs to SCIM Groups mapped to different roles",
        ));
    }
    let effective_role = roles.first().cloned().unwrap_or(base_role);
    sqlx::query(
        "UPDATE enterprise_members SET role = ?, updated_at = ?
         WHERE tenant_id = ? AND id = ?",
    )
    .bind(effective_role)
    .bind(super::now_rfc3339())
    .bind(tenant_id)
    .bind(member_id)
    .execute(&mut **transaction)
    .await
    .map_err(ScimError::database)?;
    Ok(())
}

fn version_for(id: &str, updated_at: &str) -> String {
    let digest = Sha256::digest(format!("{id}:{updated_at}").as_bytes());
    format!("W/\"{}\"", &format!("{digest:x}")[..20])
}

fn ensure_if_match(headers: &HeaderMap, member: &MemberRecord) -> Result<(), ScimError> {
    ensure_etag(
        headers,
        &version_for(&member.id, &member.updated_at),
        "User",
    )
}

fn ensure_group_if_match(headers: &HeaderMap, group: &GroupRecord) -> Result<(), ScimError> {
    ensure_etag(headers, &version_for(&group.id, &group.updated_at), "Group")
}

fn ensure_etag(headers: &HeaderMap, current: &str, resource: &str) -> Result<(), ScimError> {
    let Some(provided) = headers.get(header::IF_MATCH) else {
        return Ok(());
    };
    let provided = provided
        .to_str()
        .map_err(|_| ScimError::precondition("If-Match header is invalid"))?;
    if provided.trim() != "*" && provided.trim() != current {
        return Err(ScimError::precondition(format!(
            "{resource} version does not match If-Match"
        )));
    }
    Ok(())
}
