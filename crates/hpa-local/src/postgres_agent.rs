use serde_json::Value;
use sqlx::Row;
use sqlx::postgres::{PgPool, PgRow};
use sqlx::types::Json;
use thiserror::Error;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

#[derive(Debug, Error)]
pub enum PostgresAgentError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid agent data: {0}")]
    Invalid(String),
    #[error("agent record `{0}` was not found in PostgreSQL")]
    NotFound(String),
    #[error("agent version `{agent_id}@{version}` is frozen")]
    VersionFrozen { agent_id: String, version: String },
}

#[derive(Debug, Clone)]
pub struct AgentSummaryRecord {
    pub id: String,
    pub name: String,
    pub latest_version: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct AgentVersionRecord {
    pub agent_id: String,
    pub version: String,
    pub name: String,
    pub description: Option<String>,
    pub config_json: String,
    pub content_hash: String,
    pub status: String,
    pub created_at: String,
    pub published_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AgentSessionRecord {
    pub id: String,
    pub agent_id: String,
    pub agent_version: String,
    pub status: String,
    pub input_json: String,
    pub output_json: Option<String>,
    pub state_json: String,
    pub error_json: Option<String>,
    pub parent_session_id: Option<String>,
    pub depth: i64,
    pub created_at: String,
    pub updated_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentLeaseRecord {
    pub owner: String,
    pub token: i64,
    pub expires_at: String,
}

#[derive(Debug, Clone)]
pub struct AgentEventRecord {
    pub session_id: String,
    pub sequence: i64,
    pub at: String,
    pub kind: String,
    pub data_json: String,
}

#[derive(Debug, Clone)]
pub struct AgentApprovalRecord {
    pub id: String,
    pub session_id: String,
    pub tool_name: String,
    pub args_json: String,
    pub risk: String,
    pub status: String,
    pub created_at: String,
    pub expires_at: String,
    pub decided_at: Option<String>,
    pub used_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AgentToolCallRecord {
    pub session_id: String,
    pub call_id: String,
    pub idempotency_key: String,
    pub tool_name: String,
    pub request_hash: String,
    pub status: String,
    pub response_json: Option<String>,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone)]
pub struct PostgresAgentStore {
    pool: PgPool,
    tenant_id: String,
}

impl PostgresAgentStore {
    pub fn new(pool: PgPool, tenant_id: impl Into<String>) -> Self {
        Self {
            pool,
            tenant_id: tenant_id.into(),
        }
    }

    pub async fn upsert_agent_version(
        &self,
        row: &AgentVersionRecord,
    ) -> Result<bool, PostgresAgentError> {
        let config = parse_json(&row.config_json)?;
        let created_at = parse_time(&row.created_at)?;
        let mut transaction = self.begin().await?;
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT content_hash FROM hpa_tenant.agent_versions
             WHERE tenant_id = $1 AND agent_id = $2 AND version = $3",
        )
        .bind(&self.tenant_id)
        .bind(&row.agent_id)
        .bind(&row.version)
        .fetch_optional(&mut *transaction)
        .await?;
        if let Some(hash) = existing {
            if hash != row.content_hash {
                transaction.rollback().await?;
                return Err(PostgresAgentError::VersionFrozen {
                    agent_id: row.agent_id.clone(),
                    version: row.version.clone(),
                });
            }
            transaction.commit().await?;
            return Ok(false);
        }
        sqlx::query(
            "INSERT INTO hpa_tenant.agents (
                 tenant_id, id, name, latest_version, status, created_at, updated_at
             )
             VALUES ($1, $2, $3, $4, 'imported', $5, $5)
             ON CONFLICT (tenant_id, id) DO UPDATE
             SET name = EXCLUDED.name,
                 latest_version = EXCLUDED.latest_version,
                 status = EXCLUDED.status,
                 updated_at = EXCLUDED.updated_at",
        )
        .bind(&self.tenant_id)
        .bind(&row.agent_id)
        .bind(&row.name)
        .bind(&row.version)
        .bind(created_at)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.agent_versions (
                 tenant_id, agent_id, version, name, description, config_json,
                 content_hash, status, created_at, published_at
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, 'imported', $8, NULL)",
        )
        .bind(&self.tenant_id)
        .bind(&row.agent_id)
        .bind(&row.version)
        .bind(&row.name)
        .bind(&row.description)
        .bind(Json(&config))
        .bind(&row.content_hash)
        .bind(created_at)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(true)
    }

    pub async fn count_agents(&self) -> Result<i64, PostgresAgentError> {
        let mut transaction = self.begin().await?;
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM hpa_tenant.agents WHERE tenant_id = $1")
                .bind(&self.tenant_id)
                .fetch_one(&mut *transaction)
                .await?;
        transaction.commit().await?;
        Ok(count)
    }

    pub async fn count_agent_sessions_on(&self, day: &str) -> Result<i64, PostgresAgentError> {
        let mut transaction = self.begin().await?;
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM hpa_tenant.agent_sessions
             WHERE tenant_id = $1 AND created_at >= $2::date
               AND created_at < ($2::date + INTERVAL '1 day')",
        )
        .bind(&self.tenant_id)
        .bind(day)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(count)
    }

    pub async fn list_agents(&self) -> Result<Vec<AgentSummaryRecord>, PostgresAgentError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT id, name, latest_version, status, created_at, updated_at
             FROM hpa_tenant.agents
             WHERE tenant_id = $1
             ORDER BY updated_at DESC",
        )
        .bind(&self.tenant_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.into_iter().map(agent_summary_from_row).collect()
    }

    pub async fn get_agent(
        &self,
        id: &str,
    ) -> Result<Option<AgentSummaryRecord>, PostgresAgentError> {
        let mut transaction = self.begin().await?;
        let row = sqlx::query(
            "SELECT id, name, latest_version, status, created_at, updated_at
             FROM hpa_tenant.agents
             WHERE tenant_id = $1 AND id = $2",
        )
        .bind(&self.tenant_id)
        .bind(id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.map(agent_summary_from_row).transpose()
    }

    pub async fn get_agent_version(
        &self,
        agent_id: &str,
        version: &str,
    ) -> Result<Option<AgentVersionRecord>, PostgresAgentError> {
        let mut transaction = self.begin().await?;
        let row = sqlx::query(
            "SELECT agent_id, version, name, description, config_json,
                    content_hash, status, created_at, published_at
             FROM hpa_tenant.agent_versions
             WHERE tenant_id = $1 AND agent_id = $2 AND version = $3",
        )
        .bind(&self.tenant_id)
        .bind(agent_id)
        .bind(version)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.map(agent_version_from_row).transpose()
    }

    pub async fn list_agent_versions(
        &self,
        agent_id: &str,
    ) -> Result<Vec<AgentVersionRecord>, PostgresAgentError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT agent_id, version, name, description, config_json,
                    content_hash, status, created_at, published_at
             FROM hpa_tenant.agent_versions
             WHERE tenant_id = $1 AND agent_id = $2
             ORDER BY created_at DESC",
        )
        .bind(&self.tenant_id)
        .bind(agent_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.into_iter().map(agent_version_from_row).collect()
    }

    pub async fn latest_published_agent_version(
        &self,
        agent_id: &str,
    ) -> Result<Option<AgentVersionRecord>, PostgresAgentError> {
        let mut transaction = self.begin().await?;
        let row = sqlx::query(
            "SELECT agent_id, version, name, description, config_json,
                    content_hash, status, created_at, published_at
             FROM hpa_tenant.agent_versions
             WHERE tenant_id = $1 AND agent_id = $2 AND status = 'published'
             ORDER BY published_at DESC NULLS LAST LIMIT 1",
        )
        .bind(&self.tenant_id)
        .bind(agent_id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.map(agent_version_from_row).transpose()
    }

    pub async fn publish_agent_version(
        &self,
        agent_id: &str,
        version: &str,
        published_at: &str,
    ) -> Result<bool, PostgresAgentError> {
        let published_at = parse_time(published_at)?;
        let mut transaction = self.begin().await?;
        let changed = sqlx::query(
            "UPDATE hpa_tenant.agent_versions
             SET status = 'published', published_at = $4
             WHERE tenant_id = $1 AND agent_id = $2 AND version = $3
               AND status != 'published'",
        )
        .bind(&self.tenant_id)
        .bind(agent_id)
        .bind(version)
        .bind(published_at)
        .execute(&mut *transaction)
        .await?
        .rows_affected()
            > 0;
        sqlx::query(
            "UPDATE hpa_tenant.agents
             SET status = 'published', updated_at = $4
             WHERE tenant_id = $1 AND id = $2 AND latest_version = $3",
        )
        .bind(&self.tenant_id)
        .bind(agent_id)
        .bind(version)
        .bind(published_at)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(changed)
    }

    pub async fn insert_agent_session(
        &self,
        row: &AgentSessionRecord,
    ) -> Result<(), PostgresAgentError> {
        let input = parse_json(&row.input_json)?;
        let output = row.output_json.as_deref().map(parse_json).transpose()?;
        let state = parse_json(&row.state_json)?;
        let error = row.error_json.as_deref().map(parse_json).transpose()?;
        let depth = i32::try_from(row.depth).map_err(|_| {
            PostgresAgentError::Invalid("session depth is out of range".to_string())
        })?;
        let created_at = parse_time(&row.created_at)?;
        let updated_at = parse_time(&row.updated_at)?;
        let finished_at = row.finished_at.as_deref().map(parse_time).transpose()?;
        let mut transaction = self.begin().await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.agent_sessions (
                 tenant_id, id, agent_id, agent_version, status, input_json,
                 output_json, state_json, error_json, parent_session_id,
                 depth, created_at, updated_at, finished_at
             )
             VALUES (
                 $1, $2, $3, $4, $5, $6,
                 $7, $8, $9, $10,
                 $11, $12, $13, $14
             )",
        )
        .bind(&self.tenant_id)
        .bind(&row.id)
        .bind(&row.agent_id)
        .bind(&row.agent_version)
        .bind(&row.status)
        .bind(Json(&input))
        .bind(output.as_ref().map(Json))
        .bind(Json(&state))
        .bind(error.as_ref().map(Json))
        .bind(&row.parent_session_id)
        .bind(depth)
        .bind(created_at)
        .bind(updated_at)
        .bind(finished_at)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn update_agent_session(
        &self,
        row: &AgentSessionRecord,
    ) -> Result<(), PostgresAgentError> {
        let output = row.output_json.as_deref().map(parse_json).transpose()?;
        let state = parse_json(&row.state_json)?;
        let error = row.error_json.as_deref().map(parse_json).transpose()?;
        let updated_at = parse_time(&row.updated_at)?;
        let finished_at = row.finished_at.as_deref().map(parse_time).transpose()?;
        let mut transaction = self.begin().await?;
        sqlx::query(
            "UPDATE hpa_tenant.agent_sessions
             SET status = $2, output_json = $3, state_json = $4,
                 error_json = $5, updated_at = $6, finished_at = $7
             WHERE tenant_id = $1 AND id = $8",
        )
        .bind(&self.tenant_id)
        .bind(&row.status)
        .bind(output.as_ref().map(Json))
        .bind(Json(&state))
        .bind(error.as_ref().map(Json))
        .bind(updated_at)
        .bind(finished_at)
        .bind(&row.id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn get_agent_session(
        &self,
        id: &str,
    ) -> Result<Option<AgentSessionRecord>, PostgresAgentError> {
        let mut transaction = self.begin().await?;
        let row = sqlx::query(
            "SELECT id, agent_id, agent_version, status, input_json,
                    output_json, state_json, error_json, parent_session_id,
                    depth, created_at, updated_at, finished_at
             FROM hpa_tenant.agent_sessions
             WHERE tenant_id = $1 AND id = $2",
        )
        .bind(&self.tenant_id)
        .bind(id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.map(agent_session_from_row).transpose()
    }

    pub async fn list_agent_sessions(
        &self,
        agent_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<AgentSessionRecord>, PostgresAgentError> {
        let limit = limit.clamp(1, 200);
        let mut transaction = self.begin().await?;
        let rows = match agent_id {
            Some(agent_id) => {
                sqlx::query(
                    "SELECT id, agent_id, agent_version, status, input_json,
                            output_json, state_json, error_json, parent_session_id,
                            depth, created_at, updated_at, finished_at
                     FROM hpa_tenant.agent_sessions
                     WHERE tenant_id = $1 AND agent_id = $2
                     ORDER BY created_at DESC LIMIT $3",
                )
                .bind(&self.tenant_id)
                .bind(agent_id)
                .bind(limit)
                .fetch_all(&mut *transaction)
                .await?
            }
            None => {
                sqlx::query(
                    "SELECT id, agent_id, agent_version, status, input_json,
                            output_json, state_json, error_json, parent_session_id,
                            depth, created_at, updated_at, finished_at
                     FROM hpa_tenant.agent_sessions
                     WHERE tenant_id = $1
                     ORDER BY created_at DESC LIMIT $2",
                )
                .bind(&self.tenant_id)
                .bind(limit)
                .fetch_all(&mut *transaction)
                .await?
            }
        };
        transaction.commit().await?;
        rows.into_iter().map(agent_session_from_row).collect()
    }

    pub async fn running_agent_sessions(
        &self,
    ) -> Result<Vec<AgentSessionRecord>, PostgresAgentError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT id, agent_id, agent_version, status, input_json,
                    output_json, state_json, error_json, parent_session_id,
                    depth, created_at, updated_at, finished_at
             FROM hpa_tenant.agent_sessions
             WHERE tenant_id = $1 AND status = 'running'
             ORDER BY created_at ASC",
        )
        .bind(&self.tenant_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.into_iter().map(agent_session_from_row).collect()
    }

    pub async fn append_agent_events(
        &self,
        events: &[AgentEventRecord],
    ) -> Result<(), PostgresAgentError> {
        let mut transaction = self.begin().await?;
        for event in events {
            let data = parse_json(&event.data_json)?;
            let at = parse_time(&event.at)?;
            sqlx::query(
                "INSERT INTO hpa_tenant.agent_events (
                     tenant_id, session_id, sequence, at, kind, data_json
                 )
                 VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT (tenant_id, session_id, sequence) DO UPDATE
                 SET at = EXCLUDED.at,
                     kind = EXCLUDED.kind,
                     data_json = EXCLUDED.data_json",
            )
            .bind(&self.tenant_id)
            .bind(&event.session_id)
            .bind(event.sequence)
            .bind(at)
            .bind(&event.kind)
            .bind(Json(&data))
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn list_agent_events(
        &self,
        session_id: &str,
        limit: i64,
    ) -> Result<Vec<AgentEventRecord>, PostgresAgentError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT session_id, sequence, at, kind, data_json
             FROM hpa_tenant.agent_events
             WHERE tenant_id = $1 AND session_id = $2
             ORDER BY sequence ASC LIMIT $3",
        )
        .bind(&self.tenant_id)
        .bind(session_id)
        .bind(limit.clamp(1, 1000))
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.into_iter().map(agent_event_from_row).collect()
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_agent_approval(
        &self,
        session_id: &str,
        tool_name: &str,
        args_json: &str,
        risk: &str,
        created_at: &str,
        expires_at: &str,
    ) -> Result<AgentApprovalRecord, PostgresAgentError> {
        let id = format!("agentappr_{}", uuid::Uuid::now_v7());
        let args = parse_json(args_json)?;
        let created_at = parse_time(created_at)?;
        let expires_at = parse_time(expires_at)?;
        let mut transaction = self.begin().await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.agent_approvals (
                 tenant_id, id, session_id, tool_name, args_json, risk,
                 status, created_at, expires_at, decided_at, used_at
             )
             VALUES ($1, $2, $3, $4, $5, $6, 'pending', $7, $8, NULL, NULL)",
        )
        .bind(&self.tenant_id)
        .bind(&id)
        .bind(session_id)
        .bind(tool_name)
        .bind(Json(&args))
        .bind(risk)
        .bind(created_at)
        .bind(expires_at)
        .execute(&mut *transaction)
        .await?;
        let row = sqlx::query(
            "SELECT id, session_id, tool_name, args_json, risk, status,
                    created_at, expires_at, decided_at, used_at
             FROM hpa_tenant.agent_approvals
             WHERE tenant_id = $1 AND id = $2",
        )
        .bind(&self.tenant_id)
        .bind(&id)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        agent_approval_from_row(row)
    }

    pub async fn get_agent_approval(
        &self,
        id: &str,
    ) -> Result<Option<AgentApprovalRecord>, PostgresAgentError> {
        let mut transaction = self.begin().await?;
        let row = sqlx::query(
            "SELECT id, session_id, tool_name, args_json, risk, status,
                    created_at, expires_at, decided_at, used_at
             FROM hpa_tenant.agent_approvals
             WHERE tenant_id = $1 AND id = $2",
        )
        .bind(&self.tenant_id)
        .bind(id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.map(agent_approval_from_row).transpose()
    }

    pub async fn list_agent_approvals(
        &self,
        status: Option<&str>,
        limit: i64,
    ) -> Result<Vec<AgentApprovalRecord>, PostgresAgentError> {
        let limit = limit.clamp(1, 200);
        let mut transaction = self.begin().await?;
        let rows = match status {
            Some(status) => {
                sqlx::query(
                    "SELECT id, session_id, tool_name, args_json, risk, status,
                            created_at, expires_at, decided_at, used_at
                     FROM hpa_tenant.agent_approvals
                     WHERE tenant_id = $1 AND status = $2
                     ORDER BY created_at DESC LIMIT $3",
                )
                .bind(&self.tenant_id)
                .bind(status)
                .bind(limit)
                .fetch_all(&mut *transaction)
                .await?
            }
            None => {
                sqlx::query(
                    "SELECT id, session_id, tool_name, args_json, risk, status,
                            created_at, expires_at, decided_at, used_at
                     FROM hpa_tenant.agent_approvals
                     WHERE tenant_id = $1
                     ORDER BY created_at DESC LIMIT $2",
                )
                .bind(&self.tenant_id)
                .bind(limit)
                .fetch_all(&mut *transaction)
                .await?
            }
        };
        transaction.commit().await?;
        rows.into_iter().map(agent_approval_from_row).collect()
    }

    pub async fn decide_agent_approval(
        &self,
        id: &str,
        status: &str,
        decided_at: &str,
    ) -> Result<bool, PostgresAgentError> {
        let decided_at = parse_time(decided_at)?;
        let mut transaction = self.begin().await?;
        let changed = sqlx::query(
            "UPDATE hpa_tenant.agent_approvals
             SET status = $2, decided_at = $3
             WHERE tenant_id = $1 AND id = $4 AND status = 'pending'",
        )
        .bind(&self.tenant_id)
        .bind(status)
        .bind(decided_at)
        .bind(id)
        .execute(&mut *transaction)
        .await?
        .rows_affected()
            > 0;
        transaction.commit().await?;
        Ok(changed)
    }

    pub async fn consume_agent_approval(
        &self,
        id: &str,
        used_at: &str,
    ) -> Result<bool, PostgresAgentError> {
        let used_at = parse_time(used_at)?;
        let mut transaction = self.begin().await?;
        let changed = sqlx::query(
            "UPDATE hpa_tenant.agent_approvals
             SET used_at = $2
             WHERE tenant_id = $1 AND id = $3
               AND status = 'approved' AND used_at IS NULL",
        )
        .bind(&self.tenant_id)
        .bind(used_at)
        .bind(id)
        .execute(&mut *transaction)
        .await?
        .rows_affected()
            > 0;
        transaction.commit().await?;
        Ok(changed)
    }

    pub async fn expire_agent_approvals(&self, now: &str) -> Result<u64, PostgresAgentError> {
        let now = parse_time(now)?;
        let mut transaction = self.begin().await?;
        let expired = sqlx::query(
            "UPDATE hpa_tenant.agent_approvals
             SET status = 'expired'
             WHERE tenant_id = $1 AND status = 'pending' AND expires_at < $2",
        )
        .bind(&self.tenant_id)
        .bind(now)
        .execute(&mut *transaction)
        .await?
        .rows_affected();
        transaction.commit().await?;
        Ok(expired)
    }

    pub async fn get_agent_tool_call(
        &self,
        session_id: &str,
        call_id: &str,
    ) -> Result<Option<AgentToolCallRecord>, PostgresAgentError> {
        let mut transaction = self.begin().await?;
        let row = sqlx::query(
            "SELECT session_id, call_id, idempotency_key, tool_name,
                    request_hash, status, response_json, error,
                    created_at, updated_at
             FROM hpa_tenant.agent_tool_calls
             WHERE tenant_id = $1 AND session_id = $2 AND call_id = $3",
        )
        .bind(&self.tenant_id)
        .bind(session_id)
        .bind(call_id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.map(agent_tool_call_from_row).transpose()
    }

    pub async fn begin_agent_tool_call(
        &self,
        row: &AgentToolCallRecord,
    ) -> Result<bool, PostgresAgentError> {
        let created_at = parse_time(&row.created_at)?;
        let updated_at = parse_time(&row.updated_at)?;
        let mut transaction = self.begin().await?;
        let inserted = sqlx::query(
            "INSERT INTO hpa_tenant.agent_tool_calls (
                 tenant_id, session_id, call_id, idempotency_key, tool_name,
                 request_hash, status, response_json, error, created_at, updated_at
             )
             VALUES ($1, $2, $3, $4, $5, $6, 'in_progress', NULL, NULL, $7, $8)
             ON CONFLICT DO NOTHING",
        )
        .bind(&self.tenant_id)
        .bind(&row.session_id)
        .bind(&row.call_id)
        .bind(&row.idempotency_key)
        .bind(&row.tool_name)
        .bind(&row.request_hash)
        .bind(created_at)
        .bind(updated_at)
        .execute(&mut *transaction)
        .await?
        .rows_affected()
            > 0;
        transaction.commit().await?;
        Ok(inserted)
    }

    pub async fn finish_agent_tool_call(
        &self,
        session_id: &str,
        call_id: &str,
        status: &str,
        response_json: Option<&str>,
        error: Option<&str>,
        updated_at: &str,
    ) -> Result<bool, PostgresAgentError> {
        let response = response_json.map(parse_json).transpose()?;
        let updated_at = parse_time(updated_at)?;
        let mut transaction = self.begin().await?;
        let changed = sqlx::query(
            "UPDATE hpa_tenant.agent_tool_calls
             SET status = $2, response_json = $3, error = $4, updated_at = $5
             WHERE tenant_id = $1 AND session_id = $6 AND call_id = $7
               AND status = 'in_progress'",
        )
        .bind(&self.tenant_id)
        .bind(status)
        .bind(response.as_ref().map(Json))
        .bind(error)
        .bind(updated_at)
        .bind(session_id)
        .bind(call_id)
        .execute(&mut *transaction)
        .await?
        .rows_affected()
            > 0;
        transaction.commit().await?;
        Ok(changed)
    }

    pub async fn acquire_agent_lease(
        &self,
        session_id: &str,
        owner: &str,
        ttl_ms: i64,
    ) -> Result<Option<AgentLeaseRecord>, PostgresAgentError> {
        let expires_at = OffsetDateTime::now_utc() + time::Duration::milliseconds(ttl_ms.max(1));
        let mut transaction = self.begin().await?;
        let row = sqlx::query(
            "UPDATE hpa_tenant.agent_sessions
             SET lease_owner = $3, lease_token = lease_token + 1, lease_expires_at = $4
             WHERE tenant_id = $1 AND id = $2
               AND status NOT IN ('succeeded', 'failed', 'budget_exhausted', 'timeout', 'cancelled')
               AND (
                    lease_owner IS NULL
                    OR lease_expires_at IS NULL
                    OR lease_expires_at < now()
                    OR lease_owner = $3
               )
             RETURNING lease_owner, lease_token, lease_expires_at",
        )
        .bind(&self.tenant_id)
        .bind(session_id)
        .bind(owner)
        .bind(expires_at)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.map(|row| {
            Ok(AgentLeaseRecord {
                owner: row.try_get("lease_owner")?,
                token: row.try_get("lease_token")?,
                expires_at: format_time(row.try_get("lease_expires_at")?)?,
            })
        })
        .transpose()
    }

    pub async fn renew_agent_lease(
        &self,
        session_id: &str,
        owner: &str,
        token: i64,
        ttl_ms: i64,
    ) -> Result<bool, PostgresAgentError> {
        let expires_at = OffsetDateTime::now_utc() + time::Duration::milliseconds(ttl_ms.max(1));
        let mut transaction = self.begin().await?;
        let result = sqlx::query(
            "UPDATE hpa_tenant.agent_sessions SET lease_expires_at = $4
             WHERE tenant_id = $1 AND id = $2 AND lease_owner = $3 AND lease_token = $5
               AND lease_expires_at > now()",
        )
        .bind(&self.tenant_id)
        .bind(session_id)
        .bind(owner)
        .bind(expires_at)
        .bind(token)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn release_agent_lease(
        &self,
        session_id: &str,
        owner: &str,
        token: i64,
    ) -> Result<bool, PostgresAgentError> {
        let mut transaction = self.begin().await?;
        let result = sqlx::query(
            "UPDATE hpa_tenant.agent_sessions
             SET lease_owner = NULL, lease_expires_at = NULL
             WHERE tenant_id = $1 AND id = $2 AND lease_owner = $3 AND lease_token = $4",
        )
        .bind(&self.tenant_id)
        .bind(session_id)
        .bind(owner)
        .bind(token)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn agent_lease(
        &self,
        session_id: &str,
    ) -> Result<Option<AgentLeaseRecord>, PostgresAgentError> {
        let mut transaction = self.begin().await?;
        let row = sqlx::query(
            "SELECT lease_owner, lease_token, lease_expires_at
             FROM hpa_tenant.agent_sessions
             WHERE tenant_id = $1 AND id = $2 AND lease_owner IS NOT NULL",
        )
        .bind(&self.tenant_id)
        .bind(session_id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.map(|row| {
            Ok(AgentLeaseRecord {
                owner: row.try_get("lease_owner")?,
                token: row.try_get("lease_token")?,
                expires_at: format_time(row.try_get("lease_expires_at")?)?,
            })
        })
        .transpose()
    }

    pub async fn fenced_update_agent_session(
        &self,
        row: &AgentSessionRecord,
        owner: &str,
        token: i64,
        ttl_ms: i64,
    ) -> Result<bool, PostgresAgentError> {
        let output = row.output_json.as_deref().map(parse_json).transpose()?;
        let state = parse_json(&row.state_json)?;
        let error = row.error_json.as_deref().map(parse_json).transpose()?;
        let updated_at = parse_time(&row.updated_at)?;
        let finished_at = row.finished_at.as_deref().map(parse_time).transpose()?;
        let expires_at = OffsetDateTime::now_utc() + time::Duration::milliseconds(ttl_ms.max(1));
        let mut transaction = self.begin().await?;
        let result = sqlx::query(
            "UPDATE hpa_tenant.agent_sessions
             SET status = $2, output_json = $3, state_json = $4,
                 error_json = $5, updated_at = $6, finished_at = $7, lease_expires_at = $9
             WHERE tenant_id = $1 AND id = $8
               AND lease_owner = $10 AND lease_token = $11 AND lease_expires_at > now()
               AND status NOT IN ('succeeded', 'failed', 'budget_exhausted', 'timeout', 'cancelled')",
        )
        .bind(&self.tenant_id)
        .bind(&row.status)
        .bind(output.as_ref().map(Json))
        .bind(Json(&state))
        .bind(error.as_ref().map(Json))
        .bind(updated_at)
        .bind(finished_at)
        .bind(&row.id)
        .bind(expires_at)
        .bind(owner)
        .bind(token)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn fenced_append_agent_events(
        &self,
        events: &[AgentEventRecord],
        owner: &str,
        token: i64,
        ttl_ms: i64,
    ) -> Result<bool, PostgresAgentError> {
        let Some(first) = events.first() else {
            return Ok(true);
        };
        let expires_at = OffsetDateTime::now_utc() + time::Duration::milliseconds(ttl_ms.max(1));
        let mut transaction = self.begin().await?;
        let held: Option<i32> = sqlx::query_scalar(
            "SELECT 1 FROM hpa_tenant.agent_sessions
             WHERE tenant_id = $1 AND id = $2 AND lease_owner = $3 AND lease_token = $4
               AND lease_expires_at > now()
               AND status NOT IN ('succeeded', 'failed', 'budget_exhausted', 'timeout', 'cancelled')
             FOR UPDATE",
        )
        .bind(&self.tenant_id)
        .bind(&first.session_id)
        .bind(owner)
        .bind(token)
        .fetch_optional(&mut *transaction)
        .await?;
        if held.is_none() {
            transaction.commit().await?;
            return Ok(false);
        }
        for event in events {
            let data = parse_json(&event.data_json)?;
            sqlx::query(
                "INSERT INTO hpa_tenant.agent_events
                 (tenant_id, session_id, sequence, at, kind, data_json)
                 VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT (tenant_id, session_id, sequence) DO NOTHING",
            )
            .bind(&self.tenant_id)
            .bind(&event.session_id)
            .bind(event.sequence)
            .bind(parse_time(&event.at)?)
            .bind(&event.kind)
            .bind(Json(&data))
            .execute(&mut *transaction)
            .await?;
        }
        sqlx::query(
            "UPDATE hpa_tenant.agent_sessions SET lease_expires_at = $3
             WHERE tenant_id = $1 AND id = $2",
        )
        .bind(&self.tenant_id)
        .bind(&first.session_id)
        .bind(expires_at)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(true)
    }

    async fn begin(&self) -> Result<sqlx::Transaction<'_, sqlx::Postgres>, PostgresAgentError> {
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
}

fn parse_json(value: &str) -> Result<Value, PostgresAgentError> {
    Ok(serde_json::from_str(value)?)
}

fn parse_time(value: &str) -> Result<OffsetDateTime, PostgresAgentError> {
    OffsetDateTime::parse(value, &Rfc3339).map_err(|error| {
        PostgresAgentError::Invalid(format!("invalid timestamp `{value}`: {error}"))
    })
}

fn format_time(value: OffsetDateTime) -> Result<String, PostgresAgentError> {
    value
        .format(&Rfc3339)
        .map_err(|error| PostgresAgentError::Invalid(format!("cannot format timestamp: {error}")))
}

fn json_text(value: &Value) -> Result<String, PostgresAgentError> {
    Ok(serde_json::to_string(value)?)
}

fn agent_summary_from_row(row: PgRow) -> Result<AgentSummaryRecord, PostgresAgentError> {
    let created_at: OffsetDateTime = row.try_get("created_at")?;
    let updated_at: OffsetDateTime = row.try_get("updated_at")?;
    Ok(AgentSummaryRecord {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        latest_version: row.try_get("latest_version")?,
        status: row.try_get("status")?,
        created_at: format_time(created_at)?,
        updated_at: format_time(updated_at)?,
    })
}

fn agent_version_from_row(row: PgRow) -> Result<AgentVersionRecord, PostgresAgentError> {
    let config_json: Value = row.try_get("config_json")?;
    let created_at: OffsetDateTime = row.try_get("created_at")?;
    let published_at: Option<OffsetDateTime> = row.try_get("published_at")?;
    Ok(AgentVersionRecord {
        agent_id: row.try_get("agent_id")?,
        version: row.try_get("version")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        config_json: json_text(&config_json)?,
        content_hash: row.try_get("content_hash")?,
        status: row.try_get("status")?,
        created_at: format_time(created_at)?,
        published_at: published_at.map(format_time).transpose()?,
    })
}

fn agent_session_from_row(row: PgRow) -> Result<AgentSessionRecord, PostgresAgentError> {
    let input_json: Value = row.try_get("input_json")?;
    let output_json: Option<Value> = row.try_get("output_json")?;
    let state_json: Value = row.try_get("state_json")?;
    let error_json: Option<Value> = row.try_get("error_json")?;
    let depth: i32 = row.try_get("depth")?;
    let created_at: OffsetDateTime = row.try_get("created_at")?;
    let updated_at: OffsetDateTime = row.try_get("updated_at")?;
    let finished_at: Option<OffsetDateTime> = row.try_get("finished_at")?;
    Ok(AgentSessionRecord {
        id: row.try_get("id")?,
        agent_id: row.try_get("agent_id")?,
        agent_version: row.try_get("agent_version")?,
        status: row.try_get("status")?,
        input_json: json_text(&input_json)?,
        output_json: output_json.as_ref().map(json_text).transpose()?,
        state_json: json_text(&state_json)?,
        error_json: error_json.as_ref().map(json_text).transpose()?,
        parent_session_id: row.try_get("parent_session_id")?,
        depth: i64::from(depth),
        created_at: format_time(created_at)?,
        updated_at: format_time(updated_at)?,
        finished_at: finished_at.map(format_time).transpose()?,
    })
}

fn agent_event_from_row(row: PgRow) -> Result<AgentEventRecord, PostgresAgentError> {
    let data_json: Value = row.try_get("data_json")?;
    let at: OffsetDateTime = row.try_get("at")?;
    Ok(AgentEventRecord {
        session_id: row.try_get("session_id")?,
        sequence: row.try_get("sequence")?,
        at: format_time(at)?,
        kind: row.try_get("kind")?,
        data_json: json_text(&data_json)?,
    })
}

fn agent_approval_from_row(row: PgRow) -> Result<AgentApprovalRecord, PostgresAgentError> {
    let args_json: Value = row.try_get("args_json")?;
    let created_at: OffsetDateTime = row.try_get("created_at")?;
    let expires_at: OffsetDateTime = row.try_get("expires_at")?;
    let decided_at: Option<OffsetDateTime> = row.try_get("decided_at")?;
    let used_at: Option<OffsetDateTime> = row.try_get("used_at")?;
    Ok(AgentApprovalRecord {
        id: row.try_get("id")?,
        session_id: row.try_get("session_id")?,
        tool_name: row.try_get("tool_name")?,
        args_json: json_text(&args_json)?,
        risk: row.try_get("risk")?,
        status: row.try_get("status")?,
        created_at: format_time(created_at)?,
        expires_at: format_time(expires_at)?,
        decided_at: decided_at.map(format_time).transpose()?,
        used_at: used_at.map(format_time).transpose()?,
    })
}

fn agent_tool_call_from_row(row: PgRow) -> Result<AgentToolCallRecord, PostgresAgentError> {
    let response_json: Option<Value> = row.try_get("response_json")?;
    let created_at: OffsetDateTime = row.try_get("created_at")?;
    let updated_at: OffsetDateTime = row.try_get("updated_at")?;
    Ok(AgentToolCallRecord {
        session_id: row.try_get("session_id")?,
        call_id: row.try_get("call_id")?,
        idempotency_key: row.try_get("idempotency_key")?,
        tool_name: row.try_get("tool_name")?,
        request_hash: row.try_get("request_hash")?,
        status: row.try_get("status")?,
        response_json: response_json.as_ref().map(json_text).transpose()?,
        error: row.try_get("error")?,
        created_at: format_time(created_at)?,
        updated_at: format_time(updated_at)?,
    })
}
