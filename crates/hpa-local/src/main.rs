mod agent;
mod ai_worker_client;
mod api;
mod assets;
mod connector;
mod db;
mod driver;
mod enterprise;
mod enterprise_ops;
mod event_bus;
mod hooks;
mod mcp;
mod mcp_client;
mod mcp_http;
mod metrics;
mod oauth;
mod oauth_http;
mod observe;
mod plugin;
mod scheduler;
mod skills;
#[allow(dead_code)]
mod telemetry;
mod worker;
mod worker_daemon;
mod worker_outbox;

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::Router;
use clap::{Parser, Subcommand};
use hpa_core::ir::Workflow;
use hpa_core::runtime::{ExecutionStatus, StepStatus, new_state, start_event, step};
use hpa_core::validator;
use hpa_skill::runtime_versions;
use serde_json::{Value, json};
use tower_http::compression::CompressionLayer;

use crate::connector::{ConnectionInput, ConnectorRuntime};
use crate::db::Db;
use crate::driver::RuntimeManager;
use crate::mcp::{McpClient, McpService};
use crate::mcp_http::McpGatewayState;
use crate::plugin::{BundleOptions, PluginService};
use crate::skills::{SkillService, sandbox_label};

#[derive(Debug)]
struct CliError(String);

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for CliError {}

#[derive(Debug, Parser)]
#[command(name = "hpa", version, about = "hpa local workflow runtime")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[arg(long, global = true, default_value_t = 8787)]
    port: u16,
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Serve,
    Doctor,
    Validate {
        file: PathBuf,
    },
    Run {
        file: PathBuf,
        #[arg(long)]
        input: Option<String>,
    },
    Skill {
        #[command(subcommand)]
        command: SkillCommand,
    },
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },
    Plugin {
        #[command(subcommand)]
        command: PluginCommand,
    },
    Connector {
        #[command(subcommand)]
        command: ConnectorCommand,
    },
    Enterprise {
        #[command(subcommand)]
        command: EnterpriseCommand,
    },
    #[command(name = "db", alias = "database")]
    Db {
        #[command(subcommand)]
        command: DbCommand,
    },
    Worker {
        #[arg(long, default_value = "http://127.0.0.1:8787")]
        control_url: String,
        #[arg(long, default_value_t = 1)]
        max_concurrency: usize,
        #[arg(long, value_delimiter = ',', default_value = "workflow")]
        capabilities: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
enum EnterpriseCommand {
    Init {
        #[arg(long, default_value = "local-admin")]
        name: String,
    },
    Token {
        #[arg(long, default_value = "local")]
        tenant: String,
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "owner")]
        role: String,
        #[arg(long)]
        platform_admin: bool,
        #[arg(long)]
        expires_in_days: Option<i64>,
    },
}

#[derive(Debug, Subcommand)]
enum DbCommand {
    Migrate,
    Status,
    /// Compare the SQLite control plane with its PostgreSQL mirror.
    VerifyMirror,
    ImportSqlite {
        #[arg(long)]
        source: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum PluginCommand {
    Key,
    Keys,
    PublisherAdd {
        id: String,
        name: String,
    },
    PublisherKeyAdd {
        publisher: String,
        #[arg(long)]
        key_id: String,
        #[arg(long)]
        public_key: String,
        #[arg(long, default_value = "Publisher key")]
        label: String,
        #[arg(long)]
        valid_from: Option<String>,
        #[arg(long)]
        valid_until: Option<String>,
    },
    PublisherKeyRevoke {
        publisher: String,
        #[arg(long)]
        key_id: String,
    },
    MarketplaceSourceAdd {
        id: String,
        #[arg(long)]
        label: String,
        #[arg(long)]
        index: String,
        #[arg(long)]
        key_id: String,
        #[arg(long)]
        public_key: String,
        #[arg(long, default_value = "Marketplace index key")]
        key_label: String,
    },
    MarketplaceSourceRotate {
        id: String,
        #[arg(long)]
        key_id: String,
        #[arg(long)]
        public_key: String,
        #[arg(long, default_value = "Rotated marketplace key")]
        label: String,
    },
    MarketplaceSourceRefresh {
        id: String,
    },
    MarketplaceRemote {
        #[arg(long)]
        source: Option<String>,
    },
    MarketplaceRemoteInstall {
        source: String,
        id: String,
        version: String,
    },
    Sign {
        path: PathBuf,
    },
    Pack {
        path: PathBuf,
        output: PathBuf,
    },
    Validate {
        path: PathBuf,
        #[arg(long)]
        allow_unsigned: bool,
        #[arg(long)]
        allow_untrusted: bool,
    },
    Install {
        path: PathBuf,
        #[arg(long)]
        allow_unsigned: bool,
        #[arg(long)]
        allow_untrusted: bool,
    },
    List,
    Show {
        id: String,
    },
    Enable {
        id: String,
    },
    Disable {
        id: String,
    },
    Rollback {
        id: String,
    },
    Uninstall {
        id: String,
        #[arg(long)]
        force: bool,
    },
    Sample,
    Publish {
        path: PathBuf,
    },
    Marketplace,
    Review {
        id: String,
        version: String,
        #[arg(long)]
        status: String,
        #[arg(long)]
        note: Option<String>,
    },
    Withdraw {
        id: String,
        version: String,
    },
    Rate {
        id: String,
        version: String,
        rating: i64,
        #[arg(long)]
        comment: Option<String>,
    },
    InstallMarketplace {
        id: String,
        version: String,
    },
}

#[derive(Debug, Subcommand)]
enum ConnectorCommand {
    List,
    Connections,
    Add {
        name: String,
        connector_id: String,
        #[arg(long)]
        config: Option<String>,
        #[arg(long)]
        config_file: Option<PathBuf>,
        #[arg(long)]
        credential_env: Option<String>,
    },
    Test {
        connection: String,
    },
    Invoke {
        connection: String,
        operation: String,
        #[arg(long)]
        args: Option<String>,
    },
    Audit {
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
    Sample,
}

#[derive(Debug, Subcommand)]
enum AgentCommand {
    Import {
        file: PathBuf,
    },
    List,
    Show {
        id: String,
        #[arg(long)]
        version: Option<String>,
    },
    Publish {
        id: String,
        #[arg(long)]
        version: Option<String>,
    },
    Run {
        id: String,
        #[arg(long)]
        version: Option<String>,
        #[arg(long)]
        input: Option<String>,
    },
    Sessions {
        id: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: i64,
    },
    Session {
        id: String,
    },
    Replay {
        id: String,
    },
    Approvals {
        #[arg(long)]
        status: Option<String>,
    },
    Decide {
        id: String,
        #[arg(long)]
        decision: String,
    },
    Tools,
}

#[derive(Debug, Subcommand)]
enum McpCommand {
    Serve,
    Token {
        #[command(subcommand)]
        command: McpTokenCommand,
    },
    Probe {
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
    Call {
        #[arg(long)]
        tool: String,
        #[arg(long)]
        args: Option<String>,
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
enum McpTokenCommand {
    Create {
        name: String,
        #[arg(long, value_delimiter = ',')]
        scopes: Option<Vec<String>>,
    },
    List,
    Revoke {
        id: String,
    },
}

#[derive(Debug, Subcommand)]
enum SkillCommand {
    Import {
        path: PathBuf,
    },
    List,
    Show {
        id: String,
        #[arg(long)]
        version: Option<String>,
    },
    Validate {
        path: PathBuf,
    },
    Publish {
        id: String,
        #[arg(long)]
        version: Option<String>,
    },
    Sign {
        id: String,
        #[arg(long)]
        version: Option<String>,
    },
    Test {
        id: String,
        #[arg(long)]
        version: Option<String>,
        #[arg(long)]
        case: Option<String>,
    },
    Run {
        id: String,
        #[arg(long)]
        version: Option<String>,
        #[arg(long)]
        script: Option<String>,
        #[arg(long)]
        input: Option<String>,
    },
    Export {
        id: String,
        #[arg(long)]
        version: Option<String>,
        #[arg(long, value_enum)]
        target: Option<SkillExportTarget>,
        path: Option<PathBuf>,
    },
    Runs {
        id: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: i64,
    },
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum SkillExportTarget {
    Claude,
    Codex,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let _telemetry = hpa_local::telemetry::init("hpa-local")?;

    let cli = Cli::parse();
    let command = cli.command.unwrap_or(Command::Serve);
    match command {
        Command::Serve => serve(cli.port, resolve_data_dir(cli.data_dir)?).await?,
        Command::Doctor => doctor(resolve_data_dir(cli.data_dir)?).await?,
        Command::Validate { file } => validate_file(&file)?,
        Command::Run { file, input } => run_file(&file, input.as_deref()).await?,
        Command::Skill { command } => {
            skill_command(resolve_data_dir(cli.data_dir)?, command).await?
        }
        Command::Mcp { command } => mcp_command(resolve_data_dir(cli.data_dir)?, command).await?,
        Command::Agent { command } => {
            agent_command(resolve_data_dir(cli.data_dir)?, command).await?
        }
        Command::Plugin { command } => {
            plugin_command(resolve_data_dir(cli.data_dir)?, command).await?
        }
        Command::Connector { command } => {
            connector_command(resolve_data_dir(cli.data_dir)?, command).await?
        }
        Command::Enterprise { command } => {
            enterprise_command(resolve_data_dir(cli.data_dir)?, command).await?
        }
        Command::Db { command } => {
            database_command(command, resolve_data_dir(cli.data_dir)?).await?
        }
        Command::Worker {
            control_url,
            max_concurrency,
            capabilities,
        } => {
            let token = std::env::var("HPA_WORKER_TOKEN")
                .map_err(|_| CliError("HPA_WORKER_TOKEN is required".to_string()))?;
            if token.trim().is_empty() {
                return Err(CliError("HPA_WORKER_TOKEN is empty".to_string()).into());
            }
            if !(1..=64).contains(&max_concurrency) {
                return Err(
                    CliError("max-concurrency must be between 1 and 64".to_string()).into(),
                );
            }
            crate::worker_daemon::run_worker(crate::worker_daemon::WorkerDaemonConfig {
                control_url,
                token,
                max_concurrency: max_concurrency.max(1),
                capabilities,
                heartbeat_interval: Duration::from_secs(10),
                poll_interval: Duration::from_secs(1),
                outbox_path: resolve_data_dir(cli.data_dir)?.join("worker-outbox.db"),
            })
            .await?;
        }
    }
    Ok(())
}

async fn database_command(command: DbCommand, data_dir: PathBuf) -> Result<(), Box<dyn Error>> {
    let database_url = std::env::var("HPA_POSTGRES_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .map_err(|_| {
            CliError("HPA_POSTGRES_URL or DATABASE_URL is required for database commands".into())
        })?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await?;
    static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations/postgres");

    match command {
        DbCommand::Migrate => {
            let before = postgres_migration_count(&pool).await?;
            MIGRATOR.run(&pool).await?;
            let after = postgres_migration_count(&pool).await?;
            print_json(&json!({
                "database": "postgresql",
                "before": before,
                "after": after,
                "applied": after - before,
                "available": MIGRATOR.migrations.len(),
            }));
        }
        DbCommand::Status => {
            let installed: bool =
                sqlx::query_scalar("SELECT to_regclass('public._sqlx_migrations') IS NOT NULL")
                    .fetch_one(&pool)
                    .await?;
            let migrations = if installed {
                sqlx::query(
                    "SELECT version, description, success, installed_on
                     FROM public._sqlx_migrations
                     ORDER BY version",
                )
                .fetch_all(&pool)
                .await?
                .into_iter()
                .map(|row| {
                    use sqlx::Row;
                    json!({
                        "version": row.get::<i64, _>("version"),
                        "description": row.get::<String, _>("description"),
                        "success": row.get::<bool, _>("success"),
                        "installedOn": row
                            .get::<time::OffsetDateTime, _>("installed_on")
                            .to_string(),
                    })
                })
                .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            print_json(&json!({
                "database": "postgresql",
                "installed": installed,
                "applied": migrations.len(),
                "available": MIGRATOR.migrations.len(),
                "migrations": migrations,
            }));
        }
        DbCommand::VerifyMirror => {
            let report = verify_control_mirror(&data_dir, &pool).await?;
            let drift = report["drift"]["total"].as_i64().unwrap_or_default();
            print_json(&report);
            if drift > 0 {
                return Err(CliError(format!(
                    "control plane mirror drift detected: {drift} item(s)"
                ))
                .into());
            }
        }
        DbCommand::ImportSqlite { source } => {
            MIGRATOR.run(&pool).await?;
            let report = hpa_local::postgres_import::import_data_dir(&pool, &source).await?;
            print_json(&serde_json::to_value(report)?);
        }
    }
    pool.close().await;
    Ok(())
}

async fn postgres_migration_count(pool: &sqlx::postgres::PgPool) -> Result<i64, sqlx::Error> {
    let installed: bool =
        sqlx::query_scalar("SELECT to_regclass('public._sqlx_migrations') IS NOT NULL")
            .fetch_one(pool)
            .await?;
    if !installed {
        return Ok(0);
    }
    sqlx::query_scalar("SELECT COUNT(*) FROM public._sqlx_migrations WHERE success")
        .fetch_one(pool)
        .await
}

async fn skill_command(data_dir: PathBuf, command: SkillCommand) -> Result<(), Box<dyn Error>> {
    std::fs::create_dir_all(&data_dir)?;
    let db = Db::open(&data_dir.join("hpa.db")).await?;
    let service = SkillService::new(db.clone(), data_dir);
    match command {
        SkillCommand::Import { path } => {
            let outcome = service
                .import(&path)
                .await
                .map_err(|error| CliError(error.to_string()))?;
            print_json(&json!({
                "inserted": outcome.inserted,
                "summary": outcome.summary,
                "version": outcome.version.version,
                "warnings": outcome.report.issues,
            }));
        }
        SkillCommand::List => {
            let skills = db.list_skills().await?;
            print_json(&json!({
                "skills": skills,
                "runtimes": runtime_versions(),
                "sandbox": sandbox_label(hpa_skill::detected_sandbox()),
            }));
        }
        SkillCommand::Show { id, version } => {
            let loaded = service
                .load(&id, version.as_deref())
                .await
                .map_err(|error| CliError(error.to_string()))?;
            print_json(&loaded_skill_json(&loaded));
        }
        SkillCommand::Validate { path } => {
            let inspection =
                hpa_skill::inspect(&path).map_err(|error| CliError(error.to_string()))?;
            print_json(&json!({
                "valid": inspection.report.is_valid(),
                "report": inspection.report,
                "skill": inspection.snapshot.id,
                "version": inspection.snapshot.version,
                "contentHash": inspection.snapshot.content_hash,
            }));
            if !inspection.report.is_valid() {
                return Err(Box::new(CliError("skill validation failed".to_string())));
            }
        }
        SkillCommand::Publish { id, version } => {
            let published = service
                .publish(&id, version.as_deref())
                .await
                .map_err(|error| CliError(error.to_string()))?;
            print_json(&json!({"published": published}));
        }
        SkillCommand::Sign { id, version } => {
            let signature = service
                .sign(&id, version.as_deref())
                .await
                .map_err(|error| CliError(error.to_string()))?;
            print_json(&json!({"signature": signature}));
        }
        SkillCommand::Test { id, version, case } => {
            let results = service
                .test(&id, version.as_deref(), case.as_deref())
                .await
                .map_err(|error| CliError(error.to_string()))?;
            let passed = results.iter().all(|result| result.passed);
            print_json(&json!({"passed": passed, "results": results}));
            if !passed {
                return Err(Box::new(CliError("skill tests failed".to_string())));
            }
        }
        SkillCommand::Run {
            id,
            version,
            script,
            input,
        } => {
            let input = serde_json::from_str::<Value>(input.as_deref().unwrap_or("{}"))?;
            let outcome = service
                .run(&id, version.as_deref(), script.as_deref(), input)
                .await
                .map_err(|error| CliError(error.to_string()))?;
            let artifacts = db.list_skill_run_artifacts(&outcome.id).await?;
            print_json(&json!({"run": outcome, "artifacts": artifacts}));
            if !outcome.succeeded() {
                return Err(Box::new(CliError("skill run failed".to_string())));
            }
        }
        SkillCommand::Export {
            id,
            version,
            target,
            path,
        } => {
            let target_path =
                skill_export_path(&service, &id, version.as_deref(), target, path).await?;
            service
                .export(&id, version.as_deref(), &target_path)
                .await
                .map_err(|error| CliError(error.to_string()))?;
            print_json(&json!({"exported": true, "path": target_path}));
        }
        SkillCommand::Runs { id, limit } => {
            let runs = db.list_skill_runs(id.as_deref(), limit).await?;
            print_json(&json!({"runs": runs}));
        }
    }
    Ok(())
}

async fn skill_export_path(
    service: &SkillService,
    id: &str,
    version: Option<&str>,
    target: Option<SkillExportTarget>,
    path: Option<PathBuf>,
) -> Result<PathBuf, Box<dyn Error>> {
    if let Some(path) = path {
        return Ok(path);
    }
    let Some(target) = target else {
        return Err(Box::new(CliError(
            "provide an export path or --target claude|codex".to_string(),
        )));
    };
    let loaded = service
        .load(id, version)
        .await
        .map_err(|error| CliError(error.to_string()))?;
    let name = loaded.snapshot.name.clone();
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| CliError("HOME is not set".to_string()))?;
    let target_path = match target {
        SkillExportTarget::Claude => std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".claude"))
            .join("skills")
            .join(name),
        SkillExportTarget::Codex => match std::env::var_os("CODEX_HOME").map(PathBuf::from) {
            Some(codex_home) => codex_home.join("skills").join(name),
            None => home.join(".agents").join("skills").join(name),
        },
    };
    Ok(target_path)
}

fn loaded_skill_json(loaded: &crate::skills::LoadedSkill) -> Value {
    json!({
        "skill": {
            "id": loaded.snapshot.id,
            "name": loaded.snapshot.name,
            "version": loaded.snapshot.version,
            "license": loaded.snapshot.license,
            "status": loaded.version.status,
            "contentHash": loaded.snapshot.content_hash,
        },
        "manifest": loaded.snapshot.manifest,
        "skillMd": loaded.snapshot.skill_md,
        "report": loaded.report,
        "scripts": loaded.manifest.scripts,
        "tests": loaded.manifest.tests,
        "permissions": loaded.manifest.permissions,
        "entry": loaded.manifest.entry.kind(),
        "dependencies": loaded.manifest.dependencies,
        "files": loaded.snapshot.files.iter().map(|file| json!({
            "path": file.path,
            "size": file.size,
            "sha256": file.sha256,
        })).collect::<Vec<_>>(),
        "signature": loaded.signature,
    })
}

fn print_json(value: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string())
    );
}

async fn mcp_command(data_dir: PathBuf, command: McpCommand) -> Result<(), Box<dyn Error>> {
    std::fs::create_dir_all(&data_dir)?;
    let db = Db::open(&data_dir.join("hpa.db")).await?;
    let service = SkillService::new(db, data_dir);
    match command {
        McpCommand::Serve => {
            let db = service.db.clone();
            let data_dir = service.data_dir.clone();
            let runtime = RuntimeManager::new(db.clone());
            runtime.recover().await?;
            let auth = mcp::stdio_auth(&db, &data_dir).await?;
            mcp::serve_stdio(McpService::new(service, runtime), auth).await?;
        }
        McpCommand::Token { command } => {
            let db = service.db.clone();
            match command {
                McpTokenCommand::Create { name, scopes } => {
                    let scopes = scopes.unwrap_or_else(|| {
                        vec![mcp::SCOPE_READ.to_string(), mcp::SCOPE_EXECUTE.to_string()]
                    });
                    for scope in &scopes {
                        if !matches!(scope.as_str(), mcp::SCOPE_READ | mcp::SCOPE_EXECUTE) {
                            return Err(Box::new(CliError(format!("unknown MCP scope `{scope}`"))));
                        }
                    }
                    let (token, row) =
                        mcp::create_pat(&db, &name, &scopes, "local", &mcp::now_rfc3339())
                            .await
                            .map_err(|error| CliError(error.to_string()))?;
                    print_json(&json!({
                        "token": token,
                        "id": row.id,
                        "name": row.name,
                        "scopes": scopes,
                        "notice": "store this token now; it is not shown again",
                    }));
                }
                McpTokenCommand::List => {
                    let rows = db
                        .list_mcp_tokens()
                        .await
                        .map_err(|error| CliError(error.to_string()))?;
                    let tokens = rows
                        .iter()
                        .map(|row| {
                            json!({
                                "id": row.id,
                                "name": row.name,
                                "scopes": mcp::token_scopes(row),
                                "tenantId": row.tenant_id,
                                "createdAt": row.created_at,
                                "lastUsedAt": row.last_used_at,
                                "revokedAt": row.revoked_at,
                            })
                        })
                        .collect::<Vec<_>>();
                    print_json(&json!({"tokens": tokens}));
                }
                McpTokenCommand::Revoke { id } => {
                    let revoked = db
                        .revoke_mcp_token(&id, &mcp::now_rfc3339())
                        .await
                        .map_err(|error| CliError(error.to_string()))?;
                    print_json(&json!({"revoked": revoked}));
                }
            }
        }
        McpCommand::Probe { command } => {
            let mut client = McpClient::spawn(&command).map_err(CliError)?;
            let initialized = client.initialize().map_err(CliError)?;
            let tools = client.list_tools().map_err(CliError)?;
            print_json(&json!({
                "server": initialized,
                "tools": tools.get("tools").cloned().unwrap_or_else(|| json!([])),
            }));
        }
        McpCommand::Call {
            tool,
            args,
            command,
        } => {
            let arguments = serde_json::from_str::<Value>(args.as_deref().unwrap_or("{}"))?;
            let mut client = McpClient::spawn(&command).map_err(CliError)?;
            client.initialize().map_err(CliError)?;
            let result = client.call_tool(&tool, arguments).map_err(CliError)?;
            print_json(&result);
            if result.get("isError") == Some(&json!(true)) {
                return Err(Box::new(CliError("MCP tool call failed".to_string())));
            }
        }
    }
    Ok(())
}

async fn agent_command(data_dir: PathBuf, command: AgentCommand) -> Result<(), Box<dyn Error>> {
    std::fs::create_dir_all(&data_dir)?;
    let db = Db::open(&data_dir.join("hpa.db")).await?;
    let runtime = RuntimeManager::new(db.clone());
    let connectors = ConnectorRuntime::new(db.clone());
    connectors.initialize().await?;
    let skills = SkillService::new(db.clone(), data_dir);
    let agents = crate::agent::AgentRuntime::new(skills, runtime, connectors);
    match command {
        AgentCommand::Import { file } => {
            let content = std::fs::read_to_string(&file)?;
            let config: Value = serde_json::from_str(&content)?;
            let (definition, content_hash) = crate::agent::canonical_agent_value(&config)
                .map_err(|issues| CliError(format!("agent is invalid: {}", issues.join("; "))))?;
            let created_at = now_rfc3339();
            let row = crate::db::AgentVersionRow {
                agent_id: definition.id.clone(),
                version: definition.version.clone(),
                name: definition.name.clone(),
                description: definition.description.clone(),
                config_json: serde_json::to_string(&definition)?,
                content_hash: content_hash.clone(),
                status: "imported".to_string(),
                created_at,
                published_at: None,
            };
            let inserted = db
                .upsert_agent_version(&row)
                .await
                .map_err(|error| CliError(error.to_string()))?;
            print_json(&json!({
                "inserted": inserted,
                "agentId": definition.id,
                "version": definition.version,
                "contentHash": content_hash,
            }));
        }
        AgentCommand::List => {
            let agents = db.list_agents().await?;
            print_json(&json!({
                "agents": agents.iter().map(crate::agent::agent_summary_value).collect::<Vec<_>>(),
            }));
        }
        AgentCommand::Show { id, version } => {
            let summary = db
                .get_agent(&id)
                .await?
                .ok_or_else(|| CliError(format!("agent `{id}` not found")))?;
            let versions = db.list_agent_versions(&id).await?;
            let selected = match version {
                Some(version) => versions.iter().find(|row| row.version == version),
                None => versions.first(),
            }
            .ok_or_else(|| CliError(format!("agent `{id}` has no stored version")))?;
            print_json(&json!({
                "summary": crate::agent::agent_summary_value(&summary),
                "version": crate::agent::agent_version_value(selected),
                "versions": versions.iter().map(crate::agent::agent_version_value).collect::<Vec<_>>(),
                "config": serde_json::from_str::<Value>(&selected.config_json)?,
            }));
        }
        AgentCommand::Publish { id, version } => {
            let summary = db
                .get_agent(&id)
                .await?
                .ok_or_else(|| CliError(format!("agent `{id}` not found")))?;
            let version = version.unwrap_or(summary.latest_version);
            let row = db
                .get_agent_version(&id, &version)
                .await?
                .ok_or_else(|| CliError(format!("agent `{id}@{version}` not found")))?;
            let config: Value = serde_json::from_str(&row.config_json)?;
            let (_, hash) = crate::agent::canonical_agent_value(&config)
                .map_err(|issues| CliError(format!("agent is invalid: {}", issues.join("; "))))?;
            if hash != row.content_hash {
                return Err(Box::new(CliError(
                    "stored agent content hash does not match".to_string(),
                )));
            }
            let published = db
                .publish_agent_version(&id, &version, &now_rfc3339())
                .await?;
            let stored = db
                .get_agent_version(&id, &version)
                .await?
                .ok_or_else(|| CliError(format!("agent `{id}@{version}` not found")))?;
            print_json(&json!({
                "published": published || stored.status == "published",
                "version": crate::agent::agent_version_value(&stored),
            }));
        }
        AgentCommand::Run { id, version, input } => {
            let input = serde_json::from_str::<Value>(input.as_deref().unwrap_or("{}"))?;
            let session = agents
                .start(crate::agent::AgentStart {
                    agent_id: id,
                    version,
                    input,
                    parent_session_id: None,
                    depth: 0,
                    limit_override: None,
                })
                .await
                .map_err(|error| CliError(error.to_string()))?;
            print_json(&agent_session_json(&db, &session).await?);
            if session.status == "waiting_approval" {
                std::process::exit(2);
            }
            if !matches!(session.status.as_str(), "succeeded") {
                std::process::exit(1);
            }
        }
        AgentCommand::Sessions { id, limit } => {
            let rows = db.list_agent_sessions(id.as_deref(), limit).await?;
            let mut sessions = Vec::with_capacity(rows.len());
            for row in &rows {
                sessions.push(
                    crate::agent::agent_session_summary(row)
                        .map_err(|error| CliError(error.to_string()))?,
                );
            }
            print_json(&json!({"sessions": sessions}));
        }
        AgentCommand::Session { id } => {
            let row = db
                .get_agent_session(&id)
                .await?
                .ok_or_else(|| CliError(format!("agent session `{id}` not found")))?;
            print_json(&agent_session_json(&db, &row).await?);
        }
        AgentCommand::Replay { id } => {
            let replay = agents
                .replay(&id)
                .await
                .map_err(|error| CliError(error.to_string()))?;
            print_json(&replay);
            if replay.get("matches") != Some(&json!(true)) {
                return Err(Box::new(CliError(
                    "deterministic replay did not match".to_string(),
                )));
            }
        }
        AgentCommand::Approvals { status } => {
            let rows = db.list_agent_approvals(status.as_deref(), 50).await?;
            print_json(&json!({
                "approvals": rows.iter().map(crate::agent::approval_value).collect::<Vec<_>>(),
            }));
        }
        AgentCommand::Decide { id, decision } => {
            if !matches!(decision.as_str(), "approved" | "rejected") {
                return Err(Box::new(CliError(
                    "decision must be approved or rejected".to_string(),
                )));
            }
            let row = db
                .get_agent_approval(&id)
                .await?
                .ok_or_else(|| CliError(format!("agent approval `{id}` not found")))?;
            if row.status != "pending" {
                return Err(Box::new(CliError(format!(
                    "approval is already `{}`",
                    row.status
                ))));
            }
            if !db
                .decide_agent_approval(&id, &decision, &now_rfc3339())
                .await?
            {
                return Err(Box::new(CliError(
                    "approval was already decided".to_string(),
                )));
            }
            let session = agents
                .resume_approval(&row.session_id, &decision)
                .await
                .map_err(|error| CliError(error.to_string()))?;
            print_json(&agent_session_json(&db, &session).await?);
        }
        AgentCommand::Tools => {
            print_json(
                &agents
                    .tool_catalog()
                    .await
                    .map_err(|error| CliError(error.to_string()))?,
            );
        }
    }
    Ok(())
}

async fn agent_session_json(
    db: &Db,
    row: &crate::db::AgentSessionRow,
) -> Result<Value, Box<dyn Error>> {
    let events = db.list_agent_events(&row.id, 1_000).await?;
    let mut value =
        crate::agent::agent_session_value(row).map_err(|error| CliError(error.to_string()))?;
    value["events"] = Value::Array(events.iter().map(crate::agent::event_value).collect());
    Ok(value)
}

async fn plugin_command(data_dir: PathBuf, command: PluginCommand) -> Result<(), Box<dyn Error>> {
    std::fs::create_dir_all(&data_dir)?;
    let db = Db::open(&data_dir.join("hpa.db")).await?;
    let connectors = ConnectorRuntime::new(db.clone());
    connectors.initialize().await?;
    let skills = SkillService::new(db.clone(), data_dir.clone());
    let plugins = PluginService::new(db, data_dir, skills, connectors);
    match command {
        PluginCommand::Key => print_json(
            &plugins
                .key_info()
                .await
                .map_err(|error| CliError(error.to_string()))?,
        ),
        PluginCommand::Keys => print_json(&json!({
            "keys": plugins
                .trusted_keys()
                .await
                .map_err(|error| CliError(error.to_string()))?,
        })),
        PluginCommand::PublisherAdd { id, name } => print_json(
            &plugins
                .add_publisher(&id, &name)
                .await
                .map_err(|error| CliError(error.to_string()))?,
        ),
        PluginCommand::PublisherKeyAdd {
            publisher,
            key_id,
            public_key,
            label,
            valid_from,
            valid_until,
        } => print_json(
            &plugins
                .add_publisher_key(
                    &publisher,
                    &key_id,
                    &public_key,
                    &label,
                    valid_from.as_deref(),
                    valid_until.as_deref(),
                )
                .await
                .map_err(|error| CliError(error.to_string()))?,
        ),
        PluginCommand::PublisherKeyRevoke { publisher, key_id } => print_json(
            &plugins
                .revoke_publisher_key(&publisher, &key_id)
                .await
                .map_err(|error| CliError(error.to_string()))?,
        ),
        PluginCommand::MarketplaceSourceAdd {
            id,
            label,
            index,
            key_id,
            public_key,
            key_label,
        } => print_json(
            &plugins
                .add_marketplace_source(&id, &label, &index, &key_id, &public_key, &key_label)
                .await
                .map_err(|error| CliError(error.to_string()))?,
        ),
        PluginCommand::MarketplaceSourceRotate {
            id,
            key_id,
            public_key,
            label,
        } => print_json(
            &plugins
                .rotate_marketplace_source_key(&id, &key_id, &public_key, &label)
                .await
                .map_err(|error| CliError(error.to_string()))?,
        ),
        PluginCommand::MarketplaceSourceRefresh { id } => print_json(
            &plugins
                .refresh_marketplace_source(&id)
                .await
                .map_err(|error| CliError(error.to_string()))?,
        ),
        PluginCommand::MarketplaceRemote { source } => print_json(&json!({
            "packages": plugins
                .remote_marketplace_packages(source.as_deref())
                .await
                .map_err(|error| CliError(error.to_string()))?,
        })),
        PluginCommand::MarketplaceRemoteInstall {
            source,
            id,
            version,
        } => print_json(
            &plugins
                .install_remote_package(&source, &id, &version)
                .await
                .map_err(|error| CliError(error.to_string()))?,
        ),
        PluginCommand::Sign { path } => print_json(
            &plugins
                .sign_directory(&path)
                .await
                .map_err(|error| CliError(error.to_string()))?,
        ),
        PluginCommand::Pack { path, output } => print_json(
            &plugins
                .pack_directory(&path, &output)
                .await
                .map_err(|error| CliError(error.to_string()))?,
        ),
        PluginCommand::Validate {
            path,
            allow_unsigned,
            allow_untrusted,
        } => print_json(
            &plugins
                .inspect(
                    &path,
                    BundleOptions {
                        allow_unsigned,
                        allow_untrusted,
                        require_signature: false,
                    },
                )
                .await
                .map_err(|error| CliError(error.to_string()))?,
        ),
        PluginCommand::Install {
            path,
            allow_unsigned,
            allow_untrusted,
        } => print_json(
            &plugins
                .install(
                    &path,
                    BundleOptions {
                        allow_unsigned,
                        allow_untrusted,
                        require_signature: false,
                    },
                )
                .await
                .map_err(|error| CliError(error.to_string()))?,
        ),
        PluginCommand::List => print_json(&json!({
            "plugins": plugins
                .list()
                .await
                .map_err(|error| CliError(error.to_string()))?,
        })),
        PluginCommand::Show { id } => print_json(
            &plugins
                .detail(&id)
                .await
                .map_err(|error| CliError(error.to_string()))?,
        ),
        PluginCommand::Enable { id } => print_json(
            &plugins
                .enable(&id)
                .await
                .map_err(|error| CliError(error.to_string()))?,
        ),
        PluginCommand::Disable { id } => print_json(
            &plugins
                .disable(&id)
                .await
                .map_err(|error| CliError(error.to_string()))?,
        ),
        PluginCommand::Rollback { id } => print_json(
            &plugins
                .rollback(&id)
                .await
                .map_err(|error| CliError(error.to_string()))?,
        ),
        PluginCommand::Uninstall { id, force } => print_json(
            &plugins
                .uninstall(&id, force)
                .await
                .map_err(|error| CliError(error.to_string()))?,
        ),
        PluginCommand::Sample => print_json(
            &plugins
                .create_sample()
                .await
                .map_err(|error| CliError(error.to_string()))?,
        ),
        PluginCommand::Publish { path } => print_json(
            &plugins
                .publish_marketplace(
                    &path,
                    BundleOptions {
                        allow_unsigned: false,
                        allow_untrusted: true,
                        require_signature: true,
                    },
                )
                .await
                .map_err(|error| CliError(error.to_string()))?,
        ),
        PluginCommand::Marketplace => print_json(&json!({
            "plugins": plugins
                .marketplace_list()
                .await
                .map_err(|error| CliError(error.to_string()))?,
        })),
        PluginCommand::Review {
            id,
            version,
            status,
            note,
        } => print_json(
            &plugins
                .set_marketplace_listing(&id, &version, &status, note.as_deref())
                .await
                .map_err(|error| CliError(error.to_string()))?,
        ),
        PluginCommand::Withdraw { id, version } => print_json(
            &plugins
                .withdraw_marketplace(&id, &version)
                .await
                .map_err(|error| CliError(error.to_string()))?,
        ),
        PluginCommand::Rate {
            id,
            version,
            rating,
            comment,
        } => print_json(
            &plugins
                .rate_marketplace(&id, &version, rating, comment.as_deref())
                .await
                .map_err(|error| CliError(error.to_string()))?,
        ),
        PluginCommand::InstallMarketplace { id, version } => print_json(
            &plugins
                .install_marketplace(&id, &version)
                .await
                .map_err(|error| CliError(error.to_string()))?,
        ),
    }
    Ok(())
}

async fn connector_command(
    data_dir: PathBuf,
    command: ConnectorCommand,
) -> Result<(), Box<dyn Error>> {
    std::fs::create_dir_all(&data_dir)?;
    let db = Db::open(&data_dir.join("hpa.db")).await?;
    let connectors = ConnectorRuntime::new(db);
    connectors.initialize().await?;
    match command {
        ConnectorCommand::List => {
            let rows = connectors
                .list_definitions()
                .await
                .map_err(|error| CliError(error.to_string()))?;
            let mut definitions = Vec::new();
            for row in &rows {
                let mut value = connector::definition_value(row);
                value["configShape"] = match row.kind.as_str() {
                    "http" => json!({
                        "baseUrl": "http://127.0.0.1:8787",
                        "allowedHosts": [],
                        "auth": {"kind": "none"}
                    }),
                    "database" => json!({
                        "allowedHosts": ["db.internal:5432"],
                        "statementTimeoutMs": 15000,
                        "maxRows": 500
                    }),
                    "git" => json!({
                        "repositoryRoot": "/absolute/git/repository",
                        "allowWrite": false
                    }),
                    "mq" => json!({
                        "url": "redis://127.0.0.1:6379",
                        "allowedHosts": ["127.0.0.1"],
                        "allowedChannels": ["hpa.*"],
                        "database": 0
                    }),
                    "s3" => json!({
                        "endpoint": "https://s3.example.com",
                        "region": "us-east-1",
                        "bucket": "hpa-artifacts",
                        "allowedHosts": ["s3.example.com"]
                    }),
                    _ => json!({"root": "/absolute/workspace/path", "allowWrite": false}),
                };
                definitions.push(value);
            }
            print_json(&json!({"connectors": definitions}));
        }
        ConnectorCommand::Connections => {
            let rows = connectors
                .list_connections()
                .await
                .map_err(|error| CliError(error.to_string()))?;
            let mut values = Vec::new();
            for row in &rows {
                let definition = connectors
                    .definition(&row.connector_id, Some(&row.connector_version))
                    .await
                    .ok();
                values.push(connector::connection_value(row, definition.as_ref()));
            }
            print_json(&json!({"connections": values}));
        }
        ConnectorCommand::Add {
            name,
            connector_id,
            config,
            config_file,
            credential_env,
        } => {
            let config = match (config, config_file) {
                (Some(_), Some(_)) => {
                    return Err(Box::new(CliError(
                        "provide --config or --config-file, not both".to_string(),
                    )));
                }
                (Some(config), None) => serde_json::from_str::<Value>(&config)?,
                (None, Some(path)) => {
                    serde_json::from_str::<Value>(&std::fs::read_to_string(path)?)?
                }
                (None, None) => json!({}),
            };
            let row = connectors
                .save_connection(ConnectionInput {
                    id: None,
                    name,
                    connector_id,
                    connector_version: None,
                    config,
                    credential_env,
                })
                .await
                .map_err(|error| CliError(error.to_string()))?;
            let definition = connectors
                .definition(&row.connector_id, Some(&row.connector_version))
                .await
                .ok();
            print_json(&connector::connection_value(&row, definition.as_ref()));
        }
        ConnectorCommand::Test { connection } => print_json(
            &json!({"test": connectors.test_connection(&connection).await
                .map_err(|error| CliError(error.to_string()))?}),
        ),
        ConnectorCommand::Invoke {
            connection,
            operation,
            args,
        } => {
            let arguments = serde_json::from_str::<Value>(args.as_deref().unwrap_or("{}"))?;
            print_json(
                &connectors
                    .invoke(&connection, &operation, arguments)
                    .await
                    .map_err(|error| CliError(error.to_string()))?,
            );
        }
        ConnectorCommand::Audit { limit } => print_json(&json!({
            "entries": connectors.list_audit(limit).await
                .map_err(|error| CliError(error.to_string()))?,
        })),
        ConnectorCommand::Sample => {
            let existing = connectors
                .list_connections()
                .await
                .map_err(|error| CliError(error.to_string()))?
                .into_iter()
                .find(|connection| connection.name == "Sample file workspace");
            if let Some(row) = existing {
                let definition = connectors
                    .definition(&row.connector_id, Some(&row.connector_version))
                    .await
                    .ok();
                print_json(&connector::connection_value(&row, definition.as_ref()));
            } else {
                let root = connector::seed_connection_workspace(&data_dir)
                    .await
                    .map_err(|error| CliError(error.to_string()))?;
                let row = connectors
                    .save_connection(ConnectionInput {
                        id: None,
                        name: "Sample file workspace".to_string(),
                        connector_id: "file".to_string(),
                        connector_version: Some("1.0.0".to_string()),
                        config: json!({"root": root, "allowWrite": true}),
                        credential_env: None,
                    })
                    .await
                    .map_err(|error| CliError(error.to_string()))?;
                let definition = connectors
                    .definition(&row.connector_id, Some(&row.connector_version))
                    .await
                    .ok();
                print_json(&connector::connection_value(&row, definition.as_ref()));
            }
        }
    }
    Ok(())
}

/// PostgreSQL pool for CLI control-plane commands, when one is configured.
async fn enterprise_cli_postgres_pool() -> Result<Option<sqlx::postgres::PgPool>, Box<dyn Error>> {
    let Ok(database_url) =
        std::env::var("HPA_POSTGRES_URL").or_else(|_| std::env::var("DATABASE_URL"))
    else {
        return Ok(None);
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await?;
    let installed: bool =
        sqlx::query_scalar("SELECT to_regclass('hpa_control.tokens') IS NOT NULL")
            .fetch_one(&pool)
            .await?;
    if !installed {
        return Err(CliError(
            "PostgreSQL control schema is missing; run `hpa db migrate` first".to_string(),
        )
        .into());
    }
    Ok(Some(pool))
}

/// Compares the SQLite control plane with its PostgreSQL mirror.
async fn verify_control_mirror(
    data_dir: &Path,
    pool: &sqlx::postgres::PgPool,
) -> Result<Value, Box<dyn Error>> {
    use hpa_local::postgres_control::PostgresControlStore;
    use sqlx::Row;

    let control_db = data_dir.join("control.db");
    if !control_db.exists() {
        return Err(CliError(format!(
            "control database not found: {}",
            control_db.display()
        ))
        .into());
    }
    let sqlite = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&format!("sqlite://{}", control_db.display()))
        .await?;
    let store = PostgresControlStore::new(pool.clone());

    let mut drift: Vec<Value> = Vec::new();

    // Tenants: existence and status must agree.
    let remote_tenants = store.list_tenants().await?;
    let remote_tenant_index = remote_tenants
        .iter()
        .map(|tenant| (tenant.id.as_str(), tenant.status.as_str()))
        .collect::<std::collections::HashMap<_, _>>();
    let local_tenants = sqlx::query("SELECT id, status FROM enterprise_tenants")
        .fetch_all(&sqlite)
        .await?;
    let mut local_tenant_ids = std::collections::HashSet::new();
    for row in &local_tenants {
        let id: String = row.get("id");
        let status: String = row.get("status");
        local_tenant_ids.insert(id.clone());
        match remote_tenant_index.get(id.as_str()) {
            None => drift.push(json!({"kind": "tenant.missingInPostgres", "id": id})),
            Some(remote_status) if *remote_status != status => drift.push(json!({
                "kind": "tenant.statusMismatch",
                "id": id,
                "sqlite": status,
                "postgres": remote_status,
            })),
            Some(_) => {}
        }
    }
    for tenant in &remote_tenants {
        if !local_tenant_ids.contains(&tenant.id) {
            drift.push(json!({"kind": "tenant.extraInPostgres", "id": tenant.id}));
        }
    }

    // Quotas: every field must agree.
    let local_quotas = sqlx::query(
        "SELECT tenant_id, max_workflows, max_executions_per_day, max_skills, max_agents,
                max_plugins, max_connections, max_concurrent_runs FROM enterprise_quotas",
    )
    .fetch_all(&sqlite)
    .await?;
    for row in &local_quotas {
        let tenant_id: String = row.get("tenant_id");
        let local = json!({
            "maxWorkflows": row.get::<i64, _>("max_workflows"),
            "maxExecutionsPerDay": row.get::<i64, _>("max_executions_per_day"),
            "maxSkills": row.get::<i64, _>("max_skills"),
            "maxAgents": row.get::<i64, _>("max_agents"),
            "maxPlugins": row.get::<i64, _>("max_plugins"),
            "maxConnections": row.get::<i64, _>("max_connections"),
            "maxConcurrentRuns": row.get::<i64, _>("max_concurrent_runs"),
        });
        match store.quota(&tenant_id).await? {
            None => drift.push(json!({"kind": "quota.missingInPostgres", "tenantId": tenant_id})),
            Some(remote) => {
                let remote = json!({
                    "maxWorkflows": remote.max_workflows,
                    "maxExecutionsPerDay": remote.max_executions_per_day,
                    "maxSkills": remote.max_skills,
                    "maxAgents": remote.max_agents,
                    "maxPlugins": remote.max_plugins,
                    "maxConnections": remote.max_connections,
                    "maxConcurrentRuns": remote.max_concurrent_runs,
                });
                if remote != local {
                    drift.push(json!({
                        "kind": "quota.mismatch",
                        "tenantId": tenant_id,
                        "sqlite": local,
                        "postgres": remote,
                    }));
                }
            }
        }
    }

    // Members: existence plus the fields that drive authorization.
    let remote_members = store.list_members(None).await?;
    let remote_member_index = remote_members
        .iter()
        .map(|member| (member.id.as_str(), member))
        .collect::<std::collections::HashMap<_, _>>();
    let local_members =
        sqlx::query("SELECT id, tenant_id, subject, role, status, email FROM enterprise_members")
            .fetch_all(&sqlite)
            .await?;
    let mut local_member_ids = std::collections::HashSet::new();
    for row in &local_members {
        let id: String = row.get("id");
        let role: String = row.get("role");
        let status: String = row.get("status");
        local_member_ids.insert(id.clone());
        match remote_member_index.get(id.as_str()) {
            None => drift.push(json!({"kind": "member.missingInPostgres", "id": id})),
            Some(remote) => {
                if remote.role != role || remote.status != status {
                    drift.push(json!({
                        "kind": "member.attrMismatch",
                        "id": id,
                        "sqlite": {"role": role, "status": status},
                        "postgres": {"role": remote.role, "status": remote.status},
                    }));
                }
            }
        }
    }
    for member in &remote_members {
        if !local_member_ids.contains(&member.id) {
            drift.push(json!({"kind": "member.extraInPostgres", "id": member.id}));
        }
    }

    let total = drift.len();
    sqlite.close().await;
    Ok(json!({
        "database": "postgresql",
        "counts": {
            "tenants": {"sqlite": local_tenants.len(), "postgres": remote_tenants.len()},
            "quotas": {"sqlite": local_quotas.len()},
            "members": {"sqlite": local_members.len(), "postgres": remote_members.len()},
        },
        "drift": {"total": total, "items": drift},
    }))
}

async fn enterprise_command(
    data_dir: PathBuf,
    command: EnterpriseCommand,
) -> Result<(), Box<dyn Error>> {
    std::fs::create_dir_all(&data_dir)?;
    let mut state = crate::enterprise::EnterpriseState::new(
        data_dir,
        "http://127.0.0.1:8787".to_string(),
        false,
    )
    .await?;
    // Bootstrap tokens are written to PostgreSQL when a database is configured,
    // otherwise `enterprise init` would create a node-local token that the
    // PostgreSQL-backed control plane cannot authenticate.
    if let Some(pool) = enterprise_cli_postgres_pool().await? {
        state = state.with_postgres_execution(pool);
    }
    match command {
        EnterpriseCommand::Init { name } => {
            let token = state
                .create_bootstrap_token(&name)
                .await
                .map_err(|error| CliError(error.message))?;
            print_json(&token);
        }
        EnterpriseCommand::Token {
            tenant,
            name,
            role,
            platform_admin,
            expires_in_days,
        } => {
            let token = state
                .create_token(&tenant, &name, &role, platform_admin, expires_in_days)
                .await
                .map_err(|error| CliError(error.message))?;
            print_json(&token);
        }
    }
    Ok(())
}

fn now_rfc3339() -> String {
    use time::OffsetDateTime;
    use time::format_description::well_known::Rfc3339;
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default()
}

async fn serve(port: u16, data_dir: PathBuf) -> Result<(), Box<dyn Error>> {
    std::fs::create_dir_all(&data_dir)?;
    let base_url = format!("http://127.0.0.1:{port}");
    let node_id = std::env::var("HPA_NODE_ID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("node:{base_url}"));
    let require_auth = std::env::var("HPA_ENTERPRISE_MODE")
        .map(|value| matches!(value.as_str(), "1" | "true" | "required"))
        .unwrap_or(false);
    let mut enterprise =
        enterprise::EnterpriseState::new(data_dir.clone(), base_url.clone(), require_auth).await?;
    let postgres_execution = std::env::var("HPA_POSTGRES_RUNTIME")
        .map(|value| matches!(value.as_str(), "1" | "true"))
        .unwrap_or(false);
    let postgres_event_bus = std::env::var("HPA_POSTGRES_EVENT_BUS")
        .map(|value| matches!(value.as_str(), "1" | "true"))
        .unwrap_or(false);
    let postgres_event_bus_consumer = std::env::var("HPA_EVENT_BUS_CONSUMER")
        .map(|value| !matches!(value.as_str(), "0" | "false"))
        .unwrap_or(true);
    if postgres_event_bus && !postgres_execution {
        return Err(CliError(
            "HPA_POSTGRES_EVENT_BUS=1 requires HPA_POSTGRES_RUNTIME=1".to_string(),
        )
        .into());
    }
    let postgres_mcp = std::env::var("HPA_POSTGRES_MCP")
        .map(|value| matches!(value.as_str(), "1" | "true"))
        .unwrap_or(false);
    let postgres_scheduler = std::env::var("HPA_POSTGRES_SCHEDULER")
        .map(|value| matches!(value.as_str(), "1" | "true"))
        .unwrap_or(false);
    let postgres_pool = if postgres_execution
        || postgres_event_bus
        || postgres_mcp
        || postgres_scheduler
    {
        let database_url = std::env::var("HPA_POSTGRES_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .map_err(|_| {
                CliError(
                    "HPA_POSTGRES_URL or DATABASE_URL is required when PostgreSQL runtime mode is enabled".to_string(),
                )
            })?;
        Some(
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(8)
                .connect(&database_url)
                .await?,
        )
    } else {
        None
    };
    if postgres_execution {
        let pool = postgres_pool
            .as_ref()
            .expect("PostgreSQL pool is configured for execution runtime");
        let installed: bool =
            sqlx::query_scalar("SELECT to_regclass('hpa_tenant.executions') IS NOT NULL")
                .fetch_one(pool)
                .await?;
        if !installed {
            return Err(CliError(
                "PostgreSQL execution schema is missing; run `hpa db migrate` first".to_string(),
            )
            .into());
        }
        enterprise = enterprise.with_postgres_execution(pool.clone());
        if postgres_event_bus {
            let installed: bool = sqlx::query_scalar(
                "SELECT to_regprocedure(
                     'hpa_internal.claim_outbox_topic(text,text,integer,integer)'
                 ) IS NOT NULL",
            )
            .fetch_one(pool)
            .await?;
            if !installed {
                return Err(CliError(
                    "event bus schema is missing; run `hpa db migrate` first".to_string(),
                )
                .into());
            }
            enterprise = enterprise.with_postgres_event_bus(pool.clone());
        }
    }
    if enterprise.uses_postgres() {
        // Mirror tenants/quotas/members so PostgreSQL has the control-plane rows
        // that quota reads, worker registration and audit foreign keys rely on.
        match enterprise.sync_postgres_control().await {
            Ok(count) if count > 0 => {
                println!("postgres control mirror: {count} rows");
            }
            Ok(_) => {}
            Err(error) => {
                return Err(CliError(format!(
                    "cannot mirror the control plane into PostgreSQL: {}",
                    error.message
                ))
                .into());
            }
        }
    }
    let oauth = enterprise.oauth_service();
    let local_state = enterprise
        .local_state()
        .await
        .map_err(|error| CliError(error.message))?;
    let mcp_service = McpService::new(
        local_state.agents.skills.clone(),
        local_state.runtime.clone(),
    );
    let mut gateway = McpGatewayState::new(mcp_service, base_url)
        .with_oauth(oauth)
        .with_enterprise(enterprise.clone())
        .with_rate_limit(
            std::env::var("HPA_MCP_RATE_LIMIT")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(120),
        );
    let mut task_update_monitor = None;
    if postgres_mcp {
        let pool = postgres_pool
            .as_ref()
            .expect("PostgreSQL pool is configured for MCP");
        let installed: bool =
            sqlx::query_scalar("SELECT to_regclass('hpa_control.mcp_sessions') IS NOT NULL")
                .fetch_one(pool)
                .await?;
        if !installed {
            return Err(CliError(
                "persistent MCP schema is missing; run `hpa db migrate` first".to_string(),
            )
            .into());
        }
        gateway = gateway.with_postgres(pool.clone());
        task_update_monitor =
            mcp_http::spawn_task_update_monitor(gateway.clone(), enterprise.clone());
    }
    let scheduler_coordination = if postgres_scheduler {
        let pool = postgres_pool
            .as_ref()
            .expect("PostgreSQL pool is configured for scheduler");
        let installed: bool =
            sqlx::query_scalar("SELECT to_regclass('hpa_control.runtime_leases') IS NOT NULL")
                .fetch_one(pool)
                .await?;
        if !installed {
            return Err(CliError(
                "scheduler lease schema is missing; run `hpa db migrate` first".to_string(),
            )
            .into());
        }
        Some(scheduler::SchedulerCoordination::postgres(
            pool.clone(),
            node_id.clone(),
        ))
    } else {
        None
    };
    let control_mirror = if enterprise.uses_postgres() {
        let interval = std::env::var("HPA_CONTROL_SYNC_INTERVAL_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value >= 50)
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_secs(30));
        Some(enterprise.spawn_control_mirror(interval))
    } else {
        None
    };
    let scheduler = scheduler::spawn(enterprise.clone(), scheduler_coordination);
    let event_bus = if postgres_event_bus && postgres_event_bus_consumer {
        Some(event_bus::spawn(
            enterprise.clone(),
            postgres_pool
                .as_ref()
                .expect("PostgreSQL pool is configured for event bus")
                .clone(),
            node_id.clone(),
        ))
    } else {
        None
    };
    let remote_approval_monitor = enterprise::spawn_remote_approval_monitor(enterprise.clone());
    if task_update_monitor.is_some() {
        println!("mcp node: {}", gateway.node_id());
    }
    if postgres_scheduler {
        println!("scheduler node: {node_id}");
    }
    if postgres_execution {
        println!("execution store: postgresql");
    }
    if postgres_event_bus {
        println!("event bus: postgresql outbox");
    }
    let app = Router::new()
        .merge(enterprise::router(enterprise))
        .merge(mcp_http::router(gateway))
        .fallback(assets::spa)
        .layer(CompressionLayer::new());
    let address = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&address).await?;
    println!("hpa local runtime");
    println!("http://{address}");
    println!("data: {}", data_dir.display());
    println!(
        "enterprise auth: {}",
        if require_auth {
            "required"
        } else {
            "local mode"
        }
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    scheduler.shutdown().await;
    if let Some(event_bus) = event_bus {
        event_bus.shutdown().await;
    }
    remote_approval_monitor.abort();
    if let Some(monitor) = task_update_monitor {
        monitor.abort();
    }
    if let Some(control_mirror) = control_mirror {
        control_mirror.abort();
    }
    Ok(())
}

async fn doctor(data_dir: PathBuf) -> Result<(), Box<dyn Error>> {
    std::fs::create_dir_all(&data_dir)?;
    let db_path = data_dir.join("hpa.db");
    let db = Db::open(&db_path).await?;
    let stats = db.stats().await?;
    let writable = std::fs::metadata(&data_dir)
        .map(|metadata| !metadata.permissions().readonly())
        .unwrap_or(false);
    let postgres =
        match std::env::var("HPA_POSTGRES_URL").or_else(|_| std::env::var("DATABASE_URL")) {
            Ok(database_url) => Some(check_postgres(&database_url, &data_dir).await),
            Err(_) => None,
        };
    let degraded = postgres
        .as_ref()
        .map(|report| report["status"] == "degraded")
        .unwrap_or(false);
    let report = json!({
        "status": if degraded { "degraded" } else { "ok" },
        "internalId": "hpa",
        "version": env!("CARGO_PKG_VERSION"),
        "dataDir": data_dir,
        "database": db_path,
        "writable": writable,
        "stats": stats,
        "postgres": postgres,
        "checks": {
            "sqlite": "ok",
            "embeddedUi": true,
            "offline": true,
        }
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

/// Connectivity, migration drift and RLS coverage for the PostgreSQL control
/// plane. `doctor` stays diagnostic: it reports the problem instead of failing.
async fn check_postgres(database_url: &str, data_dir: &Path) -> Value {
    let pool = match sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(database_url)
        .await
    {
        Ok(pool) => pool,
        Err(error) => {
            return json!({
                "status": "degraded",
                "connected": false,
                "error": error.to_string(),
            });
        }
    };
    static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations/postgres");
    let available = MIGRATOR.migrations.len() as i64;
    let applied: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public._sqlx_migrations WHERE success")
            .fetch_one(&pool)
            .await
            .unwrap_or(0);
    // Tenant tables and control-plane tables are both FORCE RLS; reporting the
    // split keeps "66 tables" in the docs from being read as 66 tenant tables.
    let forced_rls_tenant: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_class c
         JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = 'hpa_tenant' AND c.relkind = 'r'
           AND c.relrowsecurity AND c.relforcerowsecurity",
    )
    .fetch_one(&pool)
    .await
    .unwrap_or(0);
    let forced_rls_control: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_class c
         JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = 'hpa_control' AND c.relkind = 'r'
           AND c.relrowsecurity AND c.relforcerowsecurity",
    )
    .fetch_one(&pool)
    .await
    .unwrap_or(0);
    let tenants: i64 = sqlx::query_scalar("SELECT count(*) FROM hpa_control.tenants")
        .fetch_one(&pool)
        .await
        .unwrap_or(0);
    // Plugin metadata is shared but package directories are node-local: report
    // the gap so "installed on another node, missing here" is visible up front
    // instead of surfacing as a failed plugin load.
    let mut plugin_installed: i64 = 0;
    if let Ok(mut transaction) = pool.begin().await {
        if sqlx::query("SET LOCAL ROLE hpa_platform_admin")
            .execute(&mut *transaction)
            .await
            .is_ok()
        {
            plugin_installed = sqlx::query_scalar(
                "SELECT count(*) FROM hpa_tenant.plugins WHERE status <> 'uninstalled'",
            )
            .fetch_one(&mut *transaction)
            .await
            .unwrap_or(0);
        }
        let _ = transaction.commit().await;
    }
    let plugin_dir = data_dir.join("plugins").join("installed");
    let plugin_local = std::fs::read_dir(&plugin_dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().is_dir())
                .count() as i64
        })
        .unwrap_or(0);
    let mut warnings: Vec<String> = Vec::new();
    if plugin_installed > plugin_local {
        warnings.push(format!(
            "{} plugin(s) are installed in PostgreSQL but their packages are missing under {};              mount the plugins directory on a shared volume when running multiple control-plane nodes",
            plugin_installed - plugin_local,
            plugin_dir.display()
        ));
    }
    let drift = available - applied;
    pool.close().await;
    json!({
        "status": if drift == 0 && applied > 0 { "ok" } else { "degraded" },
        "connected": true,
        "migrations": { "applied": applied, "available": available, "drift": drift },
        "forcedRls": {
            "tenant": forced_rls_tenant,
            "control": forced_rls_control,
            "total": forced_rls_tenant + forced_rls_control,
        },
        "tenants": tenants,
        "plugins": {
            "installedInPostgres": plugin_installed,
            "packagesPresentLocally": plugin_local,
            "packagesMissing": (plugin_installed - plugin_local).max(0),
        },
        "warnings": warnings,
    })
}

fn validate_file(path: &Path) -> Result<(), Box<dyn Error>> {
    let content = std::fs::read_to_string(path)?;
    let report = validate_text(path, &content);
    println!("{}", serde_json::to_string_pretty(&report)?);
    if !report.is_valid() {
        std::process::exit(1);
    }
    Ok(())
}

async fn run_file(path: &Path, input: Option<&str>) -> Result<(), Box<dyn Error>> {
    let content = std::fs::read_to_string(path)?;
    let (workflow, plan) = load_workflow(path, &content)?;
    let input: Value = match input {
        Some(text) => serde_json::from_str(text)
            .map_err(|error| CliError(format!("cannot parse --input JSON: {error}")))?,
        None => json!({}),
    };
    let execution_id = format!("exec_{}", uuid::Uuid::now_v7());
    let mut state = new_state(
        &workflow,
        &plan,
        execution_id,
        input,
        BTreeMap::new(),
        "local",
    )
    .map_err(|error| CliError(error.to_string()))?;
    let mut events = vec![start_event(&mut state)];
    for _ in 0..10_000 {
        if state.status.is_terminal() || state.status == ExecutionStatus::WaitingApproval {
            break;
        }
        let result = step(&workflow, &plan, &mut state).await;
        events.extend(result.events);
        match result.status {
            StepStatus::Continue => {}
            StepStatus::Retry { delay_ms } => {
                tokio::time::sleep(Duration::from_millis(delay_ms.min(60_000))).await;
            }
            StepStatus::WaitingApproval => break,
            StepStatus::Succeeded | StepStatus::Failed => break,
        }
    }
    let summary = json!({
        "executionId": state.id,
        "status": state.status,
        "output": state.output,
        "error": state.error,
        "waiting": state.waiting,
        "events": events.len(),
        "steps": state.step_count,
    });
    println!("{}", serde_json::to_string_pretty(&summary)?);
    if state.status == ExecutionStatus::WaitingApproval {
        std::process::exit(2);
    }
    if state.status == ExecutionStatus::Failed {
        std::process::exit(1);
    }
    Ok(())
}

fn load_workflow(
    path: &Path,
    content: &str,
) -> Result<(Workflow, hpa_core::compiler::ExecutionPlan), Box<dyn Error>> {
    let is_yaml = matches!(
        path.extension().and_then(|value| value.to_str()),
        Some("yaml") | Some("yml")
    );
    let compilation = if is_yaml {
        validator::check_yaml(content)
    } else {
        validator::check_json(content)
    };
    let compilation = compilation.map_err(|report| {
        CliError(format!(
            "workflow is invalid: {}",
            serde_json::to_string_pretty(&report).unwrap_or_default()
        ))
    })?;
    let workflow = if is_yaml {
        Workflow::from_yaml_str(content)
    } else {
        Workflow::from_json_str(content)
    }
    .map_err(|error| CliError(error.to_string()))?;
    Ok((workflow, compilation.plan))
}

fn validate_text(path: &Path, content: &str) -> hpa_core::error::ValidationReport {
    let is_yaml = matches!(
        path.extension().and_then(|value| value.to_str()),
        Some("yaml") | Some("yml")
    );
    if is_yaml {
        return validator::validate_yaml(content);
    }
    let report = validator::validate_json(content);
    if report.has_code("JSON_PARSE") && !content.trim_start().starts_with('{') {
        return validator::validate_yaml(content);
    }
    report
}

fn resolve_data_dir(explicit: Option<PathBuf>) -> Result<PathBuf, Box<dyn Error>> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    if let Ok(path) = std::env::var("HPA_DATA_DIR")
        && !path.trim().is_empty()
    {
        return Ok(PathBuf::from(path));
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_err(|_| CliError("cannot determine home directory; pass --data-dir".to_string()))?;
    Ok(PathBuf::from(home).join(".hpa"))
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}
