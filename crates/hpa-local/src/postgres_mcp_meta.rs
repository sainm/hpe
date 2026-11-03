use serde_json::Value;
use sqlx::Row;
use sqlx::postgres::{PgPool, PgRow};
use sqlx::types::Json;
use thiserror::Error;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

#[derive(Debug, Error)]
pub enum PostgresMcpMetaError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid MCP metadata: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone)]
pub struct McpTokenRecord {
    pub id: String,
    pub name: String,
    pub scopes_json: String,
    pub tenant_id: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub revoked_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct McpApprovalRecord {
    pub id: String,
    pub token_id: String,
    pub tool_name: String,
    pub arguments_json: String,
    pub arguments_hash: String,
    pub request_state: String,
    pub risk: String,
    pub status: String,
    pub created_at: String,
    pub expires_at: String,
    pub decided_at: Option<String>,
    pub used_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct McpAuditRecord {
    pub id: String,
    pub at: String,
    pub token_id: Option<String>,
    pub method: String,
    pub tool_name: Option<String>,
    pub status: String,
    pub latency_ms: i64,
    pub detail_json: Option<String>,
}

#[derive(Debug, Clone)]
pub struct McpServerRecord {
    pub id: String,
    pub name: String,
    pub transport: String,
    pub command_json: String,
    pub url: Option<String>,
    pub allowed_tools_json: String,
    pub network_json: String,
    pub credential_env: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct McpPluginServerInput {
    pub id: String,
    pub name: String,
    pub transport: String,
    pub command_json: String,
    pub url: Option<String>,
    pub allowed_tools_json: String,
    pub network_json: String,
    pub credential_env: Option<String>,
    pub plugin_id: String,
    pub plugin_version: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone)]
pub struct PostgresMcpMetaStore {
    pool: PgPool,
    tenant_id: String,
}

impl PostgresMcpMetaStore {
    pub fn new(pool: PgPool, tenant_id: impl Into<String>) -> Self {
        Self {
            pool,
            tenant_id: tenant_id.into(),
        }
    }

    async fn begin(&self) -> Result<sqlx::Transaction<'_, sqlx::Postgres>, PostgresMcpMetaError> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query("SET LOCAL ROLE hpa_tenant_runtime")
            .execute(&mut *transaction)
            .await?;
        sqlx::query("SELECT set_config('hpa.tenant_id', $1, true)")
            .bind(&self.tenant_id)
            .execute(&mut *transaction)
            .await?;
        Ok(transaction)
    }

    pub async fn create_token(
        &self,
        name: &str,
        token_hash: &str,
        scopes_json: &str,
        tenant_id: &str,
        created_at: &str,
    ) -> Result<McpTokenRecord, PostgresMcpMetaError> {
        if tenant_id != self.tenant_id {
            return Err(PostgresMcpMetaError::Invalid(
                "MCP token tenant does not match the runtime tenant".to_string(),
            ));
        }
        let scopes: Value = serde_json::from_str(scopes_json)?;
        let id = format!("mcptok_{}", uuid::Uuid::now_v7());
        let mut transaction = self.begin().await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.mcp_tokens
             (tenant_id, id, name, token_hash, scopes_json, created_at, last_used_at, revoked_at)
             VALUES ($1, $2, $3, $4, $5, $6, NULL, NULL)",
        )
        .bind(&self.tenant_id)
        .bind(&id)
        .bind(name)
        .bind(token_hash)
        .bind(Json(&scopes))
        .bind(parse_time(created_at)?)
        .execute(&mut *transaction)
        .await?;
        let row = sqlx::query(
            "SELECT id, name, scopes_json, tenant_id, created_at, last_used_at, revoked_at
             FROM hpa_tenant.mcp_tokens WHERE tenant_id = $1 AND id = $2",
        )
        .bind(&self.tenant_id)
        .bind(&id)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        mcp_token_from_row(&row)
    }

    pub async fn get_token(
        &self,
        id: &str,
    ) -> Result<Option<McpTokenRecord>, PostgresMcpMetaError> {
        let mut transaction = self.begin().await?;
        let row = sqlx::query(
            "SELECT id, name, scopes_json, tenant_id, created_at, last_used_at, revoked_at
             FROM hpa_tenant.mcp_tokens WHERE tenant_id = $1 AND id = $2",
        )
        .bind(&self.tenant_id)
        .bind(id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.as_ref().map(mcp_token_from_row).transpose()
    }

    pub async fn find_token(
        &self,
        token_hash: &str,
        now: &str,
    ) -> Result<Option<McpTokenRecord>, PostgresMcpMetaError> {
        let now = parse_time(now)?;
        let mut transaction = self.begin().await?;
        let row = sqlx::query(
            "SELECT id, name, scopes_json, tenant_id, created_at, last_used_at, revoked_at
             FROM hpa_tenant.mcp_tokens
             WHERE tenant_id = $1 AND token_hash = $2 AND revoked_at IS NULL",
        )
        .bind(&self.tenant_id)
        .bind(token_hash)
        .fetch_optional(&mut *transaction)
        .await?;
        if let Some(row) = &row {
            let id: String = row.try_get("id")?;
            sqlx::query(
                "UPDATE hpa_tenant.mcp_tokens SET last_used_at = $1
                 WHERE tenant_id = $2 AND id = $3",
            )
            .bind(now)
            .bind(&self.tenant_id)
            .bind(&id)
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        row.as_ref().map(mcp_token_from_row).transpose()
    }

    pub async fn list_tokens(&self) -> Result<Vec<McpTokenRecord>, PostgresMcpMetaError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT id, name, scopes_json, tenant_id, created_at, last_used_at, revoked_at
             FROM hpa_tenant.mcp_tokens WHERE tenant_id = $1 ORDER BY created_at DESC",
        )
        .bind(&self.tenant_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.iter().map(mcp_token_from_row).collect()
    }

    pub async fn revoke_token(&self, id: &str, now: &str) -> Result<bool, PostgresMcpMetaError> {
        let mut transaction = self.begin().await?;
        let result = sqlx::query(
            "UPDATE hpa_tenant.mcp_tokens SET revoked_at = $1
             WHERE tenant_id = $2 AND id = $3 AND revoked_at IS NULL",
        )
        .bind(parse_time(now)?)
        .bind(&self.tenant_id)
        .bind(id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(result.rows_affected() > 0)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_approval(
        &self,
        token_id: &str,
        tool_name: &str,
        arguments_json: &str,
        arguments_hash: &str,
        request_state: &str,
        risk: &str,
        created_at: &str,
        expires_at: &str,
    ) -> Result<McpApprovalRecord, PostgresMcpMetaError> {
        let arguments: Value = serde_json::from_str(arguments_json)?;
        let id = format!("mcpappr_{}", uuid::Uuid::now_v7());
        let mut transaction = self.begin().await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.mcp_approvals
             (tenant_id, id, token_id, tool_name, arguments_json, arguments_hash,
              request_state, risk, status, created_at, expires_at, decided_at, used_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'pending', $9, $10, NULL, NULL)",
        )
        .bind(&self.tenant_id)
        .bind(&id)
        .bind(token_id)
        .bind(tool_name)
        .bind(Json(&arguments))
        .bind(arguments_hash)
        .bind(request_state)
        .bind(risk)
        .bind(parse_time(created_at)?)
        .bind(parse_time(expires_at)?)
        .execute(&mut *transaction)
        .await?;
        let row = sqlx::query(
            "SELECT id, token_id, tool_name, arguments_json, arguments_hash, request_state,
                    risk, status, created_at, expires_at, decided_at, used_at
             FROM hpa_tenant.mcp_approvals WHERE tenant_id = $1 AND id = $2",
        )
        .bind(&self.tenant_id)
        .bind(&id)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        mcp_approval_from_row(&row)
    }

    pub async fn get_approval(
        &self,
        id: &str,
    ) -> Result<Option<McpApprovalRecord>, PostgresMcpMetaError> {
        let mut transaction = self.begin().await?;
        let row = sqlx::query(
            "SELECT id, token_id, tool_name, arguments_json, arguments_hash, request_state,
                    risk, status, created_at, expires_at, decided_at, used_at
             FROM hpa_tenant.mcp_approvals WHERE tenant_id = $1 AND id = $2",
        )
        .bind(&self.tenant_id)
        .bind(id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.as_ref().map(mcp_approval_from_row).transpose()
    }

    pub async fn get_approval_by_state(
        &self,
        state: &str,
    ) -> Result<Option<McpApprovalRecord>, PostgresMcpMetaError> {
        let mut transaction = self.begin().await?;
        let row = sqlx::query(
            "SELECT id, token_id, tool_name, arguments_json, arguments_hash, request_state,
                    risk, status, created_at, expires_at, decided_at, used_at
             FROM hpa_tenant.mcp_approvals WHERE tenant_id = $1 AND request_state = $2",
        )
        .bind(&self.tenant_id)
        .bind(state)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.as_ref().map(mcp_approval_from_row).transpose()
    }

    pub async fn list_approvals(
        &self,
        status: Option<&str>,
        limit: i64,
    ) -> Result<Vec<McpApprovalRecord>, PostgresMcpMetaError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT id, token_id, tool_name, arguments_json, arguments_hash, request_state,
                    risk, status, created_at, expires_at, decided_at, used_at
             FROM hpa_tenant.mcp_approvals
             WHERE tenant_id = $1 AND ($2::text IS NULL OR status = $2)
             ORDER BY created_at DESC LIMIT $3",
        )
        .bind(&self.tenant_id)
        .bind(status)
        .bind(limit.clamp(1, 200))
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.iter().map(mcp_approval_from_row).collect()
    }

    pub async fn decide_approval(
        &self,
        id: &str,
        status: &str,
        decided_at: &str,
    ) -> Result<bool, PostgresMcpMetaError> {
        let mut transaction = self.begin().await?;
        let result = sqlx::query(
            "UPDATE hpa_tenant.mcp_approvals SET status = $1, decided_at = $2
             WHERE tenant_id = $3 AND id = $4 AND status = 'pending'",
        )
        .bind(status)
        .bind(parse_time(decided_at)?)
        .bind(&self.tenant_id)
        .bind(id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn consume_approval(
        &self,
        id: &str,
        used_at: &str,
    ) -> Result<bool, PostgresMcpMetaError> {
        let mut transaction = self.begin().await?;
        let result = sqlx::query(
            "UPDATE hpa_tenant.mcp_approvals SET used_at = $1
             WHERE tenant_id = $2 AND id = $3 AND status = 'approved' AND used_at IS NULL",
        )
        .bind(parse_time(used_at)?)
        .bind(&self.tenant_id)
        .bind(id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn expire_approvals(&self, now: &str) -> Result<u64, PostgresMcpMetaError> {
        let mut transaction = self.begin().await?;
        let result = sqlx::query(
            "UPDATE hpa_tenant.mcp_approvals SET status = 'expired'
             WHERE tenant_id = $1 AND status = 'pending' AND expires_at < $2",
        )
        .bind(&self.tenant_id)
        .bind(parse_time(now)?)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(result.rows_affected())
    }

    pub async fn insert_audit(&self, record: &McpAuditRecord) -> Result<(), PostgresMcpMetaError> {
        let detail: Option<Value> = record
            .detail_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?;
        let mut transaction = self.begin().await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.mcp_audit
             (tenant_id, id, at, token_id, method, tool_name, status, latency_ms, detail_json)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(&self.tenant_id)
        .bind(&record.id)
        .bind(parse_time(&record.at)?)
        .bind(&record.token_id)
        .bind(&record.method)
        .bind(&record.tool_name)
        .bind(&record.status)
        .bind(record.latency_ms)
        .bind(detail.map(Json))
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn list_audit(
        &self,
        limit: i64,
    ) -> Result<Vec<McpAuditRecord>, PostgresMcpMetaError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT id, at, token_id, method, tool_name, status, latency_ms, detail_json
             FROM hpa_tenant.mcp_audit WHERE tenant_id = $1
             ORDER BY at DESC LIMIT $2",
        )
        .bind(&self.tenant_id)
        .bind(limit.clamp(1, 500))
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.iter().map(mcp_audit_from_row).collect()
    }

    pub async fn stats(&self) -> Result<(i64, i64, i64), PostgresMcpMetaError> {
        let mut transaction = self.begin().await?;
        let pending: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM hpa_tenant.mcp_approvals
             WHERE tenant_id = $1 AND status = 'pending'",
        )
        .bind(&self.tenant_id)
        .fetch_one(&mut *transaction)
        .await?;
        let tools: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM hpa_tenant.skill_versions
             WHERE tenant_id = $1 AND status = 'published'",
        )
        .bind(&self.tenant_id)
        .fetch_one(&mut *transaction)
        .await?;
        let requests: i64 =
            sqlx::query_scalar("SELECT count(*) FROM hpa_tenant.mcp_audit WHERE tenant_id = $1")
                .bind(&self.tenant_id)
                .fetch_one(&mut *transaction)
                .await?;
        transaction.commit().await?;
        Ok((pending, tools, requests))
    }

    pub async fn upsert_server(
        &self,
        record: &McpServerRecord,
    ) -> Result<(), PostgresMcpMetaError> {
        let command: Value = serde_json::from_str(&record.command_json)?;
        let allowed: Value = serde_json::from_str(&record.allowed_tools_json)?;
        let network: Value = serde_json::from_str(&record.network_json)?;
        let mut transaction = self.begin().await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.mcp_servers
             (tenant_id, id, name, transport, command_json, url, allowed_tools_json,
              network_json, credential_env, enabled, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, TRUE, $10, $11)
             ON CONFLICT (tenant_id, id) DO UPDATE
             SET name = EXCLUDED.name,
                 transport = EXCLUDED.transport,
                 command_json = EXCLUDED.command_json,
                 url = EXCLUDED.url,
                 allowed_tools_json = EXCLUDED.allowed_tools_json,
                 network_json = EXCLUDED.network_json,
                 credential_env = EXCLUDED.credential_env,
                 updated_at = EXCLUDED.updated_at",
        )
        .bind(&self.tenant_id)
        .bind(&record.id)
        .bind(&record.name)
        .bind(&record.transport)
        .bind(Json(&command))
        .bind(&record.url)
        .bind(Json(&allowed))
        .bind(Json(&network))
        .bind(&record.credential_env)
        .bind(parse_time(&record.created_at)?)
        .bind(parse_time(&record.updated_at)?)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn list_servers(&self) -> Result<Vec<McpServerRecord>, PostgresMcpMetaError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT id, name, transport, command_json, url, allowed_tools_json,
                    network_json, credential_env, created_at, updated_at
             FROM hpa_tenant.mcp_servers
             WHERE tenant_id = $1 AND enabled = TRUE ORDER BY name ASC",
        )
        .bind(&self.tenant_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.iter().map(mcp_server_from_row).collect()
    }

    pub async fn get_server(
        &self,
        id: &str,
    ) -> Result<Option<McpServerRecord>, PostgresMcpMetaError> {
        let mut transaction = self.begin().await?;
        let row = sqlx::query(
            "SELECT id, name, transport, command_json, url, allowed_tools_json,
                    network_json, credential_env, created_at, updated_at
             FROM hpa_tenant.mcp_servers
             WHERE tenant_id = $1 AND id = $2 AND enabled = TRUE",
        )
        .bind(&self.tenant_id)
        .bind(id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.as_ref().map(mcp_server_from_row).transpose()
    }

    pub async fn delete_server(&self, id: &str) -> Result<bool, PostgresMcpMetaError> {
        let mut transaction = self.begin().await?;
        let result =
            sqlx::query("DELETE FROM hpa_tenant.mcp_servers WHERE tenant_id = $1 AND id = $2")
                .bind(&self.tenant_id)
                .bind(id)
                .execute(&mut *transaction)
                .await?;
        transaction.commit().await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn plugin_server_owner(
        &self,
        id: &str,
    ) -> Result<Option<(Option<String>, Option<String>)>, PostgresMcpMetaError> {
        let mut transaction = self.begin().await?;
        let row = sqlx::query(
            "SELECT plugin_id, plugin_version FROM hpa_tenant.mcp_servers
             WHERE tenant_id = $1 AND id = $2",
        )
        .bind(&self.tenant_id)
        .bind(id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.map(|row| Ok((row.try_get("plugin_id")?, row.try_get("plugin_version")?)))
            .transpose()
    }

    pub async fn upsert_plugin_server(
        &self,
        input: &McpPluginServerInput,
    ) -> Result<(), PostgresMcpMetaError> {
        let command: Value = serde_json::from_str(&input.command_json)?;
        let allowed: Value = serde_json::from_str(&input.allowed_tools_json)?;
        let network: Value = serde_json::from_str(&input.network_json)?;
        let mut transaction = self.begin().await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.mcp_servers
             (tenant_id, id, name, transport, command_json, url, allowed_tools_json,
              network_json, credential_env, enabled, plugin_id, plugin_version,
              created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, TRUE, $10, $11, $12, $13)
             ON CONFLICT (tenant_id, id) DO UPDATE SET
                name = EXCLUDED.name,
                transport = EXCLUDED.transport,
                command_json = EXCLUDED.command_json,
                url = EXCLUDED.url,
                allowed_tools_json = EXCLUDED.allowed_tools_json,
                network_json = EXCLUDED.network_json,
                credential_env = EXCLUDED.credential_env,
                updated_at = EXCLUDED.updated_at,
                plugin_id = EXCLUDED.plugin_id,
                plugin_version = EXCLUDED.plugin_version,
                enabled = TRUE",
        )
        .bind(&self.tenant_id)
        .bind(&input.id)
        .bind(&input.name)
        .bind(&input.transport)
        .bind(Json(&command))
        .bind(&input.url)
        .bind(Json(&allowed))
        .bind(Json(&network))
        .bind(&input.credential_env)
        .bind(&input.plugin_id)
        .bind(&input.plugin_version)
        .bind(parse_time(&input.created_at)?)
        .bind(parse_time(&input.updated_at)?)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn set_plugin_servers_enabled(
        &self,
        plugin_id: &str,
        plugin_version: &str,
        enabled: bool,
    ) -> Result<u64, PostgresMcpMetaError> {
        let mut transaction = self.begin().await?;
        let result = sqlx::query(
            "UPDATE hpa_tenant.mcp_servers SET enabled = $1
             WHERE tenant_id = $2 AND plugin_id = $3 AND plugin_version = $4",
        )
        .bind(enabled)
        .bind(&self.tenant_id)
        .bind(plugin_id)
        .bind(plugin_version)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(result.rows_affected())
    }

    pub async fn delete_plugin_servers(
        &self,
        plugin_id: &str,
    ) -> Result<u64, PostgresMcpMetaError> {
        let mut transaction = self.begin().await?;
        let result = sqlx::query(
            "DELETE FROM hpa_tenant.mcp_servers WHERE tenant_id = $1 AND plugin_id = $2",
        )
        .bind(&self.tenant_id)
        .bind(plugin_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(result.rows_affected())
    }
}

fn mcp_token_from_row(row: &PgRow) -> Result<McpTokenRecord, PostgresMcpMetaError> {
    let scopes: Value = row.try_get("scopes_json")?;
    Ok(McpTokenRecord {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        scopes_json: serde_json::to_string(&scopes)?,
        tenant_id: row.try_get("tenant_id")?,
        created_at: format_time(row.try_get("created_at")?),
        last_used_at: row
            .try_get::<Option<OffsetDateTime>, _>("last_used_at")?
            .map(format_time),
        revoked_at: row
            .try_get::<Option<OffsetDateTime>, _>("revoked_at")?
            .map(format_time),
    })
}

fn mcp_approval_from_row(row: &PgRow) -> Result<McpApprovalRecord, PostgresMcpMetaError> {
    let arguments: Value = row.try_get("arguments_json")?;
    Ok(McpApprovalRecord {
        id: row.try_get("id")?,
        token_id: row.try_get("token_id")?,
        tool_name: row.try_get("tool_name")?,
        arguments_json: serde_json::to_string(&arguments)?,
        arguments_hash: row.try_get("arguments_hash")?,
        request_state: row.try_get("request_state")?,
        risk: row.try_get("risk")?,
        status: row.try_get("status")?,
        created_at: format_time(row.try_get("created_at")?),
        expires_at: format_time(row.try_get("expires_at")?),
        decided_at: row
            .try_get::<Option<OffsetDateTime>, _>("decided_at")?
            .map(format_time),
        used_at: row
            .try_get::<Option<OffsetDateTime>, _>("used_at")?
            .map(format_time),
    })
}

fn mcp_audit_from_row(row: &PgRow) -> Result<McpAuditRecord, PostgresMcpMetaError> {
    let detail: Option<Value> = row.try_get("detail_json")?;
    Ok(McpAuditRecord {
        id: row.try_get("id")?,
        at: format_time(row.try_get("at")?),
        token_id: row.try_get("token_id")?,
        method: row.try_get("method")?,
        tool_name: row.try_get("tool_name")?,
        status: row.try_get("status")?,
        latency_ms: row.try_get("latency_ms")?,
        detail_json: detail
            .map(|value| serde_json::to_string(&value))
            .transpose()?,
    })
}

fn mcp_server_from_row(row: &PgRow) -> Result<McpServerRecord, PostgresMcpMetaError> {
    let command: Value = row.try_get("command_json")?;
    let allowed: Value = row.try_get("allowed_tools_json")?;
    let network: Value = row.try_get("network_json")?;
    Ok(McpServerRecord {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        transport: row.try_get("transport")?,
        command_json: serde_json::to_string(&command)?,
        url: row.try_get("url")?,
        allowed_tools_json: serde_json::to_string(&allowed)?,
        network_json: serde_json::to_string(&network)?,
        credential_env: row.try_get("credential_env")?,
        created_at: format_time(row.try_get("created_at")?),
        updated_at: format_time(row.try_get("updated_at")?),
    })
}

fn parse_time(raw: &str) -> Result<OffsetDateTime, PostgresMcpMetaError> {
    OffsetDateTime::parse(raw, &Rfc3339).map_err(|error| {
        PostgresMcpMetaError::Invalid(format!("invalid timestamp `{raw}`: {error}"))
    })
}

fn format_time(value: OffsetDateTime) -> String {
    value.format(&Rfc3339).unwrap_or_default()
}
