use std::path::Path;

use hpa_core::runtime::{ExecutionEvent, ExecutionState, ExecutionStatus};
use hpa_local::postgres_agent::{
    AgentApprovalRecord, AgentEventRecord, AgentSessionRecord, AgentSummaryRecord,
    AgentToolCallRecord, AgentVersionRecord, PostgresAgentError, PostgresAgentStore,
};
use hpa_local::postgres_connector::PostgresConnectorStore;
use hpa_local::postgres_execution::{
    ArtifactRecord, ExecutionLeaseRecord, ExecutionRecord, PostgresExecutionError,
    PostgresExecutionStore, WorkflowRecord, WorkflowVersionRecord,
};
use hpa_local::postgres_mcp_meta::{
    McpApprovalRecord, McpAuditRecord, McpServerRecord, McpTokenRecord, PostgresMcpMetaError,
    PostgresMcpMetaStore,
};
use hpa_local::postgres_plugin::PostgresPluginStore;
use hpa_local::postgres_skill::{
    PostgresSkillError, PostgresSkillStore, SkillArtifactRecord, SkillFileRecord, SkillRunRecord,
    SkillSummaryRecord, SkillVersionRecord,
};
use hpa_local::postgres_trigger::{
    PostgresTriggerError, PostgresTriggerStore, TriggerFireRecord, TriggerRecord, TriggerSeedRecord,
};
use hpa_skill::{RunOutcome, SkillFile, SkillReport, SkillSnapshot};
use serde::Serialize;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{FromRow, SqlitePool};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DbError {
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("version `{version}` of workflow `{workflow_id}` is frozen")]
    VersionFrozen {
        workflow_id: String,
        version: String,
    },
    #[error("skill version `{skill_id}@{version}` is frozen")]
    SkillVersionFrozen { skill_id: String, version: String },
    #[error("agent version `{agent_id}@{version}` is frozen")]
    AgentVersionFrozen { agent_id: String, version: String },
    #[error("PostgreSQL execution error: {0}")]
    PostgresExecution(String),
    #[error("PostgreSQL trigger error: {0}")]
    PostgresTrigger(String),
    #[error("PostgreSQL agent error: {0}")]
    PostgresAgent(String),
    #[error("PostgreSQL skill error: {0}")]
    PostgresSkill(String),
    #[error("PostgreSQL MCP metadata error: {0}")]
    PostgresMcpMeta(String),
}

pub type DbResult<T> = Result<T, DbError>;

#[derive(Clone)]
pub struct Db {
    pub pool: SqlitePool,
    postgres_execution: Option<PostgresExecutionStore>,
    postgres_trigger: Option<PostgresTriggerStore>,
    postgres_agent: Option<PostgresAgentStore>,
    postgres_skill: Option<PostgresSkillStore>,
    postgres_mcp_meta: Option<PostgresMcpMetaStore>,
    postgres_connector: Option<PostgresConnectorStore>,
    postgres_plugin: Option<PostgresPluginStore>,
}

#[derive(Debug, Clone, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRow {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub published: i64,
    pub ir_json: String,
    pub yaml: String,
    pub created_at: String,
    pub updated_at: String,
}

impl From<WorkflowRecord> for WorkflowRow {
    fn from(record: WorkflowRecord) -> Self {
        Self {
            id: record.id,
            name: record.name,
            version: record.version,
            description: record.description,
            published: record.published,
            ir_json: record.ir_json,
            yaml: record.yaml,
            created_at: record.created_at,
            updated_at: record.updated_at,
        }
    }
}

impl From<&WorkflowRow> for WorkflowRecord {
    fn from(row: &WorkflowRow) -> Self {
        Self {
            id: row.id.clone(),
            name: row.name.clone(),
            version: row.version.clone(),
            description: row.description.clone(),
            published: row.published,
            ir_json: row.ir_json.clone(),
            yaml: row.yaml.clone(),
            created_at: row.created_at.clone(),
            updated_at: row.updated_at.clone(),
        }
    }
}

#[derive(Debug, Clone, FromRow)]
pub struct WorkflowVersionRow {
    pub version: String,
    pub registry_version: String,
    pub ir_json: String,
    pub created_at: String,
}

impl From<WorkflowVersionRecord> for WorkflowVersionRow {
    fn from(record: WorkflowVersionRecord) -> Self {
        Self {
            version: record.version,
            registry_version: record.registry_version,
            ir_json: record.ir_json,
            created_at: record.created_at,
        }
    }
}

#[derive(Debug, Clone, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSummaryRow {
    pub id: String,
    pub name: String,
    pub latest_version: String,
    pub license: Option<String>,
    pub status: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct SkillVersionRow {
    pub skill_id: String,
    pub version: String,
    pub name: String,
    pub license: Option<String>,
    pub manifest_json: String,
    pub skill_md: String,
    pub report_json: String,
    pub content_hash: String,
    pub status: String,
    pub source_path: Option<String>,
    pub imported_at: String,
    pub published_at: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
pub struct SkillFileRow {
    pub path: String,
    pub sha256: String,
    pub size: i64,
    pub content: Vec<u8>,
}

#[derive(Debug, Clone, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillRunRow {
    pub id: String,
    pub skill_id: String,
    pub version: String,
    pub script_id: String,
    pub status: String,
    pub input_json: String,
    pub output_json: Option<String>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub exit_code: Option<i64>,
    pub error_json: Option<String>,
    pub sandbox: String,
    pub duration_ms: i64,
    pub started_at: String,
    pub finished_at: String,
}

#[derive(Debug, Clone, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillArtifactRow {
    pub id: String,
    pub run_id: String,
    pub name: String,
    pub sha256: String,
    pub size: i64,
    pub mime_type: String,
    #[serde(skip_serializing)]
    pub stored_path: String,
    pub created_at: String,
}

impl From<SkillSummaryRecord> for SkillSummaryRow {
    fn from(record: SkillSummaryRecord) -> Self {
        Self {
            id: record.id,
            name: record.name,
            latest_version: record.latest_version,
            license: record.license,
            status: record.status,
            updated_at: record.updated_at,
        }
    }
}

impl From<SkillVersionRecord> for SkillVersionRow {
    fn from(record: SkillVersionRecord) -> Self {
        Self {
            skill_id: record.skill_id,
            version: record.version,
            name: record.name,
            license: record.license,
            manifest_json: record.manifest_json,
            skill_md: record.skill_md,
            report_json: record.report_json,
            content_hash: record.content_hash,
            status: record.status,
            source_path: record.source_path,
            imported_at: record.imported_at,
            published_at: record.published_at,
        }
    }
}

impl From<SkillFileRecord> for SkillFileRow {
    fn from(record: SkillFileRecord) -> Self {
        Self {
            path: record.path,
            sha256: record.sha256,
            size: record.size,
            content: record.content,
        }
    }
}

impl From<SkillRunRecord> for SkillRunRow {
    fn from(record: SkillRunRecord) -> Self {
        Self {
            id: record.id,
            skill_id: record.skill_id,
            version: record.version,
            script_id: record.script_id,
            status: record.status,
            input_json: record.input_json,
            output_json: record.output_json,
            stdout: record.stdout,
            stderr: record.stderr,
            exit_code: record.exit_code,
            error_json: record.error_json,
            sandbox: record.sandbox,
            duration_ms: record.duration_ms,
            started_at: record.started_at,
            finished_at: record.finished_at,
        }
    }
}

impl From<SkillArtifactRecord> for SkillArtifactRow {
    fn from(record: SkillArtifactRecord) -> Self {
        Self {
            id: record.id,
            run_id: record.run_id,
            name: record.name,
            sha256: record.sha256,
            size: record.size,
            mime_type: record.mime_type,
            stored_path: record.stored_path,
            created_at: record.created_at,
        }
    }
}

#[derive(Debug, Clone, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpTokenRow {
    pub id: String,
    pub name: String,
    pub scopes_json: String,
    pub tenant_id: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub revoked_at: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
pub struct McpApprovalRow {
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

#[derive(Debug, Clone, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpAuditRow {
    pub id: String,
    pub at: String,
    pub token_id: Option<String>,
    pub method: String,
    pub tool_name: Option<String>,
    pub status: String,
    pub latency_ms: i64,
    pub detail_json: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
pub struct McpServerRow {
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

impl From<McpTokenRecord> for McpTokenRow {
    fn from(record: McpTokenRecord) -> Self {
        Self {
            id: record.id,
            name: record.name,
            scopes_json: record.scopes_json,
            tenant_id: record.tenant_id,
            created_at: record.created_at,
            last_used_at: record.last_used_at,
            revoked_at: record.revoked_at,
        }
    }
}

impl From<McpApprovalRecord> for McpApprovalRow {
    fn from(record: McpApprovalRecord) -> Self {
        Self {
            id: record.id,
            token_id: record.token_id,
            tool_name: record.tool_name,
            arguments_json: record.arguments_json,
            arguments_hash: record.arguments_hash,
            request_state: record.request_state,
            risk: record.risk,
            status: record.status,
            created_at: record.created_at,
            expires_at: record.expires_at,
            decided_at: record.decided_at,
            used_at: record.used_at,
        }
    }
}

impl From<&McpAuditRow> for McpAuditRecord {
    fn from(row: &McpAuditRow) -> Self {
        Self {
            id: row.id.clone(),
            at: row.at.clone(),
            token_id: row.token_id.clone(),
            method: row.method.clone(),
            tool_name: row.tool_name.clone(),
            status: row.status.clone(),
            latency_ms: row.latency_ms,
            detail_json: row.detail_json.clone(),
        }
    }
}

impl From<McpAuditRecord> for McpAuditRow {
    fn from(record: McpAuditRecord) -> Self {
        Self {
            id: record.id,
            at: record.at,
            token_id: record.token_id,
            method: record.method,
            tool_name: record.tool_name,
            status: record.status,
            latency_ms: record.latency_ms,
            detail_json: record.detail_json,
        }
    }
}

impl From<McpServerRow> for McpServerRecord {
    fn from(row: McpServerRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            transport: row.transport,
            command_json: row.command_json,
            url: row.url,
            allowed_tools_json: row.allowed_tools_json,
            network_json: row.network_json,
            credential_env: row.credential_env,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

impl From<McpServerRecord> for McpServerRow {
    fn from(record: McpServerRecord) -> Self {
        Self {
            id: record.id,
            name: record.name,
            transport: record.transport,
            command_json: record.command_json,
            url: record.url,
            allowed_tools_json: record.allowed_tools_json,
            network_json: record.network_json,
            credential_env: record.credential_env,
            created_at: record.created_at,
            updated_at: record.updated_at,
        }
    }
}

#[derive(Debug, Clone, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSummaryRow {
    pub id: String,
    pub name: String,
    pub latest_version: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct AgentVersionRow {
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

#[derive(Debug, Clone, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionRow {
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

#[derive(Debug, Clone, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEventRow {
    pub session_id: String,
    pub sequence: i64,
    pub at: String,
    pub kind: String,
    pub data_json: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct AgentApprovalRow {
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

#[derive(Debug, Clone, FromRow)]
pub struct AgentToolCallRow {
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

impl From<AgentSummaryRecord> for AgentSummaryRow {
    fn from(record: AgentSummaryRecord) -> Self {
        Self {
            id: record.id,
            name: record.name,
            latest_version: record.latest_version,
            status: record.status,
            created_at: record.created_at,
            updated_at: record.updated_at,
        }
    }
}

impl From<&AgentVersionRow> for AgentVersionRecord {
    fn from(row: &AgentVersionRow) -> Self {
        Self {
            agent_id: row.agent_id.clone(),
            version: row.version.clone(),
            name: row.name.clone(),
            description: row.description.clone(),
            config_json: row.config_json.clone(),
            content_hash: row.content_hash.clone(),
            status: row.status.clone(),
            created_at: row.created_at.clone(),
            published_at: row.published_at.clone(),
        }
    }
}

impl From<AgentVersionRecord> for AgentVersionRow {
    fn from(record: AgentVersionRecord) -> Self {
        Self {
            agent_id: record.agent_id,
            version: record.version,
            name: record.name,
            description: record.description,
            config_json: record.config_json,
            content_hash: record.content_hash,
            status: record.status,
            created_at: record.created_at,
            published_at: record.published_at,
        }
    }
}

impl From<&AgentSessionRow> for AgentSessionRecord {
    fn from(row: &AgentSessionRow) -> Self {
        Self {
            id: row.id.clone(),
            agent_id: row.agent_id.clone(),
            agent_version: row.agent_version.clone(),
            status: row.status.clone(),
            input_json: row.input_json.clone(),
            output_json: row.output_json.clone(),
            state_json: row.state_json.clone(),
            error_json: row.error_json.clone(),
            parent_session_id: row.parent_session_id.clone(),
            depth: row.depth,
            created_at: row.created_at.clone(),
            updated_at: row.updated_at.clone(),
            finished_at: row.finished_at.clone(),
        }
    }
}

impl From<AgentSessionRecord> for AgentSessionRow {
    fn from(record: AgentSessionRecord) -> Self {
        Self {
            id: record.id,
            agent_id: record.agent_id,
            agent_version: record.agent_version,
            status: record.status,
            input_json: record.input_json,
            output_json: record.output_json,
            state_json: record.state_json,
            error_json: record.error_json,
            parent_session_id: record.parent_session_id,
            depth: record.depth,
            created_at: record.created_at,
            updated_at: record.updated_at,
            finished_at: record.finished_at,
        }
    }
}

impl From<&AgentEventRow> for AgentEventRecord {
    fn from(row: &AgentEventRow) -> Self {
        Self {
            session_id: row.session_id.clone(),
            sequence: row.sequence,
            at: row.at.clone(),
            kind: row.kind.clone(),
            data_json: row.data_json.clone(),
        }
    }
}

impl From<AgentEventRecord> for AgentEventRow {
    fn from(record: AgentEventRecord) -> Self {
        Self {
            session_id: record.session_id,
            sequence: record.sequence,
            at: record.at,
            kind: record.kind,
            data_json: record.data_json,
        }
    }
}

impl From<AgentApprovalRecord> for AgentApprovalRow {
    fn from(record: AgentApprovalRecord) -> Self {
        Self {
            id: record.id,
            session_id: record.session_id,
            tool_name: record.tool_name,
            args_json: record.args_json,
            risk: record.risk,
            status: record.status,
            created_at: record.created_at,
            expires_at: record.expires_at,
            decided_at: record.decided_at,
            used_at: record.used_at,
        }
    }
}

impl From<AgentToolCallRow> for AgentToolCallRecord {
    fn from(row: AgentToolCallRow) -> Self {
        Self {
            session_id: row.session_id,
            call_id: row.call_id,
            idempotency_key: row.idempotency_key,
            tool_name: row.tool_name,
            request_hash: row.request_hash,
            status: row.status,
            response_json: row.response_json,
            error: row.error,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

impl From<AgentToolCallRecord> for AgentToolCallRow {
    fn from(record: AgentToolCallRecord) -> Self {
        Self {
            session_id: record.session_id,
            call_id: record.call_id,
            idempotency_key: record.idempotency_key,
            tool_name: record.tool_name,
            request_hash: record.request_hash,
            status: record.status,
            response_json: record.response_json,
            error: record.error,
            created_at: record.created_at,
            updated_at: record.updated_at,
        }
    }
}

#[derive(Debug, Clone, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceEventRow {
    pub sequence: i64,
    pub id: String,
    pub at: String,
    pub plugin_id: Option<String>,
    pub version: Option<String>,
    pub kind: String,
    pub detail_json: String,
}

#[derive(Debug, Clone, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionRow {
    pub id: String,
    pub workflow_id: String,
    pub workflow_version: String,
    pub status: String,
    pub state_json: String,
    pub workflow_json: String,
    pub output_json: Option<String>,
    pub error_json: Option<String>,
    pub started_at: String,
    pub updated_at: String,
}

impl From<ExecutionRecord> for ExecutionRow {
    fn from(record: ExecutionRecord) -> Self {
        Self {
            id: record.id,
            workflow_id: record.workflow_id,
            workflow_version: record.workflow_version,
            status: record.status,
            state_json: record.state_json,
            workflow_json: record.workflow_json,
            output_json: record.output_json,
            error_json: record.error_json,
            started_at: record.started_at,
            updated_at: record.updated_at,
        }
    }
}

impl From<&ExecutionRow> for ExecutionRecord {
    fn from(row: &ExecutionRow) -> Self {
        Self {
            id: row.id.clone(),
            workflow_id: row.workflow_id.clone(),
            workflow_version: row.workflow_version.clone(),
            status: row.status.clone(),
            state_json: row.state_json.clone(),
            workflow_json: row.workflow_json.clone(),
            output_json: row.output_json.clone(),
            error_json: row.error_json.clone(),
            started_at: row.started_at.clone(),
            updated_at: row.updated_at.clone(),
        }
    }
}

#[derive(Debug, Clone, FromRow)]
pub struct EventRow {
    pub payload_json: String,
}

#[derive(Debug, Clone, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRow {
    pub id: String,
    pub execution_id: String,
    pub name: String,
    pub kind: String,
    pub mime_type: String,
    pub size: i64,
    pub content_json: String,
    pub created_at: String,
}

impl From<ArtifactRecord> for ArtifactRow {
    fn from(record: ArtifactRecord) -> Self {
        Self {
            id: record.id,
            execution_id: record.execution_id,
            name: record.name,
            kind: record.kind,
            mime_type: record.mime_type,
            size: record.size,
            content_json: record.content_json,
            created_at: record.created_at,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub workflows: i64,
    pub executions: i64,
    pub running: i64,
    pub waiting_approval: i64,
    pub succeeded: i64,
    pub failed: i64,
}

#[derive(Debug, Clone, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TriggerStateRow {
    pub workflow_id: String,
    pub trigger_id: String,
    pub kind: String,
    pub config_json: String,
    pub workflow_version: Option<String>,
    pub enabled: i64,
    pub next_fire_at: Option<i64>,
    pub last_fire_at: Option<i64>,
    pub last_execution_id: Option<String>,
    pub last_status: Option<String>,
    pub updated_at: String,
}

impl From<TriggerRecord> for TriggerStateRow {
    fn from(record: TriggerRecord) -> Self {
        Self {
            workflow_id: record.workflow_id,
            trigger_id: record.trigger_id,
            kind: record.kind,
            config_json: record.config_json,
            workflow_version: record.workflow_version,
            enabled: record.enabled,
            next_fire_at: record.next_fire_at,
            last_fire_at: record.last_fire_at,
            last_execution_id: record.last_execution_id,
            last_status: record.last_status,
            updated_at: record.updated_at,
        }
    }
}

#[derive(Debug, Clone, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TriggerFireRow {
    pub id: String,
    pub workflow_id: String,
    pub trigger_id: String,
    pub kind: String,
    pub fired_at: String,
    pub scheduled_at: Option<String>,
    pub execution_id: Option<String>,
    pub status: String,
    pub event_id: Option<String>,
    pub detail_json: Option<String>,
}

impl From<TriggerFireRecord> for TriggerFireRow {
    fn from(record: TriggerFireRecord) -> Self {
        Self {
            id: record.id,
            workflow_id: record.workflow_id,
            trigger_id: record.trigger_id,
            kind: record.kind,
            fired_at: record.fired_at,
            scheduled_at: record.scheduled_at,
            execution_id: record.execution_id,
            status: record.status,
            event_id: record.event_id,
            detail_json: record.detail_json,
        }
    }
}

impl From<&TriggerFireRow> for TriggerFireRecord {
    fn from(row: &TriggerFireRow) -> Self {
        Self {
            id: row.id.clone(),
            workflow_id: row.workflow_id.clone(),
            trigger_id: row.trigger_id.clone(),
            kind: row.kind.clone(),
            fired_at: row.fired_at.clone(),
            scheduled_at: row.scheduled_at.clone(),
            execution_id: row.execution_id.clone(),
            status: row.status.clone(),
            event_id: row.event_id.clone(),
            detail_json: row.detail_json.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TriggerSeed {
    pub trigger_id: String,
    pub kind: String,
    pub config_json: String,
    pub enabled: bool,
    pub next_fire_at: Option<i64>,
}

const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS workflows (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        version TEXT NOT NULL,
        description TEXT,
        published INTEGER NOT NULL DEFAULT 0,
        ir_json TEXT NOT NULL,
        yaml TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS workflow_versions (
        workflow_id TEXT NOT NULL,
        version TEXT NOT NULL,
        registry_version TEXT NOT NULL DEFAULT '',
        ir_json TEXT NOT NULL,
        created_at TEXT NOT NULL,
        PRIMARY KEY (workflow_id, version)
    )",
    "CREATE TABLE IF NOT EXISTS executions (
        id TEXT PRIMARY KEY,
        workflow_id TEXT NOT NULL,
        workflow_version TEXT NOT NULL,
        status TEXT NOT NULL,
        state_json TEXT NOT NULL,
        workflow_json TEXT NOT NULL,
        output_json TEXT,
        error_json TEXT,
        started_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS idx_executions_workflow ON executions(workflow_id, started_at DESC)",
    "CREATE TABLE IF NOT EXISTS remote_executions (
        execution_id TEXT PRIMARY KEY,
        worker_id TEXT NOT NULL,
        created_at TEXT NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS events (
        execution_id TEXT NOT NULL,
        sequence INTEGER NOT NULL,
        payload_json TEXT NOT NULL,
        PRIMARY KEY (execution_id, sequence)
    )",
    "CREATE TABLE IF NOT EXISTS artifacts (
        id TEXT PRIMARY KEY,
        execution_id TEXT NOT NULL,
        name TEXT NOT NULL,
        kind TEXT NOT NULL,
        mime_type TEXT NOT NULL,
        size INTEGER NOT NULL,
        content_json TEXT NOT NULL,
        created_at TEXT NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS idx_artifacts_execution ON artifacts(execution_id)",
    "CREATE TABLE IF NOT EXISTS skills (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        latest_version TEXT NOT NULL,
        license TEXT,
        status TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS skill_versions (
        skill_id TEXT NOT NULL,
        version TEXT NOT NULL,
        name TEXT NOT NULL,
        license TEXT,
        manifest_json TEXT NOT NULL,
        skill_md TEXT NOT NULL,
        report_json TEXT NOT NULL,
        content_hash TEXT NOT NULL,
        status TEXT NOT NULL,
        source_path TEXT,
        imported_at TEXT NOT NULL,
        published_at TEXT,
        PRIMARY KEY (skill_id, version)
    )",
    "CREATE INDEX IF NOT EXISTS idx_skill_versions_imported ON skill_versions(imported_at DESC)",
    "CREATE TABLE IF NOT EXISTS skill_files (
        skill_id TEXT NOT NULL,
        version TEXT NOT NULL,
        path TEXT NOT NULL,
        sha256 TEXT NOT NULL,
        size INTEGER NOT NULL,
        content BLOB NOT NULL,
        PRIMARY KEY (skill_id, version, path),
        FOREIGN KEY (skill_id, version) REFERENCES skill_versions(skill_id, version) ON DELETE CASCADE
    )",
    "CREATE TABLE IF NOT EXISTS skill_runs (
        id TEXT PRIMARY KEY,
        skill_id TEXT NOT NULL,
        version TEXT NOT NULL,
        script_id TEXT NOT NULL,
        status TEXT NOT NULL,
        input_json TEXT NOT NULL,
        output_json TEXT,
        stdout TEXT,
        stderr TEXT,
        exit_code INTEGER,
        error_json TEXT,
        sandbox TEXT NOT NULL,
        duration_ms INTEGER NOT NULL,
        started_at TEXT NOT NULL,
        finished_at TEXT NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS idx_skill_runs_skill ON skill_runs(skill_id, started_at DESC)",
    "CREATE TABLE IF NOT EXISTS skill_run_artifacts (
        id TEXT PRIMARY KEY,
        run_id TEXT NOT NULL,
        name TEXT NOT NULL,
        sha256 TEXT NOT NULL,
        size INTEGER NOT NULL,
        mime_type TEXT NOT NULL,
        stored_path TEXT NOT NULL,
        created_at TEXT NOT NULL,
        FOREIGN KEY (run_id) REFERENCES skill_runs(id) ON DELETE CASCADE
    )",
    "CREATE INDEX IF NOT EXISTS idx_skill_run_artifacts ON skill_run_artifacts(run_id)",
    "CREATE TABLE IF NOT EXISTS mcp_tokens (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        token_hash TEXT NOT NULL UNIQUE,
        scopes_json TEXT NOT NULL,
        tenant_id TEXT NOT NULL,
        created_at TEXT NOT NULL,
        last_used_at TEXT,
        revoked_at TEXT
    )",
    "CREATE TABLE IF NOT EXISTS mcp_approvals (
        id TEXT PRIMARY KEY,
        token_id TEXT NOT NULL,
        tool_name TEXT NOT NULL,
        arguments_json TEXT NOT NULL,
        arguments_hash TEXT NOT NULL,
        request_state TEXT NOT NULL UNIQUE,
        risk TEXT NOT NULL,
        status TEXT NOT NULL,
        created_at TEXT NOT NULL,
        expires_at TEXT NOT NULL,
        decided_at TEXT,
        used_at TEXT
    )",
    "CREATE INDEX IF NOT EXISTS idx_mcp_approvals_status ON mcp_approvals(status, created_at DESC)",
    "CREATE TABLE IF NOT EXISTS mcp_audit (
        id TEXT PRIMARY KEY,
        at TEXT NOT NULL,
        token_id TEXT,
        method TEXT NOT NULL,
        tool_name TEXT,
        status TEXT NOT NULL,
        latency_ms INTEGER NOT NULL,
        detail_json TEXT
    )",
    "CREATE INDEX IF NOT EXISTS idx_mcp_audit_at ON mcp_audit(at DESC)",
    "CREATE TABLE IF NOT EXISTS mcp_servers (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        transport TEXT NOT NULL,
        command_json TEXT NOT NULL,
        url TEXT,
        allowed_tools_json TEXT NOT NULL,
        network_json TEXT NOT NULL,
        credential_env TEXT,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS agents (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        latest_version TEXT NOT NULL,
        status TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS agent_versions (
        agent_id TEXT NOT NULL,
        version TEXT NOT NULL,
        name TEXT NOT NULL,
        description TEXT,
        config_json TEXT NOT NULL,
        content_hash TEXT NOT NULL,
        status TEXT NOT NULL,
        created_at TEXT NOT NULL,
        published_at TEXT,
        PRIMARY KEY (agent_id, version)
    )",
    "CREATE TABLE IF NOT EXISTS agent_sessions (
        id TEXT PRIMARY KEY,
        agent_id TEXT NOT NULL,
        agent_version TEXT NOT NULL,
        status TEXT NOT NULL,
        input_json TEXT NOT NULL,
        output_json TEXT,
        state_json TEXT NOT NULL,
        error_json TEXT,
        parent_session_id TEXT,
        depth INTEGER NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        finished_at TEXT
    )",
    "CREATE INDEX IF NOT EXISTS idx_agent_sessions_agent ON agent_sessions(agent_id, created_at DESC)",
    "CREATE TABLE IF NOT EXISTS agent_events (
        session_id TEXT NOT NULL,
        sequence INTEGER NOT NULL,
        at TEXT NOT NULL,
        kind TEXT NOT NULL,
        data_json TEXT NOT NULL,
        PRIMARY KEY (session_id, sequence)
    )",
    "CREATE TABLE IF NOT EXISTS agent_approvals (
        id TEXT PRIMARY KEY,
        session_id TEXT NOT NULL,
        tool_name TEXT NOT NULL,
        args_json TEXT NOT NULL,
        risk TEXT NOT NULL,
        status TEXT NOT NULL,
        created_at TEXT NOT NULL,
        expires_at TEXT NOT NULL,
        decided_at TEXT,
        used_at TEXT
    )",
    "CREATE INDEX IF NOT EXISTS idx_agent_approvals_status ON agent_approvals(status, created_at DESC)",
    "CREATE TABLE IF NOT EXISTS agent_tool_calls (
        session_id TEXT NOT NULL,
        call_id TEXT NOT NULL,
        idempotency_key TEXT NOT NULL,
        tool_name TEXT NOT NULL,
        request_hash TEXT NOT NULL,
        status TEXT NOT NULL,
        response_json TEXT,
        error TEXT,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        PRIMARY KEY (session_id, call_id),
        UNIQUE (session_id, idempotency_key),
        FOREIGN KEY (session_id) REFERENCES agent_sessions(id) ON DELETE CASCADE
    )",
    "CREATE INDEX IF NOT EXISTS idx_agent_tool_calls_status
        ON agent_tool_calls(status, updated_at)",
    "CREATE TABLE IF NOT EXISTS plugin_trusted_keys (
        key_id TEXT PRIMARY KEY,
        public_key TEXT NOT NULL,
        label TEXT NOT NULL,
        trust TEXT NOT NULL,
        created_at TEXT NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS plugin_publishers (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        status TEXT NOT NULL CHECK (status IN ('active', 'suspended')),
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS plugin_publisher_keys (
        publisher_id TEXT NOT NULL,
        key_id TEXT NOT NULL,
        public_key TEXT NOT NULL,
        label TEXT NOT NULL,
        valid_from TEXT,
        valid_until TEXT,
        revoked_at TEXT,
        created_at TEXT NOT NULL,
        PRIMARY KEY (publisher_id, key_id),
        FOREIGN KEY (publisher_id) REFERENCES plugin_publishers(id) ON DELETE CASCADE
    )",
    "CREATE TABLE IF NOT EXISTS marketplace_sources (
        id TEXT PRIMARY KEY,
        label TEXT NOT NULL,
        index_uri TEXT NOT NULL,
        enabled INTEGER NOT NULL DEFAULT 1,
        last_sequence INTEGER NOT NULL DEFAULT 0,
        last_refresh_at TEXT,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS marketplace_source_keys (
        source_id TEXT NOT NULL,
        key_id TEXT NOT NULL,
        public_key TEXT NOT NULL,
        label TEXT NOT NULL,
        valid_from TEXT,
        valid_until TEXT,
        revoked_at TEXT,
        created_at TEXT NOT NULL,
        PRIMARY KEY (source_id, key_id),
        FOREIGN KEY (source_id) REFERENCES marketplace_sources(id) ON DELETE CASCADE
    )",
    "CREATE TABLE IF NOT EXISTS marketplace_remote_packages (
        source_id TEXT NOT NULL,
        plugin_id TEXT NOT NULL,
        version TEXT NOT NULL,
        publisher_id TEXT NOT NULL,
        signer_key_id TEXT NOT NULL,
        package_sha256 TEXT NOT NULL,
        package_path TEXT NOT NULL,
        review_status TEXT NOT NULL,
        index_sequence INTEGER NOT NULL,
        published_at TEXT,
        withdrawn_at TEXT,
        revocation_reason TEXT,
        provenance_json TEXT,
        oci_json TEXT,
        PRIMARY KEY (source_id, plugin_id, version),
        FOREIGN KEY (source_id) REFERENCES marketplace_sources(id) ON DELETE CASCADE
    )",
    "CREATE TABLE IF NOT EXISTS plugin_packages (
        plugin_id TEXT NOT NULL,
        version TEXT NOT NULL,
        name TEXT NOT NULL,
        description TEXT,
        license TEXT NOT NULL,
        publisher TEXT,
        manifest_json TEXT NOT NULL,
        content_hash TEXT NOT NULL,
        package_path TEXT NOT NULL,
        signature_json TEXT,
        signer_key_id TEXT,
        signer_public_key TEXT,
        trust TEXT NOT NULL,
        source TEXT NOT NULL,
        capabilities_json TEXT NOT NULL,
        sbom_json TEXT NOT NULL,
        created_at TEXT NOT NULL,
        PRIMARY KEY (plugin_id, version)
    )",
    "CREATE TABLE IF NOT EXISTS plugins (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        latest_version TEXT NOT NULL,
        status TEXT NOT NULL,
        installed_version TEXT,
        previous_version TEXT,
        trust TEXT NOT NULL,
        license TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS plugin_capabilities (
        plugin_id TEXT NOT NULL,
        version TEXT NOT NULL,
        kind TEXT NOT NULL,
        capability_id TEXT NOT NULL,
        capability_version TEXT NOT NULL,
        path TEXT NOT NULL,
        status TEXT NOT NULL,
        config_json TEXT NOT NULL,
        permissions_json TEXT NOT NULL,
        PRIMARY KEY (plugin_id, version, kind, capability_id)
    )",
    "CREATE TABLE IF NOT EXISTS plugin_market_listings (
        plugin_id TEXT NOT NULL,
        version TEXT NOT NULL,
        review_status TEXT NOT NULL,
        review_note TEXT,
        submitted_at TEXT NOT NULL,
        reviewed_at TEXT,
        withdrawn_at TEXT,
        downloads INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (plugin_id, version)
    )",
    "CREATE TABLE IF NOT EXISTS plugin_reviews (
        id TEXT PRIMARY KEY,
        plugin_id TEXT NOT NULL,
        version TEXT NOT NULL,
        rating INTEGER NOT NULL,
        comment TEXT,
        created_at TEXT NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS plugin_events (
        id TEXT PRIMARY KEY,
        at TEXT NOT NULL,
        plugin_id TEXT,
        version TEXT,
        kind TEXT NOT NULL,
        detail_json TEXT NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS idx_plugin_events_at ON plugin_events(at DESC)",
    "CREATE TABLE IF NOT EXISTS marketplace_event_stream (
        sequence INTEGER PRIMARY KEY AUTOINCREMENT,
        id TEXT NOT NULL UNIQUE,
        at TEXT NOT NULL,
        plugin_id TEXT,
        version TEXT,
        kind TEXT NOT NULL,
        detail_json TEXT NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS idx_marketplace_event_stream_at
        ON marketplace_event_stream(at)",
    "CREATE TABLE IF NOT EXISTS connector_definitions (
        connector_id TEXT NOT NULL,
        version TEXT NOT NULL,
        name TEXT NOT NULL,
        kind TEXT NOT NULL,
        manifest_json TEXT NOT NULL,
        plugin_id TEXT,
        plugin_version TEXT,
        status TEXT NOT NULL,
        created_at TEXT NOT NULL,
        PRIMARY KEY (connector_id, version)
    )",
    "CREATE TABLE IF NOT EXISTS connections (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        connector_id TEXT NOT NULL,
        connector_version TEXT NOT NULL,
        config_json TEXT NOT NULL,
        credential_env TEXT,
        status TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS connector_audit (
        id TEXT PRIMARY KEY,
        at TEXT NOT NULL,
        connection_id TEXT NOT NULL,
        operation TEXT NOT NULL,
        status TEXT NOT NULL,
        latency_ms INTEGER NOT NULL,
        detail_json TEXT NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS idx_connector_audit_at ON connector_audit(at DESC)",
    "CREATE TABLE IF NOT EXISTS trigger_state (
        workflow_id TEXT NOT NULL,
        trigger_id TEXT NOT NULL,
        kind TEXT NOT NULL,
        config_json TEXT NOT NULL,
        workflow_version TEXT,
        enabled INTEGER NOT NULL,
        next_fire_at INTEGER,
        last_fire_at INTEGER,
        last_execution_id TEXT,
        last_status TEXT,
        updated_at TEXT NOT NULL,
        PRIMARY KEY (workflow_id, trigger_id)
    )",
    "CREATE INDEX IF NOT EXISTS idx_trigger_state_due ON trigger_state(enabled, next_fire_at)",
    "CREATE TABLE IF NOT EXISTS trigger_fires (
        id TEXT PRIMARY KEY,
        workflow_id TEXT NOT NULL,
        trigger_id TEXT NOT NULL,
        kind TEXT NOT NULL,
        fired_at TEXT NOT NULL,
        scheduled_at TEXT,
        execution_id TEXT,
        status TEXT NOT NULL,
        event_id TEXT,
        detail_json TEXT
    )",
    "CREATE UNIQUE INDEX IF NOT EXISTS idx_trigger_fires_event ON trigger_fires(trigger_id, event_id) WHERE event_id IS NOT NULL",
    "CREATE INDEX IF NOT EXISTS idx_trigger_fires_at ON trigger_fires(fired_at DESC)",
];

impl Db {
    pub async fn open(path: &Path) -> DbResult<Self> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(options)
            .await?;
        for &statement in SCHEMA {
            sqlx::query(statement).execute(&pool).await?;
        }
        ensure_version_columns(&pool).await?;
        ensure_plugin_columns(&pool).await?;
        Ok(Self {
            pool,
            postgres_execution: None,
            postgres_trigger: None,
            postgres_agent: None,
            postgres_skill: None,
            postgres_mcp_meta: None,
            postgres_connector: None,
            postgres_plugin: None,
        })
    }

    pub fn with_postgres_execution(
        mut self,
        pool: sqlx::postgres::PgPool,
        tenant_id: impl Into<String>,
    ) -> Self {
        let tenant_id = tenant_id.into();
        self.postgres_execution = Some(PostgresExecutionStore::new(pool.clone(), &tenant_id));
        self.postgres_agent = Some(PostgresAgentStore::new(pool.clone(), &tenant_id));
        self.postgres_skill = Some(PostgresSkillStore::new(pool.clone(), &tenant_id));
        self.postgres_mcp_meta = Some(PostgresMcpMetaStore::new(pool.clone(), &tenant_id));
        self.postgres_connector = Some(PostgresConnectorStore::new(pool.clone(), &tenant_id));
        self.postgres_plugin = Some(PostgresPluginStore::new(pool.clone(), &tenant_id));
        self.postgres_trigger = Some(PostgresTriggerStore::new(pool, tenant_id));
        self
    }

    pub fn postgres_connector(&self) -> Option<PostgresConnectorStore> {
        self.postgres_connector.clone()
    }

    pub fn postgres_agent(&self) -> Option<PostgresAgentStore> {
        self.postgres_agent.clone()
    }

    pub fn postgres_plugin(&self) -> Option<PostgresPluginStore> {
        self.postgres_plugin.clone()
    }

    pub fn postgres_mcp_meta(&self) -> Option<PostgresMcpMetaStore> {
        self.postgres_mcp_meta.clone()
    }

    fn map_postgres_execution(error: PostgresExecutionError) -> DbError {
        DbError::PostgresExecution(error.to_string())
    }

    fn map_postgres_trigger(error: PostgresTriggerError) -> DbError {
        DbError::PostgresTrigger(error.to_string())
    }

    fn map_postgres_agent(error: PostgresAgentError) -> DbError {
        match error {
            PostgresAgentError::VersionFrozen { agent_id, version } => {
                DbError::AgentVersionFrozen { agent_id, version }
            }
            other => DbError::PostgresAgent(other.to_string()),
        }
    }

    fn map_postgres_skill(error: PostgresSkillError) -> DbError {
        match error {
            PostgresSkillError::VersionFrozen { skill_id, version } => {
                DbError::SkillVersionFrozen { skill_id, version }
            }
            other => DbError::PostgresSkill(other.to_string()),
        }
    }

    fn map_postgres_mcp_meta(error: PostgresMcpMetaError) -> DbError {
        DbError::PostgresMcpMeta(error.to_string())
    }

    pub async fn upsert_workflow(&self, row: &WorkflowRow) -> DbResult<()> {
        if let Some(store) = self.postgres_execution.as_ref() {
            return store
                .upsert_workflow(&WorkflowRecord::from(row))
                .await
                .map_err(Self::map_postgres_execution);
        }
        sqlx::query(
            "INSERT INTO workflows (id, name, version, description, published, ir_json, yaml, created_at, updated_at)
             VALUES (?, ?, ?, ?, 0, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                version = excluded.version,
                description = excluded.description,
                published = 0,
                ir_json = excluded.ir_json,
                yaml = excluded.yaml,
                updated_at = excluded.updated_at",
        )
        .bind(&row.id)
        .bind(&row.name)
        .bind(&row.version)
        .bind(&row.description)
        .bind(&row.ir_json)
        .bind(&row.yaml)
        .bind(&row.created_at)
        .bind(&row.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn publish_workflow(
        &self,
        row: &WorkflowRow,
        registry_version: &str,
    ) -> DbResult<()> {
        if let Some(store) = self.postgres_execution.as_ref() {
            return store
                .publish_workflow(&WorkflowRecord::from(row), registry_version)
                .await
                .map_err(|error| match error {
                    PostgresExecutionError::WorkflowSnapshotConflict {
                        workflow_id,
                        version,
                    } => DbError::VersionFrozen {
                        workflow_id,
                        version,
                    },
                    other => Self::map_postgres_execution(other),
                });
        }
        let mut tx = self.pool.begin().await?;
        let existing: Option<(String,)> = sqlx::query_as(
            "SELECT ir_json FROM workflow_versions WHERE workflow_id = ? AND version = ?",
        )
        .bind(&row.id)
        .bind(&row.version)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((stored,)) = existing {
            if stored != row.ir_json {
                return Err(DbError::VersionFrozen {
                    workflow_id: row.id.clone(),
                    version: row.version.clone(),
                });
            }
        } else {
            sqlx::query(
                "INSERT INTO workflow_versions (workflow_id, version, registry_version, ir_json, created_at)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&row.id)
            .bind(&row.version)
            .bind(registry_version)
            .bind(&row.ir_json)
            .bind(&row.updated_at)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query("UPDATE workflows SET published = 1, updated_at = ? WHERE id = ?")
            .bind(&row.updated_at)
            .bind(&row.id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn list_workflows(&self) -> DbResult<Vec<WorkflowRow>> {
        if let Some(store) = self.postgres_execution.as_ref() {
            return store
                .list_workflows()
                .await
                .map(|rows| rows.into_iter().map(Into::into).collect())
                .map_err(Self::map_postgres_execution);
        }
        Ok(sqlx::query_as::<_, WorkflowRow>(
            "SELECT id, name, version, description, published, ir_json, yaml, created_at, updated_at
             FROM workflows ORDER BY updated_at DESC",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn get_workflow(&self, id: &str) -> DbResult<Option<WorkflowRow>> {
        if let Some(store) = self.postgres_execution.as_ref() {
            return store
                .get_workflow(id)
                .await
                .map(|row| row.map(Into::into))
                .map_err(Self::map_postgres_execution);
        }
        Ok(sqlx::query_as::<_, WorkflowRow>(
            "SELECT id, name, version, description, published, ir_json, yaml, created_at, updated_at
             FROM workflows WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn list_versions(&self, workflow_id: &str) -> DbResult<Vec<WorkflowVersionRow>> {
        if let Some(store) = self.postgres_execution.as_ref() {
            return store
                .list_versions(workflow_id)
                .await
                .map(|rows| rows.into_iter().map(Into::into).collect())
                .map_err(Self::map_postgres_execution);
        }
        Ok(sqlx::query_as::<_, WorkflowVersionRow>(
            "SELECT version, registry_version, ir_json, created_at
             FROM workflow_versions WHERE workflow_id = ? ORDER BY created_at DESC",
        )
        .bind(workflow_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn get_version(
        &self,
        workflow_id: &str,
        version: &str,
    ) -> DbResult<Option<WorkflowVersionRow>> {
        if let Some(store) = self.postgres_execution.as_ref() {
            return store
                .get_workflow_version(workflow_id, version)
                .await
                .map(|row| row.map(Into::into))
                .map_err(Self::map_postgres_execution);
        }
        Ok(sqlx::query_as::<_, WorkflowVersionRow>(
            "SELECT version, registry_version, ir_json, created_at
             FROM workflow_versions WHERE workflow_id = ? AND version = ?",
        )
        .bind(workflow_id)
        .bind(version)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn import_skill(
        &self,
        snapshot: &SkillSnapshot,
        report: &SkillReport,
        source_path: Option<&str>,
        imported_at: &str,
    ) -> DbResult<bool> {
        if let Some(store) = self.postgres_skill.as_ref() {
            return store
                .import_skill(snapshot, report, source_path, imported_at)
                .await
                .map_err(Self::map_postgres_skill);
        }
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT content_hash FROM skill_versions WHERE skill_id = ? AND version = ?",
        )
        .bind(&snapshot.id)
        .bind(&snapshot.version)
        .fetch_optional(&self.pool)
        .await?;
        if let Some(hash) = existing {
            if hash == snapshot.content_hash {
                return Ok(false);
            }
            return Err(DbError::SkillVersionFrozen {
                skill_id: snapshot.id.clone(),
                version: snapshot.version.clone(),
            });
        }
        let manifest_json = serde_json::to_string(&snapshot.manifest)?;
        let report_json = serde_json::to_string(report)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO skill_versions
             (skill_id, version, name, license, manifest_json, skill_md, report_json, content_hash, status, source_path, imported_at, published_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'imported', ?, ?, NULL)",
        )
        .bind(&snapshot.id)
        .bind(&snapshot.version)
        .bind(&snapshot.name)
        .bind(&snapshot.license)
        .bind(&manifest_json)
        .bind(&snapshot.skill_md)
        .bind(&report_json)
        .bind(&snapshot.content_hash)
        .bind(source_path)
        .bind(imported_at)
        .execute(&mut *tx)
        .await?;
        for file in &snapshot.files {
            sqlx::query(
                "INSERT INTO skill_files (skill_id, version, path, sha256, size, content)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(&snapshot.id)
            .bind(&snapshot.version)
            .bind(&file.path)
            .bind(&file.sha256)
            .bind(file.size as i64)
            .bind(&file.content)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "INSERT INTO skills (id, name, latest_version, license, status, created_at, updated_at)
             VALUES (?, ?, ?, ?, 'imported', ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                latest_version = excluded.latest_version,
                license = excluded.license,
                status = excluded.status,
                updated_at = excluded.updated_at",
        )
        .bind(&snapshot.id)
        .bind(&snapshot.name)
        .bind(&snapshot.version)
        .bind(&snapshot.license)
        .bind(imported_at)
        .bind(imported_at)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    pub async fn list_skills(&self) -> DbResult<Vec<SkillSummaryRow>> {
        if let Some(store) = self.postgres_skill.as_ref() {
            return store
                .list_skills()
                .await
                .map(|records| records.into_iter().map(Into::into).collect())
                .map_err(Self::map_postgres_skill);
        }
        Ok(sqlx::query_as::<_, SkillSummaryRow>(
            "SELECT id, name, latest_version, license, status, updated_at
             FROM skills ORDER BY updated_at DESC",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn get_skill(&self, skill_id: &str) -> DbResult<Option<SkillSummaryRow>> {
        if let Some(store) = self.postgres_skill.as_ref() {
            return store
                .get_skill(skill_id)
                .await
                .map(|record| record.map(Into::into))
                .map_err(Self::map_postgres_skill);
        }
        Ok(sqlx::query_as::<_, SkillSummaryRow>(
            "SELECT id, name, latest_version, license, status, updated_at
             FROM skills WHERE id = ?",
        )
        .bind(skill_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn list_skill_versions(&self, skill_id: &str) -> DbResult<Vec<SkillVersionRow>> {
        if let Some(store) = self.postgres_skill.as_ref() {
            return store
                .list_skill_versions(skill_id)
                .await
                .map(|records| records.into_iter().map(Into::into).collect())
                .map_err(Self::map_postgres_skill);
        }
        Ok(sqlx::query_as::<_, SkillVersionRow>(
            "SELECT skill_id, version, name, license, manifest_json, skill_md, report_json,
                    content_hash, status, source_path, imported_at, published_at
             FROM skill_versions WHERE skill_id = ? ORDER BY imported_at DESC",
        )
        .bind(skill_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn get_skill_version(
        &self,
        skill_id: &str,
        version: &str,
    ) -> DbResult<Option<SkillVersionRow>> {
        if let Some(store) = self.postgres_skill.as_ref() {
            return store
                .get_skill_version(skill_id, version)
                .await
                .map(|record| record.map(Into::into))
                .map_err(Self::map_postgres_skill);
        }
        Ok(sqlx::query_as::<_, SkillVersionRow>(
            "SELECT skill_id, version, name, license, manifest_json, skill_md, report_json,
                    content_hash, status, source_path, imported_at, published_at
             FROM skill_versions WHERE skill_id = ? AND version = ?",
        )
        .bind(skill_id)
        .bind(version)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn get_skill_files(
        &self,
        skill_id: &str,
        version: &str,
    ) -> DbResult<Vec<SkillFileRow>> {
        if let Some(store) = self.postgres_skill.as_ref() {
            return store
                .get_skill_files(skill_id, version)
                .await
                .map(|records| records.into_iter().map(Into::into).collect())
                .map_err(Self::map_postgres_skill);
        }
        Ok(sqlx::query_as::<_, SkillFileRow>(
            "SELECT path, sha256, size, content FROM skill_files
             WHERE skill_id = ? AND version = ? ORDER BY path ASC",
        )
        .bind(skill_id)
        .bind(version)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn publish_skill_version(
        &self,
        skill_id: &str,
        version: &str,
        published_at: &str,
    ) -> DbResult<bool> {
        if let Some(store) = self.postgres_skill.as_ref() {
            return store
                .publish_skill_version(skill_id, version, published_at)
                .await
                .map_err(Self::map_postgres_skill);
        }
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            "UPDATE skill_versions SET status = 'published', published_at = ?
             WHERE skill_id = ? AND version = ? AND status != 'published'",
        )
        .bind(published_at)
        .bind(skill_id)
        .bind(version)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE skills SET status = 'published', updated_at = ?
             WHERE id = ? AND latest_version = ?",
        )
        .bind(published_at)
        .bind(skill_id)
        .bind(version)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn insert_skill_run(
        &self,
        outcome: &RunOutcome,
        input: &serde_json::Value,
    ) -> DbResult<()> {
        if let Some(store) = self.postgres_skill.as_ref() {
            return store
                .insert_skill_run(outcome, input)
                .await
                .map_err(Self::map_postgres_skill);
        }
        let input_json = serde_json::to_string(input)?;
        let output_json = outcome
            .output
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let error_json = outcome
            .error
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let status = match outcome.status {
            hpa_skill::RunStatus::Succeeded => "succeeded",
            hpa_skill::RunStatus::Failed => "failed",
            hpa_skill::RunStatus::TimedOut => "timed_out",
        };
        let sandbox = match outcome.sandbox {
            hpa_skill::SandboxKind::MacosSeatbelt => "macos_seatbelt",
            hpa_skill::SandboxKind::LinuxBubblewrap => "linux_bubblewrap",
            hpa_skill::SandboxKind::Process => "process",
        };
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO skill_runs
             (id, skill_id, version, script_id, status, input_json, output_json, stdout, stderr,
              exit_code, error_json, sandbox, duration_ms, started_at, finished_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&outcome.id)
        .bind(&outcome.skill_id)
        .bind(&outcome.version)
        .bind(&outcome.script_id)
        .bind(status)
        .bind(input_json)
        .bind(output_json)
        .bind(&outcome.stdout)
        .bind(&outcome.stderr)
        .bind(outcome.exit_code.map(i64::from))
        .bind(error_json)
        .bind(sandbox)
        .bind(outcome.duration_ms as i64)
        .bind(&outcome.started_at)
        .bind(&outcome.finished_at)
        .execute(&mut *tx)
        .await?;
        for artifact in &outcome.artifacts {
            sqlx::query(
                "INSERT INTO skill_run_artifacts
                 (id, run_id, name, sha256, size, mime_type, stored_path, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&artifact.id)
            .bind(&outcome.id)
            .bind(&artifact.name)
            .bind(&artifact.sha256)
            .bind(artifact.size as i64)
            .bind(&artifact.mime_type)
            .bind(artifact.stored_path.display().to_string())
            .bind(&outcome.finished_at)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn list_skill_runs(
        &self,
        skill_id: Option<&str>,
        limit: i64,
    ) -> DbResult<Vec<SkillRunRow>> {
        if let Some(store) = self.postgres_skill.as_ref() {
            return store
                .list_skill_runs(skill_id, limit)
                .await
                .map(|records| records.into_iter().map(Into::into).collect())
                .map_err(Self::map_postgres_skill);
        }
        let limit = limit.clamp(1, 200);
        match skill_id {
            Some(skill_id) => Ok(sqlx::query_as::<_, SkillRunRow>(
                "SELECT id, skill_id, version, script_id, status, input_json, output_json, stdout,
                        stderr, exit_code, error_json, sandbox, duration_ms, started_at, finished_at
                 FROM skill_runs WHERE skill_id = ? ORDER BY started_at DESC LIMIT ?",
            )
            .bind(skill_id)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?),
            None => Ok(sqlx::query_as::<_, SkillRunRow>(
                "SELECT id, skill_id, version, script_id, status, input_json, output_json, stdout,
                        stderr, exit_code, error_json, sandbox, duration_ms, started_at, finished_at
                 FROM skill_runs ORDER BY started_at DESC LIMIT ?",
            )
            .bind(limit)
            .fetch_all(&self.pool)
            .await?),
        }
    }

    pub async fn get_skill_run(&self, run_id: &str) -> DbResult<Option<SkillRunRow>> {
        if let Some(store) = self.postgres_skill.as_ref() {
            return store
                .get_skill_run(run_id)
                .await
                .map(|record| record.map(Into::into))
                .map_err(Self::map_postgres_skill);
        }
        Ok(sqlx::query_as::<_, SkillRunRow>(
            "SELECT id, skill_id, version, script_id, status, input_json, output_json, stdout,
                    stderr, exit_code, error_json, sandbox, duration_ms, started_at, finished_at
             FROM skill_runs WHERE id = ?",
        )
        .bind(run_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn list_skill_run_artifacts(&self, run_id: &str) -> DbResult<Vec<SkillArtifactRow>> {
        if let Some(store) = self.postgres_skill.as_ref() {
            return store
                .list_skill_run_artifacts(run_id)
                .await
                .map(|records| records.into_iter().map(Into::into).collect())
                .map_err(Self::map_postgres_skill);
        }
        Ok(sqlx::query_as::<_, SkillArtifactRow>(
            "SELECT id, run_id, name, sha256, size, mime_type, stored_path, created_at
             FROM skill_run_artifacts WHERE run_id = ? ORDER BY name ASC",
        )
        .bind(run_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn insert_execution(
        &self,
        row: &ExecutionRow,
        start_event: &ExecutionEvent,
    ) -> DbResult<()> {
        if let Some(store) = self.postgres_execution.as_ref() {
            return store
                .insert_execution(&ExecutionRecord::from(row), start_event)
                .await
                .map_err(Self::map_postgres_execution);
        }
        let event_json = serde_json::to_string(start_event)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO executions (id, workflow_id, workflow_version, status, state_json, workflow_json, output_json, error_json, started_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&row.id)
        .bind(&row.workflow_id)
        .bind(&row.workflow_version)
        .bind(&row.status)
        .bind(&row.state_json)
        .bind(&row.workflow_json)
        .bind(&row.output_json)
        .bind(&row.error_json)
        .bind(&row.started_at)
        .bind(&row.updated_at)
        .execute(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO events (execution_id, sequence, payload_json) VALUES (?, ?, ?)")
            .bind(&start_event.execution_id)
            .bind(start_event.sequence as i64)
            .bind(event_json)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn insert_execution_with_trigger(
        &self,
        row: &ExecutionRow,
        start_event: &ExecutionEvent,
        fire: &TriggerFireRow,
    ) -> DbResult<bool> {
        if let Some(store) = self.postgres_execution.as_ref() {
            return store
                .insert_execution_with_trigger(
                    &ExecutionRecord::from(row),
                    start_event,
                    &TriggerFireRecord::from(fire),
                )
                .await
                .map_err(Self::map_postgres_execution);
        }
        let event_json = serde_json::to_string(start_event)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO executions (id, workflow_id, workflow_version, status, state_json, workflow_json, output_json, error_json, started_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&row.id)
        .bind(&row.workflow_id)
        .bind(&row.workflow_version)
        .bind(&row.status)
        .bind(&row.state_json)
        .bind(&row.workflow_json)
        .bind(&row.output_json)
        .bind(&row.error_json)
        .bind(&row.started_at)
        .bind(&row.updated_at)
        .execute(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO events (execution_id, sequence, payload_json) VALUES (?, ?, ?)")
            .bind(&start_event.execution_id)
            .bind(start_event.sequence as i64)
            .bind(event_json)
            .execute(&mut *tx)
            .await?;
        let inserted = sqlx::query(
            "INSERT INTO trigger_fires
             (id, workflow_id, trigger_id, kind, fired_at, scheduled_at, execution_id, status,
              event_id, detail_json)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(trigger_id, event_id) WHERE event_id IS NOT NULL DO NOTHING",
        )
        .bind(&fire.id)
        .bind(&fire.workflow_id)
        .bind(&fire.trigger_id)
        .bind(&fire.kind)
        .bind(&fire.fired_at)
        .bind(&fire.scheduled_at)
        .bind(&fire.execution_id)
        .bind(&fire.status)
        .bind(&fire.event_id)
        .bind(&fire.detail_json)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        if !inserted {
            tx.rollback().await?;
            return Ok(false);
        }
        tx.commit().await?;
        Ok(true)
    }

    pub async fn save_execution(
        &self,
        state: &ExecutionState,
        events: &[ExecutionEvent],
    ) -> DbResult<()> {
        if let Some(store) = self.postgres_execution.as_ref() {
            return store
                .save_execution(state, events)
                .await
                .map_err(Self::map_postgres_execution);
        }
        let state_json = serde_json::to_string(state)?;
        let output_json = state
            .output
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let error_json = state
            .error
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE executions SET status = ?, state_json = ?, output_json = ?, error_json = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(status_text(state.status))
        .bind(state_json)
        .bind(output_json)
        .bind(error_json)
        .bind(&state.updated_at)
        .bind(&state.id)
        .execute(&mut *tx)
        .await?;
        for event in events {
            let payload = serde_json::to_string(event)?;
            sqlx::query(
                "INSERT INTO events (execution_id, sequence, payload_json) VALUES (?, ?, ?)
                 ON CONFLICT(execution_id, sequence) DO NOTHING",
            )
            .bind(&event.execution_id)
            .bind(event.sequence as i64)
            .bind(payload)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn acquire_execution_lease(
        &self,
        execution_id: &str,
        holder_id: &str,
        ttl_seconds: i32,
    ) -> DbResult<Option<ExecutionLeaseRecord>> {
        if let Some(store) = self.postgres_execution.as_ref() {
            return store
                .acquire_execution_lease(execution_id, holder_id, ttl_seconds)
                .await
                .map_err(Self::map_postgres_execution);
        }
        let expires_at = (time::OffsetDateTime::now_utc()
            + time::Duration::seconds(ttl_seconds as i64))
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
        Ok(Some(ExecutionLeaseRecord {
            holder_id: holder_id.to_string(),
            fencing_token: 0,
            expires_at,
        }))
    }

    pub async fn renew_execution_lease(
        &self,
        execution_id: &str,
        holder_id: &str,
        fencing_token: i64,
        ttl_seconds: i32,
    ) -> DbResult<bool> {
        if let Some(store) = self.postgres_execution.as_ref() {
            return store
                .renew_execution_lease(execution_id, holder_id, fencing_token, ttl_seconds)
                .await
                .map_err(Self::map_postgres_execution);
        }
        Ok(true)
    }

    pub async fn release_execution_lease(
        &self,
        execution_id: &str,
        holder_id: &str,
        fencing_token: i64,
    ) -> DbResult<bool> {
        if let Some(store) = self.postgres_execution.as_ref() {
            return store
                .release_execution_lease(execution_id, holder_id, fencing_token)
                .await
                .map_err(Self::map_postgres_execution);
        }
        Ok(true)
    }

    pub async fn save_execution_fenced(
        &self,
        state: &ExecutionState,
        events: &[ExecutionEvent],
        holder_id: &str,
        fencing_token: i64,
    ) -> DbResult<()> {
        if let Some(store) = self.postgres_execution.as_ref() {
            return store
                .save_execution_fenced(state, events, holder_id, fencing_token)
                .await
                .map_err(Self::map_postgres_execution);
        }
        self.save_execution(state, events).await
    }

    pub async fn get_execution(&self, id: &str) -> DbResult<Option<ExecutionRow>> {
        if let Some(store) = self.postgres_execution.as_ref() {
            return store
                .get_execution(id)
                .await
                .map(|row| row.map(Into::into))
                .map_err(Self::map_postgres_execution);
        }
        Ok(sqlx::query_as::<_, ExecutionRow>(
            "SELECT id, workflow_id, workflow_version, status, state_json, workflow_json, output_json, error_json, started_at, updated_at
             FROM executions WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Paged listing with an optional status filter; the caller supplies `count_executions`
    /// for the total so the header and the page agree.
    pub async fn list_executions_page(
        &self,
        limit: i64,
        offset: i64,
        status: Option<&str>,
    ) -> DbResult<Vec<ExecutionRow>> {
        if let Some(store) = self.postgres_execution.as_ref() {
            return store
                .list_executions_page(limit, offset, status)
                .await
                .map(|rows| rows.into_iter().map(Into::into).collect())
                .map_err(Self::map_postgres_execution);
        }
        Ok(sqlx::query_as::<_, ExecutionRow>(
            "SELECT id, workflow_id, workflow_version, status, state_json, workflow_json,
                    output_json, error_json, started_at, updated_at
             FROM executions
             WHERE (?1 IS NULL OR status = ?1)
             ORDER BY started_at DESC, id DESC
             LIMIT ?2 OFFSET ?3",
        )
        .bind(status)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn count_executions(&self, status: Option<&str>) -> DbResult<i64> {
        if let Some(store) = self.postgres_execution.as_ref() {
            return store
                .count_executions(status)
                .await
                .map_err(Self::map_postgres_execution);
        }
        Ok(
            sqlx::query_scalar("SELECT count(*) FROM executions WHERE (?1 IS NULL OR status = ?1)")
                .bind(status)
                .fetch_one(&self.pool)
                .await?,
        )
    }

    pub async fn list_executions(&self, limit: i64) -> DbResult<Vec<ExecutionRow>> {
        if let Some(store) = self.postgres_execution.as_ref() {
            return store
                .list_executions(limit)
                .await
                .map(|rows| rows.into_iter().map(Into::into).collect())
                .map_err(Self::map_postgres_execution);
        }
        Ok(sqlx::query_as::<_, ExecutionRow>(
            "SELECT id, workflow_id, workflow_version, status, state_json, workflow_json, output_json, error_json, started_at, updated_at
             FROM executions ORDER BY started_at DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn list_events(
        &self,
        execution_id: &str,
        after: i64,
        limit: i64,
    ) -> DbResult<Vec<ExecutionEvent>> {
        if let Some(store) = self.postgres_execution.as_ref() {
            return store
                .list_events(execution_id, after, limit)
                .await
                .map_err(Self::map_postgres_execution);
        }
        let rows = sqlx::query_as::<_, EventRow>(
            "SELECT payload_json FROM events WHERE execution_id = ? AND sequence > ?
             ORDER BY sequence ASC LIMIT ?",
        )
        .bind(execution_id)
        .bind(after)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        let mut events = Vec::with_capacity(rows.len());
        for row in rows {
            events.push(serde_json::from_str(&row.payload_json)?);
        }
        Ok(events)
    }

    pub async fn running_executions(&self) -> DbResult<Vec<ExecutionRow>> {
        if let Some(store) = self.postgres_execution.as_ref() {
            return store
                .running_executions()
                .await
                .map(|rows| rows.into_iter().map(Into::into).collect())
                .map_err(Self::map_postgres_execution);
        }
        Ok(sqlx::query_as::<_, ExecutionRow>(
            "SELECT id, workflow_id, workflow_version, status, state_json, workflow_json, output_json, error_json, started_at, updated_at
             FROM executions
             WHERE status = 'running'
               AND id NOT IN (SELECT execution_id FROM remote_executions)
             ORDER BY started_at ASC",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn waiting_executions(&self) -> DbResult<Vec<ExecutionRow>> {
        if let Some(store) = self.postgres_execution.as_ref() {
            return store
                .waiting_executions()
                .await
                .map(|rows| rows.into_iter().map(Into::into).collect())
                .map_err(Self::map_postgres_execution);
        }
        Ok(sqlx::query_as::<_, ExecutionRow>(
            "SELECT id, workflow_id, workflow_version, status, state_json, workflow_json, output_json, error_json, started_at, updated_at
             FROM executions WHERE status = 'waiting_approval' ORDER BY started_at ASC",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn waiting_local_executions(&self) -> DbResult<Vec<ExecutionRow>> {
        if let Some(store) = self.postgres_execution.as_ref() {
            return store
                .waiting_local_executions()
                .await
                .map(|rows| rows.into_iter().map(Into::into).collect())
                .map_err(Self::map_postgres_execution);
        }
        Ok(sqlx::query_as::<_, ExecutionRow>(
            "SELECT id, workflow_id, workflow_version, status, state_json, workflow_json, output_json, error_json, started_at, updated_at
             FROM executions
             WHERE status = 'waiting_approval'
               AND id NOT IN (SELECT execution_id FROM remote_executions)
             ORDER BY started_at ASC",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn mark_remote_execution(&self, execution_id: &str, worker_id: &str) -> DbResult<()> {
        if let Some(store) = self.postgres_execution.as_ref() {
            return store
                .mark_remote_execution(execution_id, worker_id)
                .await
                .map_err(Self::map_postgres_execution);
        }
        sqlx::query(
            "INSERT INTO remote_executions (execution_id, worker_id, created_at)
             VALUES (?, ?, ?) ON CONFLICT(execution_id) DO NOTHING",
        )
        .bind(execution_id)
        .bind(worker_id)
        .bind(
            time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default(),
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn is_remote_execution(&self, execution_id: &str) -> DbResult<bool> {
        if let Some(store) = self.postgres_execution.as_ref() {
            return store
                .is_remote_execution(execution_id)
                .await
                .map_err(Self::map_postgres_execution);
        }
        let found: Option<String> =
            sqlx::query_scalar("SELECT execution_id FROM remote_executions WHERE execution_id = ?")
                .bind(execution_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(found.is_some())
    }

    pub async fn save_output_artifact(&self, state: &ExecutionState) -> DbResult<()> {
        if let Some(store) = self.postgres_execution.as_ref() {
            return store
                .save_output_artifact(state)
                .await
                .map_err(Self::map_postgres_execution);
        }
        let Some(output) = &state.output else {
            return Ok(());
        };
        let content = serde_json::to_string(output)?;
        let id = format!("{}:output", state.id);
        sqlx::query(
            "INSERT INTO artifacts (id, execution_id, name, kind, mime_type, size, content_json, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                content_json = excluded.content_json,
                size = excluded.size,
                created_at = excluded.created_at",
        )
        .bind(&id)
        .bind(&state.id)
        .bind("output.json")
        .bind("execution-output")
        .bind("application/json")
        .bind(content.len() as i64)
        .bind(content)
        .bind(&state.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_artifacts(&self, execution_id: Option<&str>) -> DbResult<Vec<ArtifactRow>> {
        if let Some(store) = self.postgres_execution.as_ref() {
            return store
                .list_artifacts(execution_id)
                .await
                .map(|rows| rows.into_iter().map(Into::into).collect())
                .map_err(Self::map_postgres_execution);
        }
        let rows =
            match execution_id {
                Some(id) => sqlx::query_as::<_, ArtifactRow>(
                    "SELECT id, execution_id, name, kind, mime_type, size, content_json, created_at
                     FROM artifacts WHERE execution_id = ? ORDER BY created_at DESC",
                )
                .bind(id)
                .fetch_all(&self.pool)
                .await?,
                None => sqlx::query_as::<_, ArtifactRow>(
                    "SELECT id, execution_id, name, kind, mime_type, size, content_json, created_at
                     FROM artifacts ORDER BY created_at DESC",
                )
                .fetch_all(&self.pool)
                .await?,
            };
        Ok(rows)
    }

    pub async fn get_artifact(&self, id: &str) -> DbResult<Option<ArtifactRow>> {
        if let Some(store) = self.postgres_execution.as_ref() {
            return store
                .get_artifact(id)
                .await
                .map(|row| row.map(Into::into))
                .map_err(Self::map_postgres_execution);
        }
        Ok(sqlx::query_as::<_, ArtifactRow>(
            "SELECT id, execution_id, name, kind, mime_type, size, content_json, created_at
             FROM artifacts WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn stats(&self) -> DbResult<Stats> {
        let workflows = if let Some(store) = self.postgres_execution.as_ref() {
            store
                .workflow_count()
                .await
                .map_err(Self::map_postgres_execution)?
        } else {
            sqlx::query_scalar("SELECT COUNT(*) FROM workflows")
                .fetch_one(&self.pool)
                .await?
        };
        if let Some(store) = self.postgres_execution.as_ref() {
            let (executions, running, waiting_approval, succeeded, failed) = store
                .execution_counts()
                .await
                .map_err(Self::map_postgres_execution)?;
            return Ok(Stats {
                workflows,
                executions,
                running,
                waiting_approval,
                succeeded,
                failed,
            });
        }
        let executions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM executions")
            .fetch_one(&self.pool)
            .await?;
        let running: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM executions WHERE status = 'running'")
                .fetch_one(&self.pool)
                .await?;
        let waiting_approval: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM executions WHERE status = 'waiting_approval'")
                .fetch_one(&self.pool)
                .await?;
        let succeeded: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM executions WHERE status = 'succeeded'")
                .fetch_one(&self.pool)
                .await?;
        let failed: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM executions WHERE status = 'failed'")
                .fetch_one(&self.pool)
                .await?;
        Ok(Stats {
            workflows,
            executions,
            running,
            waiting_approval,
            succeeded,
            failed,
        })
    }

    pub async fn execution_count_since(&self, date_prefix: &str) -> DbResult<i64> {
        if let Some(store) = self.postgres_execution.as_ref() {
            return store
                .execution_count_since(date_prefix)
                .await
                .map_err(Self::map_postgres_execution);
        }
        sqlx::query_scalar("SELECT COUNT(*) FROM executions WHERE started_at LIKE ?")
            .bind(format!("{date_prefix}%"))
            .fetch_one(&self.pool)
            .await
            .map_err(DbError::from)
    }

    pub async fn active_execution_count(&self) -> DbResult<i64> {
        if let Some(store) = self.postgres_execution.as_ref() {
            return store
                .active_execution_count()
                .await
                .map_err(Self::map_postgres_execution);
        }
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM executions
             WHERE status IN ('running', 'waiting_approval')",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(DbError::from)
    }

    pub async fn workflow_count(&self) -> DbResult<i64> {
        if let Some(store) = self.postgres_execution.as_ref() {
            return store
                .workflow_count()
                .await
                .map_err(Self::map_postgres_execution);
        }
        sqlx::query_scalar("SELECT COUNT(*) FROM workflows")
            .fetch_one(&self.pool)
            .await
            .map_err(DbError::from)
    }

    pub async fn create_mcp_token(
        &self,
        name: &str,
        token_hash: &str,
        scopes_json: &str,
        tenant_id: &str,
        created_at: &str,
    ) -> DbResult<McpTokenRow> {
        if let Some(store) = self.postgres_mcp_meta.as_ref() {
            return store
                .create_token(name, token_hash, scopes_json, tenant_id, created_at)
                .await
                .map(Into::into)
                .map_err(Self::map_postgres_mcp_meta);
        }
        let id = format!("mcptok_{}", uuid::Uuid::now_v7());
        sqlx::query(
            "INSERT INTO mcp_tokens (id, name, token_hash, scopes_json, tenant_id, created_at, last_used_at, revoked_at)
             VALUES (?, ?, ?, ?, ?, ?, NULL, NULL)",
        )
        .bind(&id)
        .bind(name)
        .bind(token_hash)
        .bind(scopes_json)
        .bind(tenant_id)
        .bind(created_at)
        .execute(&self.pool)
        .await?;
        self.get_mcp_token(&id)
            .await?
            .ok_or(sqlx::Error::RowNotFound.into())
    }

    pub async fn get_mcp_token(&self, id: &str) -> DbResult<Option<McpTokenRow>> {
        if let Some(store) = self.postgres_mcp_meta.as_ref() {
            return store
                .get_token(id)
                .await
                .map(|record| record.map(Into::into))
                .map_err(Self::map_postgres_mcp_meta);
        }
        Ok(sqlx::query_as::<_, McpTokenRow>(
            "SELECT id, name, scopes_json, tenant_id, created_at, last_used_at, revoked_at
             FROM mcp_tokens WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn find_mcp_token(
        &self,
        token_hash: &str,
        now: &str,
    ) -> DbResult<Option<McpTokenRow>> {
        if let Some(store) = self.postgres_mcp_meta.as_ref() {
            return store
                .find_token(token_hash, now)
                .await
                .map(|record| record.map(Into::into))
                .map_err(Self::map_postgres_mcp_meta);
        }
        let row = sqlx::query_as::<_, McpTokenRow>(
            "SELECT id, name, scopes_json, tenant_id, created_at, last_used_at, revoked_at
             FROM mcp_tokens WHERE token_hash = ? AND revoked_at IS NULL",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await?;
        if let Some(row) = &row {
            sqlx::query("UPDATE mcp_tokens SET last_used_at = ? WHERE id = ?")
                .bind(now)
                .bind(&row.id)
                .execute(&self.pool)
                .await?;
        }
        Ok(row)
    }

    pub async fn list_mcp_tokens(&self) -> DbResult<Vec<McpTokenRow>> {
        if let Some(store) = self.postgres_mcp_meta.as_ref() {
            return store
                .list_tokens()
                .await
                .map(|records| records.into_iter().map(Into::into).collect())
                .map_err(Self::map_postgres_mcp_meta);
        }
        Ok(sqlx::query_as::<_, McpTokenRow>(
            "SELECT id, name, scopes_json, tenant_id, created_at, last_used_at, revoked_at
             FROM mcp_tokens ORDER BY created_at DESC",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn revoke_mcp_token(&self, id: &str, now: &str) -> DbResult<bool> {
        if let Some(store) = self.postgres_mcp_meta.as_ref() {
            return store
                .revoke_token(id, now)
                .await
                .map_err(Self::map_postgres_mcp_meta);
        }
        let result =
            sqlx::query("UPDATE mcp_tokens SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL")
                .bind(now)
                .bind(id)
                .execute(&self.pool)
                .await?;
        Ok(result.rows_affected() > 0)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_mcp_approval(
        &self,
        token_id: &str,
        tool_name: &str,
        arguments_json: &str,
        arguments_hash: &str,
        request_state: &str,
        risk: &str,
        created_at: &str,
        expires_at: &str,
    ) -> DbResult<McpApprovalRow> {
        if let Some(store) = self.postgres_mcp_meta.as_ref() {
            return store
                .create_approval(
                    token_id,
                    tool_name,
                    arguments_json,
                    arguments_hash,
                    request_state,
                    risk,
                    created_at,
                    expires_at,
                )
                .await
                .map(Into::into)
                .map_err(Self::map_postgres_mcp_meta);
        }
        let id = format!("mcpappr_{}", uuid::Uuid::now_v7());
        sqlx::query(
            "INSERT INTO mcp_approvals
             (id, token_id, tool_name, arguments_json, arguments_hash, request_state, risk, status,
              created_at, expires_at, decided_at, used_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?, NULL, NULL)",
        )
        .bind(&id)
        .bind(token_id)
        .bind(tool_name)
        .bind(arguments_json)
        .bind(arguments_hash)
        .bind(request_state)
        .bind(risk)
        .bind(created_at)
        .bind(expires_at)
        .execute(&self.pool)
        .await?;
        self.get_mcp_approval(&id)
            .await?
            .ok_or(sqlx::Error::RowNotFound.into())
    }

    pub async fn get_mcp_approval(&self, id: &str) -> DbResult<Option<McpApprovalRow>> {
        if let Some(store) = self.postgres_mcp_meta.as_ref() {
            return store
                .get_approval(id)
                .await
                .map(|record| record.map(Into::into))
                .map_err(Self::map_postgres_mcp_meta);
        }
        Ok(sqlx::query_as::<_, McpApprovalRow>(
            "SELECT id, token_id, tool_name, arguments_json, arguments_hash, request_state, risk,
                    status, created_at, expires_at, decided_at, used_at
             FROM mcp_approvals WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn get_mcp_approval_by_state(&self, state: &str) -> DbResult<Option<McpApprovalRow>> {
        if let Some(store) = self.postgres_mcp_meta.as_ref() {
            return store
                .get_approval_by_state(state)
                .await
                .map(|record| record.map(Into::into))
                .map_err(Self::map_postgres_mcp_meta);
        }
        Ok(sqlx::query_as::<_, McpApprovalRow>(
            "SELECT id, token_id, tool_name, arguments_json, arguments_hash, request_state, risk,
                    status, created_at, expires_at, decided_at, used_at
             FROM mcp_approvals WHERE request_state = ?",
        )
        .bind(state)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn list_mcp_approvals(
        &self,
        status: Option<&str>,
        limit: i64,
    ) -> DbResult<Vec<McpApprovalRow>> {
        if let Some(store) = self.postgres_mcp_meta.as_ref() {
            return store
                .list_approvals(status, limit)
                .await
                .map(|records| records.into_iter().map(Into::into).collect())
                .map_err(Self::map_postgres_mcp_meta);
        }
        let limit = limit.clamp(1, 200);
        match status {
            Some(status) => Ok(sqlx::query_as::<_, McpApprovalRow>(
                "SELECT id, token_id, tool_name, arguments_json, arguments_hash, request_state, risk,
                        status, created_at, expires_at, decided_at, used_at
                 FROM mcp_approvals WHERE status = ? ORDER BY created_at DESC LIMIT ?",
            )
            .bind(status)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?),
            None => Ok(sqlx::query_as::<_, McpApprovalRow>(
                "SELECT id, token_id, tool_name, arguments_json, arguments_hash, request_state, risk,
                        status, created_at, expires_at, decided_at, used_at
                 FROM mcp_approvals ORDER BY created_at DESC LIMIT ?",
            )
            .bind(limit)
            .fetch_all(&self.pool)
            .await?),
        }
    }

    pub async fn decide_mcp_approval(
        &self,
        id: &str,
        status: &str,
        decided_at: &str,
    ) -> DbResult<bool> {
        if let Some(store) = self.postgres_mcp_meta.as_ref() {
            return store
                .decide_approval(id, status, decided_at)
                .await
                .map_err(Self::map_postgres_mcp_meta);
        }
        let result = sqlx::query(
            "UPDATE mcp_approvals SET status = ?, decided_at = ?
             WHERE id = ? AND status = 'pending'",
        )
        .bind(status)
        .bind(decided_at)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn consume_mcp_approval(&self, id: &str, used_at: &str) -> DbResult<bool> {
        if let Some(store) = self.postgres_mcp_meta.as_ref() {
            return store
                .consume_approval(id, used_at)
                .await
                .map_err(Self::map_postgres_mcp_meta);
        }
        let result = sqlx::query(
            "UPDATE mcp_approvals SET used_at = ?
             WHERE id = ? AND status = 'approved' AND used_at IS NULL",
        )
        .bind(used_at)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn expire_mcp_approvals(&self, now: &str) -> DbResult<u64> {
        if let Some(store) = self.postgres_mcp_meta.as_ref() {
            return store
                .expire_approvals(now)
                .await
                .map_err(Self::map_postgres_mcp_meta);
        }
        let result = sqlx::query(
            "UPDATE mcp_approvals SET status = 'expired'
             WHERE status = 'pending' AND expires_at < ?",
        )
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    pub async fn insert_mcp_audit(&self, row: &McpAuditRow) -> DbResult<()> {
        if let Some(store) = self.postgres_mcp_meta.as_ref() {
            return store
                .insert_audit(&McpAuditRecord::from(row))
                .await
                .map_err(Self::map_postgres_mcp_meta);
        }
        sqlx::query(
            "INSERT INTO mcp_audit (id, at, token_id, method, tool_name, status, latency_ms, detail_json)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&row.id)
        .bind(&row.at)
        .bind(&row.token_id)
        .bind(&row.method)
        .bind(&row.tool_name)
        .bind(&row.status)
        .bind(row.latency_ms)
        .bind(&row.detail_json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_mcp_audit(&self, limit: i64) -> DbResult<Vec<McpAuditRow>> {
        if let Some(store) = self.postgres_mcp_meta.as_ref() {
            return store
                .list_audit(limit)
                .await
                .map(|records| records.into_iter().map(Into::into).collect())
                .map_err(Self::map_postgres_mcp_meta);
        }
        Ok(sqlx::query_as::<_, McpAuditRow>(
            "SELECT id, at, token_id, method, tool_name, status, latency_ms, detail_json
             FROM mcp_audit ORDER BY at DESC LIMIT ?",
        )
        .bind(limit.clamp(1, 500))
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn mcp_stats(&self) -> DbResult<(i64, i64, i64)> {
        if let Some(store) = self.postgres_mcp_meta.as_ref() {
            return store.stats().await.map_err(Self::map_postgres_mcp_meta);
        }
        let pending: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM mcp_approvals WHERE status = 'pending'")
                .fetch_one(&self.pool)
                .await?;
        let tools: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM skill_versions WHERE status = 'published'")
                .fetch_one(&self.pool)
                .await?;
        let requests: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mcp_audit")
            .fetch_one(&self.pool)
            .await?;
        Ok((pending, tools, requests))
    }

    pub async fn upsert_mcp_server(&self, row: &McpServerRow) -> DbResult<()> {
        if let Some(store) = self.postgres_mcp_meta.as_ref() {
            return store
                .upsert_server(&McpServerRecord::from(row.clone()))
                .await
                .map_err(Self::map_postgres_mcp_meta);
        }
        sqlx::query(
            "INSERT INTO mcp_servers
             (id, name, transport, command_json, url, allowed_tools_json, network_json, credential_env, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                transport = excluded.transport,
                command_json = excluded.command_json,
                url = excluded.url,
                allowed_tools_json = excluded.allowed_tools_json,
                network_json = excluded.network_json,
                credential_env = excluded.credential_env,
                updated_at = excluded.updated_at",
        )
        .bind(&row.id)
        .bind(&row.name)
        .bind(&row.transport)
        .bind(&row.command_json)
        .bind(&row.url)
        .bind(&row.allowed_tools_json)
        .bind(&row.network_json)
        .bind(&row.credential_env)
        .bind(&row.created_at)
        .bind(&row.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_mcp_servers(&self) -> DbResult<Vec<McpServerRow>> {
        if let Some(store) = self.postgres_mcp_meta.as_ref() {
            return store
                .list_servers()
                .await
                .map(|records| records.into_iter().map(Into::into).collect())
                .map_err(Self::map_postgres_mcp_meta);
        }
        Ok(sqlx::query_as::<_, McpServerRow>(
            "SELECT id, name, transport, command_json, url, allowed_tools_json, network_json,
                    credential_env, created_at, updated_at
             FROM mcp_servers WHERE enabled = 1 ORDER BY name ASC",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn get_mcp_server(&self, id: &str) -> DbResult<Option<McpServerRow>> {
        if let Some(store) = self.postgres_mcp_meta.as_ref() {
            return store
                .get_server(id)
                .await
                .map(|record| record.map(Into::into))
                .map_err(Self::map_postgres_mcp_meta);
        }
        Ok(sqlx::query_as::<_, McpServerRow>(
            "SELECT id, name, transport, command_json, url, allowed_tools_json, network_json,
                    credential_env, created_at, updated_at
             FROM mcp_servers WHERE id = ? AND enabled = 1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn delete_mcp_server(&self, id: &str) -> DbResult<bool> {
        if let Some(store) = self.postgres_mcp_meta.as_ref() {
            return store
                .delete_server(id)
                .await
                .map_err(Self::map_postgres_mcp_meta);
        }
        let result = sqlx::query("DELETE FROM mcp_servers WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn upsert_agent_version(&self, row: &AgentVersionRow) -> DbResult<bool> {
        if let Some(store) = self.postgres_agent.as_ref() {
            return store
                .upsert_agent_version(&AgentVersionRecord::from(row))
                .await
                .map_err(Self::map_postgres_agent);
        }
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT content_hash FROM agent_versions WHERE agent_id = ? AND version = ?",
        )
        .bind(&row.agent_id)
        .bind(&row.version)
        .fetch_optional(&self.pool)
        .await?;
        if let Some(hash) = existing {
            if hash == row.content_hash {
                return Ok(false);
            }
            return Err(DbError::AgentVersionFrozen {
                agent_id: row.agent_id.clone(),
                version: row.version.clone(),
            });
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO agent_versions
             (agent_id, version, name, description, config_json, content_hash, status, created_at, published_at)
             VALUES (?, ?, ?, ?, ?, ?, 'imported', ?, NULL)",
        )
        .bind(&row.agent_id)
        .bind(&row.version)
        .bind(&row.name)
        .bind(&row.description)
        .bind(&row.config_json)
        .bind(&row.content_hash)
        .bind(&row.created_at)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO agents (id, name, latest_version, status, created_at, updated_at)
             VALUES (?, ?, ?, 'imported', ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                latest_version = excluded.latest_version,
                status = excluded.status,
                updated_at = excluded.updated_at",
        )
        .bind(&row.agent_id)
        .bind(&row.name)
        .bind(&row.version)
        .bind(&row.created_at)
        .bind(&row.created_at)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    pub async fn agent_count(&self) -> DbResult<i64> {
        if let Some(store) = self.postgres_agent.as_ref() {
            return store.count_agents().await.map_err(Self::map_postgres_agent);
        }
        Ok(sqlx::query_scalar("SELECT COUNT(*) FROM agents")
            .fetch_one(&self.pool)
            .await?)
    }

    pub async fn agent_sessions_on(&self, day: &str) -> DbResult<i64> {
        if let Some(store) = self.postgres_agent.as_ref() {
            return store
                .count_agent_sessions_on(day)
                .await
                .map_err(Self::map_postgres_agent);
        }
        Ok(
            sqlx::query_scalar("SELECT COUNT(*) FROM agent_sessions WHERE created_at LIKE ?")
                .bind(format!("{day}%"))
                .fetch_one(&self.pool)
                .await?,
        )
    }

    pub async fn skill_count(&self) -> DbResult<i64> {
        if let Some(store) = self.postgres_skill.as_ref() {
            return store.count_skills().await.map_err(Self::map_postgres_skill);
        }
        Ok(sqlx::query_scalar("SELECT COUNT(DISTINCT id) FROM skills")
            .fetch_one(&self.pool)
            .await?)
    }

    pub async fn list_agents(&self) -> DbResult<Vec<AgentSummaryRow>> {
        if let Some(store) = self.postgres_agent.as_ref() {
            return store
                .list_agents()
                .await
                .map(|records| records.into_iter().map(Into::into).collect())
                .map_err(Self::map_postgres_agent);
        }
        Ok(sqlx::query_as::<_, AgentSummaryRow>(
            "SELECT id, name, latest_version, status, created_at, updated_at
             FROM agents ORDER BY updated_at DESC",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn get_agent(&self, id: &str) -> DbResult<Option<AgentSummaryRow>> {
        if let Some(store) = self.postgres_agent.as_ref() {
            return store
                .get_agent(id)
                .await
                .map(|record| record.map(Into::into))
                .map_err(Self::map_postgres_agent);
        }
        Ok(sqlx::query_as::<_, AgentSummaryRow>(
            "SELECT id, name, latest_version, status, created_at, updated_at
             FROM agents WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn get_agent_version(
        &self,
        agent_id: &str,
        version: &str,
    ) -> DbResult<Option<AgentVersionRow>> {
        if let Some(store) = self.postgres_agent.as_ref() {
            return store
                .get_agent_version(agent_id, version)
                .await
                .map(|record| record.map(Into::into))
                .map_err(Self::map_postgres_agent);
        }
        Ok(sqlx::query_as::<_, AgentVersionRow>(
            "SELECT agent_id, version, name, description, config_json, content_hash, status,
                    created_at, published_at
             FROM agent_versions WHERE agent_id = ? AND version = ?",
        )
        .bind(agent_id)
        .bind(version)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn list_agent_versions(&self, agent_id: &str) -> DbResult<Vec<AgentVersionRow>> {
        if let Some(store) = self.postgres_agent.as_ref() {
            return store
                .list_agent_versions(agent_id)
                .await
                .map(|records| records.into_iter().map(Into::into).collect())
                .map_err(Self::map_postgres_agent);
        }
        Ok(sqlx::query_as::<_, AgentVersionRow>(
            "SELECT agent_id, version, name, description, config_json, content_hash, status,
                    created_at, published_at
             FROM agent_versions WHERE agent_id = ? ORDER BY created_at DESC",
        )
        .bind(agent_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn latest_published_agent_version(
        &self,
        agent_id: &str,
    ) -> DbResult<Option<AgentVersionRow>> {
        if let Some(store) = self.postgres_agent.as_ref() {
            return store
                .latest_published_agent_version(agent_id)
                .await
                .map(|record| record.map(Into::into))
                .map_err(Self::map_postgres_agent);
        }
        Ok(sqlx::query_as::<_, AgentVersionRow>(
            "SELECT agent_id, version, name, description, config_json, content_hash, status,
                    created_at, published_at
             FROM agent_versions WHERE agent_id = ? AND status = 'published'
             ORDER BY published_at DESC LIMIT 1",
        )
        .bind(agent_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn publish_agent_version(
        &self,
        agent_id: &str,
        version: &str,
        published_at: &str,
    ) -> DbResult<bool> {
        if let Some(store) = self.postgres_agent.as_ref() {
            return store
                .publish_agent_version(agent_id, version, published_at)
                .await
                .map_err(Self::map_postgres_agent);
        }
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            "UPDATE agent_versions SET status = 'published', published_at = ?
             WHERE agent_id = ? AND version = ? AND status != 'published'",
        )
        .bind(published_at)
        .bind(agent_id)
        .bind(version)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE agents SET status = 'published', updated_at = ?
             WHERE id = ? AND latest_version = ?",
        )
        .bind(published_at)
        .bind(agent_id)
        .bind(version)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn insert_agent_session(&self, row: &AgentSessionRow) -> DbResult<()> {
        if let Some(store) = self.postgres_agent.as_ref() {
            return store
                .insert_agent_session(&AgentSessionRecord::from(row))
                .await
                .map_err(Self::map_postgres_agent);
        }
        sqlx::query(
            "INSERT INTO agent_sessions
             (id, agent_id, agent_version, status, input_json, output_json, state_json, error_json,
              parent_session_id, depth, created_at, updated_at, finished_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&row.id)
        .bind(&row.agent_id)
        .bind(&row.agent_version)
        .bind(&row.status)
        .bind(&row.input_json)
        .bind(&row.output_json)
        .bind(&row.state_json)
        .bind(&row.error_json)
        .bind(&row.parent_session_id)
        .bind(row.depth)
        .bind(&row.created_at)
        .bind(&row.updated_at)
        .bind(&row.finished_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update_agent_session(&self, row: &AgentSessionRow) -> DbResult<()> {
        if let Some(store) = self.postgres_agent.as_ref() {
            return store
                .update_agent_session(&AgentSessionRecord::from(row))
                .await
                .map_err(Self::map_postgres_agent);
        }
        sqlx::query(
            "UPDATE agent_sessions SET status = ?, output_json = ?, state_json = ?, error_json = ?,
                    updated_at = ?, finished_at = ? WHERE id = ?",
        )
        .bind(&row.status)
        .bind(&row.output_json)
        .bind(&row.state_json)
        .bind(&row.error_json)
        .bind(&row.updated_at)
        .bind(&row.finished_at)
        .bind(&row.id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update_agent_session_fenced(
        &self,
        row: &AgentSessionRow,
        owner: &str,
        token: i64,
        ttl_ms: i64,
    ) -> DbResult<bool> {
        if let Some(store) = self.postgres_agent.as_ref() {
            return store
                .fenced_update_agent_session(&AgentSessionRecord::from(row), owner, token, ttl_ms)
                .await
                .map_err(Self::map_postgres_agent);
        }
        self.update_agent_session(row).await?;
        Ok(true)
    }

    pub async fn get_agent_session(&self, id: &str) -> DbResult<Option<AgentSessionRow>> {
        if let Some(store) = self.postgres_agent.as_ref() {
            return store
                .get_agent_session(id)
                .await
                .map(|record| record.map(Into::into))
                .map_err(Self::map_postgres_agent);
        }
        Ok(sqlx::query_as::<_, AgentSessionRow>(
            "SELECT id, agent_id, agent_version, status, input_json, output_json, state_json,
                    error_json, parent_session_id, depth, created_at, updated_at, finished_at
             FROM agent_sessions WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn list_agent_sessions(
        &self,
        agent_id: Option<&str>,
        limit: i64,
    ) -> DbResult<Vec<AgentSessionRow>> {
        if let Some(store) = self.postgres_agent.as_ref() {
            return store
                .list_agent_sessions(agent_id, limit)
                .await
                .map(|records| records.into_iter().map(Into::into).collect())
                .map_err(Self::map_postgres_agent);
        }
        let limit = limit.clamp(1, 200);
        match agent_id {
            Some(agent_id) => Ok(sqlx::query_as::<_, AgentSessionRow>(
                "SELECT id, agent_id, agent_version, status, input_json, output_json, state_json,
                        error_json, parent_session_id, depth, created_at, updated_at, finished_at
                 FROM agent_sessions WHERE agent_id = ? ORDER BY created_at DESC LIMIT ?",
            )
            .bind(agent_id)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?),
            None => Ok(sqlx::query_as::<_, AgentSessionRow>(
                "SELECT id, agent_id, agent_version, status, input_json, output_json, state_json,
                        error_json, parent_session_id, depth, created_at, updated_at, finished_at
                 FROM agent_sessions ORDER BY created_at DESC LIMIT ?",
            )
            .bind(limit)
            .fetch_all(&self.pool)
            .await?),
        }
    }

    pub async fn running_agent_sessions(&self) -> DbResult<Vec<AgentSessionRow>> {
        if let Some(store) = self.postgres_agent.as_ref() {
            return store
                .running_agent_sessions()
                .await
                .map(|records| records.into_iter().map(Into::into).collect())
                .map_err(Self::map_postgres_agent);
        }
        Ok(sqlx::query_as::<_, AgentSessionRow>(
            "SELECT id, agent_id, agent_version, status, input_json, output_json, state_json,
                    error_json, parent_session_id, depth, created_at, updated_at, finished_at
             FROM agent_sessions WHERE status = 'running' ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn append_agent_events(&self, events: &[AgentEventRow]) -> DbResult<()> {
        if let Some(store) = self.postgres_agent.as_ref() {
            let records = events
                .iter()
                .map(AgentEventRecord::from)
                .collect::<Vec<_>>();
            return store
                .append_agent_events(&records)
                .await
                .map_err(Self::map_postgres_agent);
        }
        let mut tx = self.pool.begin().await?;
        for event in events {
            sqlx::query(
                "INSERT OR REPLACE INTO agent_events (session_id, sequence, at, kind, data_json)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&event.session_id)
            .bind(event.sequence)
            .bind(&event.at)
            .bind(&event.kind)
            .bind(&event.data_json)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn append_agent_events_fenced(
        &self,
        events: &[AgentEventRow],
        owner: &str,
        token: i64,
        ttl_ms: i64,
    ) -> DbResult<bool> {
        if let Some(store) = self.postgres_agent.as_ref() {
            let records = events
                .iter()
                .map(AgentEventRecord::from)
                .collect::<Vec<_>>();
            return store
                .fenced_append_agent_events(&records, owner, token, ttl_ms)
                .await
                .map_err(Self::map_postgres_agent);
        }
        self.append_agent_events(events).await?;
        Ok(true)
    }

    pub async fn list_agent_events(
        &self,
        session_id: &str,
        limit: i64,
    ) -> DbResult<Vec<AgentEventRow>> {
        if let Some(store) = self.postgres_agent.as_ref() {
            return store
                .list_agent_events(session_id, limit)
                .await
                .map(|records| records.into_iter().map(Into::into).collect())
                .map_err(Self::map_postgres_agent);
        }
        Ok(sqlx::query_as::<_, AgentEventRow>(
            "SELECT session_id, sequence, at, kind, data_json FROM agent_events
             WHERE session_id = ? ORDER BY sequence ASC LIMIT ?",
        )
        .bind(session_id)
        .bind(limit.clamp(1, 1000))
        .fetch_all(&self.pool)
        .await?)
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
    ) -> DbResult<AgentApprovalRow> {
        if let Some(store) = self.postgres_agent.as_ref() {
            return store
                .create_agent_approval(
                    session_id, tool_name, args_json, risk, created_at, expires_at,
                )
                .await
                .map(Into::into)
                .map_err(Self::map_postgres_agent);
        }
        let id = format!("agentappr_{}", uuid::Uuid::now_v7());
        sqlx::query(
            "INSERT INTO agent_approvals
             (id, session_id, tool_name, args_json, risk, status, created_at, expires_at, decided_at, used_at)
             VALUES (?, ?, ?, ?, ?, 'pending', ?, ?, NULL, NULL)",
        )
        .bind(&id)
        .bind(session_id)
        .bind(tool_name)
        .bind(args_json)
        .bind(risk)
        .bind(created_at)
        .bind(expires_at)
        .execute(&self.pool)
        .await?;
        self.get_agent_approval(&id)
            .await?
            .ok_or(sqlx::Error::RowNotFound.into())
    }

    pub async fn get_agent_approval(&self, id: &str) -> DbResult<Option<AgentApprovalRow>> {
        if let Some(store) = self.postgres_agent.as_ref() {
            return store
                .get_agent_approval(id)
                .await
                .map(|record| record.map(Into::into))
                .map_err(Self::map_postgres_agent);
        }
        Ok(sqlx::query_as::<_, AgentApprovalRow>(
            "SELECT id, session_id, tool_name, args_json, risk, status, created_at, expires_at,
                    decided_at, used_at
             FROM agent_approvals WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn list_agent_approvals(
        &self,
        status: Option<&str>,
        limit: i64,
    ) -> DbResult<Vec<AgentApprovalRow>> {
        if let Some(store) = self.postgres_agent.as_ref() {
            return store
                .list_agent_approvals(status, limit)
                .await
                .map(|records| records.into_iter().map(Into::into).collect())
                .map_err(Self::map_postgres_agent);
        }
        let limit = limit.clamp(1, 200);
        match status {
            Some(status) => Ok(sqlx::query_as::<_, AgentApprovalRow>(
                "SELECT id, session_id, tool_name, args_json, risk, status, created_at, expires_at,
                        decided_at, used_at
                 FROM agent_approvals WHERE status = ? ORDER BY created_at DESC LIMIT ?",
            )
            .bind(status)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?),
            None => Ok(sqlx::query_as::<_, AgentApprovalRow>(
                "SELECT id, session_id, tool_name, args_json, risk, status, created_at, expires_at,
                        decided_at, used_at
                 FROM agent_approvals ORDER BY created_at DESC LIMIT ?",
            )
            .bind(limit)
            .fetch_all(&self.pool)
            .await?),
        }
    }

    pub async fn decide_agent_approval(
        &self,
        id: &str,
        status: &str,
        decided_at: &str,
    ) -> DbResult<bool> {
        if let Some(store) = self.postgres_agent.as_ref() {
            return store
                .decide_agent_approval(id, status, decided_at)
                .await
                .map_err(Self::map_postgres_agent);
        }
        let result = sqlx::query(
            "UPDATE agent_approvals SET status = ?, decided_at = ?
             WHERE id = ? AND status = 'pending'",
        )
        .bind(status)
        .bind(decided_at)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn consume_agent_approval(&self, id: &str, used_at: &str) -> DbResult<bool> {
        if let Some(store) = self.postgres_agent.as_ref() {
            return store
                .consume_agent_approval(id, used_at)
                .await
                .map_err(Self::map_postgres_agent);
        }
        let result = sqlx::query(
            "UPDATE agent_approvals SET used_at = ?
             WHERE id = ? AND status = 'approved' AND used_at IS NULL",
        )
        .bind(used_at)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn expire_agent_approvals(&self, now: &str) -> DbResult<u64> {
        if let Some(store) = self.postgres_agent.as_ref() {
            return store
                .expire_agent_approvals(now)
                .await
                .map_err(Self::map_postgres_agent);
        }
        let result = sqlx::query(
            "UPDATE agent_approvals SET status = 'expired'
             WHERE status = 'pending' AND expires_at < ?",
        )
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    pub async fn get_agent_tool_call(
        &self,
        session_id: &str,
        call_id: &str,
    ) -> DbResult<Option<AgentToolCallRow>> {
        if let Some(store) = self.postgres_agent.as_ref() {
            return store
                .get_agent_tool_call(session_id, call_id)
                .await
                .map(|record| record.map(Into::into))
                .map_err(Self::map_postgres_agent);
        }
        Ok(sqlx::query_as::<_, AgentToolCallRow>(
            "SELECT session_id, call_id, idempotency_key, tool_name, request_hash, status,
                    response_json, error, created_at, updated_at
             FROM agent_tool_calls WHERE session_id = ? AND call_id = ?",
        )
        .bind(session_id)
        .bind(call_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn begin_agent_tool_call(&self, row: &AgentToolCallRow) -> DbResult<bool> {
        if let Some(store) = self.postgres_agent.as_ref() {
            return store
                .begin_agent_tool_call(&AgentToolCallRecord::from(row.clone()))
                .await
                .map_err(Self::map_postgres_agent);
        }
        let result = sqlx::query(
            "INSERT INTO agent_tool_calls
             (session_id, call_id, idempotency_key, tool_name, request_hash, status,
              response_json, error, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, 'in_progress', NULL, NULL, ?, ?)
             ON CONFLICT DO NOTHING",
        )
        .bind(&row.session_id)
        .bind(&row.call_id)
        .bind(&row.idempotency_key)
        .bind(&row.tool_name)
        .bind(&row.request_hash)
        .bind(&row.created_at)
        .bind(&row.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn finish_agent_tool_call(
        &self,
        session_id: &str,
        call_id: &str,
        status: &str,
        response_json: Option<&str>,
        error: Option<&str>,
        updated_at: &str,
    ) -> DbResult<bool> {
        if let Some(store) = self.postgres_agent.as_ref() {
            return store
                .finish_agent_tool_call(
                    session_id,
                    call_id,
                    status,
                    response_json,
                    error,
                    updated_at,
                )
                .await
                .map_err(Self::map_postgres_agent);
        }
        let result = sqlx::query(
            "UPDATE agent_tool_calls
             SET status = ?, response_json = ?, error = ?, updated_at = ?
             WHERE session_id = ? AND call_id = ? AND status = 'in_progress'",
        )
        .bind(status)
        .bind(response_json)
        .bind(error)
        .bind(updated_at)
        .bind(session_id)
        .bind(call_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn list_marketplace_events(
        &self,
        after: i64,
        limit: i64,
    ) -> DbResult<Vec<MarketplaceEventRow>> {
        Ok(sqlx::query_as::<_, MarketplaceEventRow>(
            "SELECT sequence, id, at, plugin_id, version, kind, detail_json
             FROM marketplace_event_stream
             WHERE sequence > ?
             ORDER BY sequence ASC
             LIMIT ?",
        )
        .bind(after.max(0))
        .bind(limit.clamp(1, 1_000))
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn latest_marketplace_event_sequence(&self) -> DbResult<i64> {
        Ok(
            sqlx::query_scalar("SELECT COALESCE(max(sequence), 0) FROM marketplace_event_stream")
                .fetch_one(&self.pool)
                .await?,
        )
    }
}

pub fn status_text(status: ExecutionStatus) -> &'static str {
    match status {
        ExecutionStatus::Pending => "pending",
        ExecutionStatus::Running => "running",
        ExecutionStatus::WaitingApproval => "waiting_approval",
        ExecutionStatus::Succeeded => "succeeded",
        ExecutionStatus::Failed => "failed",
        ExecutionStatus::Cancelled => "cancelled",
    }
}

pub fn parse_state(row: &ExecutionRow) -> DbResult<ExecutionState> {
    Ok(serde_json::from_str(&row.state_json)?)
}

pub fn build_skill_snapshot(
    version: &SkillVersionRow,
    files: &[SkillFileRow],
) -> DbResult<SkillSnapshot> {
    let manifest = serde_json::from_str(&version.manifest_json)?;
    let manifest_present = files.iter().any(|file| file.path == "hpa.skill.json");
    let files = files
        .iter()
        .map(|file| SkillFile {
            path: file.path.clone(),
            size: file.size as u64,
            sha256: file.sha256.clone(),
            content: file.content.clone(),
        })
        .collect();
    Ok(SkillSnapshot {
        id: version.skill_id.clone(),
        name: version.name.clone(),
        version: version.version.clone(),
        license: version.license.clone(),
        manifest,
        manifest_present,
        skill_md: version.skill_md.clone(),
        files,
        content_hash: version.content_hash.clone(),
    })
}

async fn ensure_version_columns(pool: &SqlitePool) -> DbResult<()> {
    let columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('workflow_versions')")
            .fetch_all(pool)
            .await?;
    if !columns.iter().any(|column| column == "registry_version") {
        sqlx::query(
            "ALTER TABLE workflow_versions ADD COLUMN registry_version TEXT NOT NULL DEFAULT ''",
        )
        .execute(pool)
        .await?;
    }
    Ok(())
}

async fn ensure_plugin_columns(pool: &SqlitePool) -> DbResult<()> {
    let columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('mcp_servers')")
            .fetch_all(pool)
            .await?;
    if !columns.iter().any(|column| column == "plugin_id") {
        sqlx::query("ALTER TABLE mcp_servers ADD COLUMN plugin_id TEXT")
            .execute(pool)
            .await?;
    }
    if !columns.iter().any(|column| column == "plugin_version") {
        sqlx::query("ALTER TABLE mcp_servers ADD COLUMN plugin_version TEXT")
            .execute(pool)
            .await?;
    }
    if !columns.iter().any(|column| column == "enabled") {
        sqlx::query("ALTER TABLE mcp_servers ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1")
            .execute(pool)
            .await?;
    }
    let remote_columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('marketplace_remote_packages')")
            .fetch_all(pool)
            .await?;
    if !remote_columns
        .iter()
        .any(|column| column == "provenance_json")
    {
        sqlx::query("ALTER TABLE marketplace_remote_packages ADD COLUMN provenance_json TEXT")
            .execute(pool)
            .await?;
    }
    if !remote_columns.iter().any(|column| column == "oci_json") {
        sqlx::query("ALTER TABLE marketplace_remote_packages ADD COLUMN oci_json TEXT")
            .execute(pool)
            .await?;
    }
    Ok(())
}

impl Db {
    /// Replaces the trigger rows of a workflow with the published snapshot.
    /// `last_fire_at` and the last execution reference survive republication.
    pub async fn sync_triggers(
        &self,
        workflow_id: &str,
        version: Option<&str>,
        seeds: &[TriggerSeed],
        now: &str,
    ) -> DbResult<()> {
        if let Some(store) = self.postgres_trigger.as_ref() {
            let records = seeds
                .iter()
                .map(|seed| TriggerSeedRecord {
                    trigger_id: seed.trigger_id.clone(),
                    kind: seed.kind.clone(),
                    config_json: seed.config_json.clone(),
                    enabled: seed.enabled,
                    next_fire_at: seed.next_fire_at,
                })
                .collect::<Vec<_>>();
            return store
                .sync_triggers(workflow_id, version, &records, now)
                .await
                .map_err(Self::map_postgres_trigger);
        }
        let mut tx = self.pool.begin().await?;
        let existing: Vec<String> =
            sqlx::query_scalar("SELECT trigger_id FROM trigger_state WHERE workflow_id = ?")
                .bind(workflow_id)
                .fetch_all(&mut *tx)
                .await?;
        for trigger_id in existing {
            if seeds.iter().any(|seed| seed.trigger_id == trigger_id) {
                continue;
            }
            sqlx::query("DELETE FROM trigger_state WHERE workflow_id = ? AND trigger_id = ?")
                .bind(workflow_id)
                .bind(&trigger_id)
                .execute(&mut *tx)
                .await?;
        }
        for seed in seeds {
            sqlx::query(
                "INSERT INTO trigger_state
                 (workflow_id, trigger_id, kind, config_json, workflow_version, enabled,
                  next_fire_at, last_fire_at, last_execution_id, last_status, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, NULL, NULL, NULL, ?)
                 ON CONFLICT(workflow_id, trigger_id) DO UPDATE SET
                   kind = excluded.kind,
                   config_json = excluded.config_json,
                   workflow_version = excluded.workflow_version,
                   enabled = excluded.enabled,
                   next_fire_at = excluded.next_fire_at,
                   updated_at = excluded.updated_at",
            )
            .bind(workflow_id)
            .bind(&seed.trigger_id)
            .bind(&seed.kind)
            .bind(&seed.config_json)
            .bind(version)
            .bind(seed.enabled as i64)
            .bind(seed.next_fire_at)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn list_triggers(&self) -> DbResult<Vec<TriggerStateRow>> {
        if let Some(store) = self.postgres_trigger.as_ref() {
            return store
                .list_triggers()
                .await
                .map(|rows| rows.into_iter().map(Into::into).collect())
                .map_err(Self::map_postgres_trigger);
        }
        Ok(sqlx::query_as::<_, TriggerStateRow>(
            "SELECT workflow_id, trigger_id, kind, config_json, workflow_version, enabled,
                    next_fire_at, last_fire_at, last_execution_id, last_status, updated_at
             FROM trigger_state ORDER BY workflow_id, trigger_id",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn get_trigger(
        &self,
        workflow_id: &str,
        trigger_id: &str,
    ) -> DbResult<Option<TriggerStateRow>> {
        if let Some(store) = self.postgres_trigger.as_ref() {
            return store
                .get_trigger(workflow_id, trigger_id)
                .await
                .map(|row| row.map(Into::into))
                .map_err(Self::map_postgres_trigger);
        }
        Ok(sqlx::query_as::<_, TriggerStateRow>(
            "SELECT workflow_id, trigger_id, kind, config_json, workflow_version, enabled,
                    next_fire_at, last_fire_at, last_execution_id, last_status, updated_at
             FROM trigger_state WHERE workflow_id = ? AND trigger_id = ?",
        )
        .bind(workflow_id)
        .bind(trigger_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn due_triggers(&self, now_ms: i64, limit: i64) -> DbResult<Vec<TriggerStateRow>> {
        if let Some(store) = self.postgres_trigger.as_ref() {
            return store
                .due_triggers(now_ms, limit)
                .await
                .map(|rows| rows.into_iter().map(Into::into).collect())
                .map_err(Self::map_postgres_trigger);
        }
        Ok(sqlx::query_as::<_, TriggerStateRow>(
            "SELECT workflow_id, trigger_id, kind, config_json, workflow_version, enabled,
                    next_fire_at, last_fire_at, last_execution_id, last_status, updated_at
             FROM trigger_state
             WHERE enabled = 1 AND kind = 'schedule' AND next_fire_at IS NOT NULL
               AND next_fire_at <= ?
             ORDER BY next_fire_at ASC LIMIT ?",
        )
        .bind(now_ms)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Optimistically advances a schedule slot. Returns false when another tick
    /// already claimed it.
    pub async fn claim_schedule_slot(
        &self,
        workflow_id: &str,
        trigger_id: &str,
        expected_next: i64,
        now_ms: i64,
        next_fire_at: Option<i64>,
        updated_at: &str,
    ) -> DbResult<bool> {
        if let Some(store) = self.postgres_trigger.as_ref() {
            return store
                .claim_schedule_slot(
                    workflow_id,
                    trigger_id,
                    expected_next,
                    now_ms,
                    next_fire_at,
                    updated_at,
                )
                .await
                .map_err(Self::map_postgres_trigger);
        }
        let updated = sqlx::query(
            "UPDATE trigger_state
             SET next_fire_at = ?, last_fire_at = ?, updated_at = ?
             WHERE workflow_id = ? AND trigger_id = ? AND enabled = 1 AND next_fire_at = ?",
        )
        .bind(next_fire_at)
        .bind(now_ms)
        .bind(updated_at)
        .bind(workflow_id)
        .bind(trigger_id)
        .bind(expected_next)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() > 0)
    }

    pub async fn set_trigger_enabled(
        &self,
        workflow_id: &str,
        trigger_id: &str,
        enabled: bool,
        next_fire_at: Option<i64>,
        updated_at: &str,
    ) -> DbResult<bool> {
        if let Some(store) = self.postgres_trigger.as_ref() {
            return store
                .set_trigger_enabled(workflow_id, trigger_id, enabled, next_fire_at, updated_at)
                .await
                .map_err(Self::map_postgres_trigger);
        }
        let updated = sqlx::query(
            "UPDATE trigger_state SET enabled = ?, next_fire_at = ?, updated_at = ?
             WHERE workflow_id = ? AND trigger_id = ?",
        )
        .bind(enabled as i64)
        .bind(next_fire_at)
        .bind(updated_at)
        .bind(workflow_id)
        .bind(trigger_id)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() > 0)
    }

    pub async fn note_trigger_execution(
        &self,
        workflow_id: &str,
        trigger_id: &str,
        execution_id: &str,
        status: &str,
    ) -> DbResult<()> {
        if let Some(store) = self.postgres_trigger.as_ref() {
            return store
                .note_trigger_execution(workflow_id, trigger_id, execution_id, status)
                .await
                .map_err(Self::map_postgres_trigger);
        }
        sqlx::query(
            "UPDATE trigger_state SET last_execution_id = ?, last_status = ?
             WHERE workflow_id = ? AND trigger_id = ?",
        )
        .bind(execution_id)
        .bind(status)
        .bind(workflow_id)
        .bind(trigger_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn trigger_event_delivered(
        &self,
        trigger_id: &str,
        event_id: &str,
    ) -> DbResult<bool> {
        if let Some(store) = self.postgres_trigger.as_ref() {
            return store
                .trigger_event_delivered(trigger_id, event_id)
                .await
                .map_err(Self::map_postgres_trigger);
        }
        sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(
                SELECT 1 FROM trigger_fires WHERE trigger_id = ? AND event_id = ?
             )",
        )
        .bind(trigger_id)
        .bind(event_id)
        .fetch_one(&self.pool)
        .await
        .map(|value| value != 0)
        .map_err(DbError::from)
    }

    pub async fn list_trigger_fires(&self, limit: i64) -> DbResult<Vec<TriggerFireRow>> {
        if let Some(store) = self.postgres_trigger.as_ref() {
            return store
                .list_trigger_fires(limit)
                .await
                .map(|rows| rows.into_iter().map(Into::into).collect())
                .map_err(Self::map_postgres_trigger);
        }
        Ok(sqlx::query_as::<_, TriggerFireRow>(
            "SELECT id, workflow_id, trigger_id, kind, fired_at, scheduled_at, execution_id,
                    status, event_id, detail_json
             FROM trigger_fires ORDER BY fired_at DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workflow_row(ir_json: &str) -> WorkflowRow {
        WorkflowRow {
            id: "wf_frozen".to_string(),
            name: "Frozen".to_string(),
            version: "1.0.0".to_string(),
            description: None,
            published: 0,
            ir_json: ir_json.to_string(),
            yaml: String::new(),
            created_at: "2026-11-01T00:00:00Z".to_string(),
            updated_at: "2026-11-01T00:00:00Z".to_string(),
        }
    }

    #[tokio::test]
    async fn published_versions_are_frozen() {
        let path = std::env::temp_dir().join(format!("hpa-db-{}.sqlite", uuid::Uuid::now_v7()));
        let db = Db::open(&path).await.expect("open db");
        let row = workflow_row(r#"{"nodes":[]}"#);
        db.upsert_workflow(&row).await.expect("save workflow");
        db.publish_workflow(&row, "2026.11.1")
            .await
            .expect("publish version");
        db.publish_workflow(&row, "2026.12.1")
            .await
            .expect("republish identical content");

        let version = db
            .get_version(&row.id, &row.version)
            .await
            .expect("load version")
            .expect("version exists");
        assert_eq!(
            version.registry_version, "2026.11.1",
            "republishing must not rewrite the frozen registry version"
        );

        let mut changed = row.clone();
        changed.ir_json = r#"{"nodes":[{"id":"changed"}]}"#.to_string();
        let error = db
            .publish_workflow(&changed, "2026.11.1")
            .await
            .expect_err("changed content must be rejected");
        assert!(matches!(error, DbError::VersionFrozen { .. }));

        let stored = db
            .get_workflow(&row.id)
            .await
            .expect("load workflow")
            .expect("workflow exists");
        assert_eq!(stored.ir_json, row.ir_json);
        drop(db);
        let _ = std::fs::remove_file(&path);
    }
}
