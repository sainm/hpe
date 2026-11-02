use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use futures_util::TryStreamExt;
use serde::Serialize;
use serde_json::Value as JsonValue;
use sqlx::postgres::PgPool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use sqlx::{AssertSqlSafe, Row};
use thiserror::Error;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

#[derive(Debug, Error)]
pub enum ImportError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("PostgreSQL/SQLite error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("invalid import source: {0}")]
    Invalid(String),
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub source: String,
    pub tenants: BTreeSet<String>,
    pub tables: BTreeMap<String, TableImportReport>,
    pub missing_tables: BTreeSet<String>,
    pub intentionally_skipped: BTreeMap<String, String>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableImportReport {
    pub rows: u64,
    pub inserted: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TenantMode {
    FromSource,
    Injected,
}

#[derive(Debug, Clone)]
struct TableSpec {
    source: &'static str,
    schema: &'static str,
    target: &'static str,
    tenant_mode: TenantMode,
    conflict_columns: &'static [&'static str],
}

#[derive(Debug)]
enum ImportValue {
    Null(NullKind),
    Text(String),
    SmallInt(i16),
    Integer(i32),
    BigInt(i64),
    Boolean(bool),
    Json(JsonValue),
    Timestamp(OffsetDateTime),
    Bytes(Vec<u8>),
}

#[derive(Debug, Clone, Copy)]
enum NullKind {
    Text,
    SmallInt,
    Integer,
    BigInt,
    Boolean,
    Json,
    Timestamp,
    Bytes,
}

const ID: &[&str] = &["id"];
const TENANT_ID: &[&str] = &["tenant_id"];
const TENANT_AND_ID: &[&str] = &["tenant_id", "id"];
const GROUP_MEMBER: &[&str] = &["group_id", "member_id"];

fn control_specs() -> Vec<TableSpec> {
    vec![
        TableSpec {
            source: "enterprise_tenants",
            schema: "hpa_control",
            target: "tenants",
            tenant_mode: TenantMode::FromSource,
            conflict_columns: ID,
        },
        TableSpec {
            source: "enterprise_quotas",
            schema: "hpa_control",
            target: "quotas",
            tenant_mode: TenantMode::FromSource,
            conflict_columns: TENANT_ID,
        },
        TableSpec {
            source: "enterprise_tokens",
            schema: "hpa_control",
            target: "tokens",
            tenant_mode: TenantMode::FromSource,
            conflict_columns: ID,
        },
        TableSpec {
            source: "enterprise_members",
            schema: "hpa_control",
            target: "members",
            tenant_mode: TenantMode::FromSource,
            conflict_columns: ID,
        },
        TableSpec {
            source: "enterprise_scim_groups",
            schema: "hpa_control",
            target: "scim_groups",
            tenant_mode: TenantMode::FromSource,
            conflict_columns: ID,
        },
        TableSpec {
            source: "enterprise_scim_group_members",
            schema: "hpa_control",
            target: "scim_group_members",
            tenant_mode: TenantMode::FromSource,
            conflict_columns: GROUP_MEMBER,
        },
        TableSpec {
            source: "enterprise_audit",
            schema: "hpa_control",
            target: "audit",
            tenant_mode: TenantMode::FromSource,
            conflict_columns: ID,
        },
        TableSpec {
            source: "enterprise_backups",
            schema: "hpa_control",
            target: "backups",
            tenant_mode: TenantMode::FromSource,
            conflict_columns: ID,
        },
        TableSpec {
            source: "enterprise_workers",
            schema: "hpa_control",
            target: "workers",
            tenant_mode: TenantMode::FromSource,
            conflict_columns: ID,
        },
        TableSpec {
            source: "enterprise_worker_tokens",
            schema: "hpa_control",
            target: "worker_tokens",
            tenant_mode: TenantMode::FromSource,
            conflict_columns: ID,
        },
        TableSpec {
            source: "enterprise_worker_jobs",
            schema: "hpa_control",
            target: "worker_jobs",
            tenant_mode: TenantMode::FromSource,
            conflict_columns: ID,
        },
        TableSpec {
            source: "enterprise_oidc",
            schema: "hpa_control",
            target: "oidc_config",
            tenant_mode: TenantMode::FromSource,
            conflict_columns: ID,
        },
        TableSpec {
            source: "oauth_clients",
            schema: "hpa_control",
            target: "oauth_clients",
            tenant_mode: TenantMode::FromSource,
            conflict_columns: ID,
        },
        TableSpec {
            source: "oauth_signing_keys",
            schema: "hpa_control",
            target: "oauth_signing_keys",
            tenant_mode: TenantMode::FromSource,
            conflict_columns: &["kid"],
        },
        TableSpec {
            source: "oauth_tokens",
            schema: "hpa_control",
            target: "oauth_tokens",
            tenant_mode: TenantMode::FromSource,
            conflict_columns: ID,
        },
        TableSpec {
            source: "oauth_consents",
            schema: "hpa_control",
            target: "oauth_consents",
            tenant_mode: TenantMode::FromSource,
            conflict_columns: &["client_id", "tenant_id", "subject"],
        },
    ]
}

fn business_specs() -> Vec<TableSpec> {
    vec![
        business("workflows", TENANT_AND_ID),
        business(
            "workflow_versions",
            &["tenant_id", "workflow_id", "version"],
        ),
        business("executions", TENANT_AND_ID),
        business("remote_executions", &["tenant_id", "execution_id"]),
        business("events", &["tenant_id", "execution_id", "sequence"]),
        business("artifacts", TENANT_AND_ID),
        business("skills", TENANT_AND_ID),
        business("skill_versions", &["tenant_id", "skill_id", "version"]),
        business("skill_files", &["tenant_id", "skill_id", "version", "path"]),
        business("skill_runs", TENANT_AND_ID),
        business("skill_run_artifacts", TENANT_AND_ID),
        business("mcp_tokens", TENANT_AND_ID),
        business("mcp_approvals", TENANT_AND_ID),
        business("mcp_audit", TENANT_AND_ID),
        business("mcp_servers", TENANT_AND_ID),
        business("agents", TENANT_AND_ID),
        business("agent_versions", &["tenant_id", "agent_id", "version"]),
        business("agent_sessions", TENANT_AND_ID),
        business("agent_events", &["tenant_id", "session_id", "sequence"]),
        business("agent_approvals", TENANT_AND_ID),
        business("plugin_trusted_keys", &["tenant_id", "key_id"]),
        business("plugin_packages", &["tenant_id", "plugin_id", "version"]),
        business("plugins", TENANT_AND_ID),
        business(
            "plugin_capabilities",
            &["tenant_id", "plugin_id", "version", "kind", "capability_id"],
        ),
        business(
            "plugin_market_listings",
            &["tenant_id", "plugin_id", "version"],
        ),
        business("plugin_reviews", TENANT_AND_ID),
        business("plugin_events", TENANT_AND_ID),
        business("plugin_publishers", TENANT_AND_ID),
        business(
            "plugin_publisher_keys",
            &["tenant_id", "publisher_id", "key_id"],
        ),
        business("marketplace_sources", TENANT_AND_ID),
        business(
            "marketplace_source_keys",
            &["tenant_id", "source_id", "key_id"],
        ),
        business(
            "marketplace_remote_packages",
            &["tenant_id", "source_id", "plugin_id", "version"],
        ),
        business(
            "connector_definitions",
            &["tenant_id", "connector_id", "version"],
        ),
        business("connections", TENANT_AND_ID),
        business("connector_audit", TENANT_AND_ID),
        business("trigger_state", &["tenant_id", "workflow_id", "trigger_id"]),
        business("trigger_fires", TENANT_AND_ID),
    ]
}

fn business(target: &'static str, conflict_columns: &'static [&'static str]) -> TableSpec {
    TableSpec {
        source: target,
        schema: "hpa_tenant",
        target,
        tenant_mode: TenantMode::Injected,
        conflict_columns,
    }
}

pub async fn import_data_dir(
    postgres: &PgPool,
    source: &Path,
) -> Result<ImportReport, ImportError> {
    if !source.is_dir() {
        return Err(ImportError::Invalid(format!(
            "{} is not a directory",
            source.display()
        )));
    }
    let mut report = ImportReport {
        source: source.display().to_string(),
        ..ImportReport::default()
    };
    for (table, reason) in [
        (
            "enterprise_auth_flows",
            "transient OIDC state is intentionally not migrated; users must restart login",
        ),
        (
            "enterprise_sessions",
            "browser sessions are revoked on migration and require reauthentication",
        ),
        (
            "oauth_requests",
            "transient authorization requests are intentionally not migrated",
        ),
        (
            "oauth_codes",
            "authorization codes are single-use transient credentials and are not migrated",
        ),
    ] {
        report
            .intentionally_skipped
            .insert(table.to_string(), reason.to_string());
    }

    let control_path = source.join("control.db");
    let mut discovered_tenants = BTreeSet::new();
    if control_path.is_file() {
        let sqlite = open_sqlite_read_only(&control_path).await?;
        if table_exists(&sqlite, "enterprise_tenants").await? {
            let tenants: Vec<String> =
                sqlx::query_scalar("SELECT id FROM enterprise_tenants ORDER BY id")
                    .fetch_all(&sqlite)
                    .await?;
            discovered_tenants.extend(tenants);
        }
        import_database(postgres, &sqlite, &control_specs(), None, &mut report).await?;
        sqlite.close().await;
    }

    let local_path = source.join("hpa.db");
    if local_path.is_file() {
        discovered_tenants.insert("local".to_string());
    }
    let tenants_dir = source.join("tenants");
    if tenants_dir.is_dir() {
        for entry in std::fs::read_dir(&tenants_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() || !entry.path().join("hpa.db").is_file() {
                continue;
            }
            let tenant_id = entry.file_name().to_string_lossy().to_string();
            discovered_tenants.insert(tenant_id);
        }
    }

    for tenant_id in &discovered_tenants {
        let path = tenant_database_path(source, tenant_id);
        if !path.is_file() {
            continue;
        }
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM hpa_control.tenants WHERE id = $1)")
                .bind(tenant_id)
                .fetch_one(postgres)
                .await?;
        if !exists {
            return Err(ImportError::Invalid(format!(
                "tenant {tenant_id:?} has a SQLite database but no hpa_control.tenants row"
            )));
        }
        report.tenants.insert(tenant_id.clone());
        let sqlite = open_sqlite_read_only(&path).await?;
        import_database(
            postgres,
            &sqlite,
            &business_specs(),
            Some(tenant_id.as_str()),
            &mut report,
        )
        .await?;
        sqlite.close().await;
    }

    Ok(report)
}

fn tenant_database_path(source: &Path, tenant_id: &str) -> PathBuf {
    if tenant_id == "local" {
        source.join("hpa.db")
    } else {
        source.join("tenants").join(tenant_id).join("hpa.db")
    }
}

async fn open_sqlite_read_only(path: &Path) -> Result<SqlitePool, ImportError> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .read_only(true)
        .foreign_keys(true);
    Ok(SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?)
}

async fn table_exists(sqlite: &SqlitePool, table: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?
         )",
    )
    .bind(table)
    .fetch_one(sqlite)
    .await
}

async fn import_database(
    postgres: &PgPool,
    sqlite: &SqlitePool,
    specs: &[TableSpec],
    tenant_id: Option<&str>,
    report: &mut ImportReport,
) -> Result<(), ImportError> {
    let mut transaction = postgres.begin().await?;
    sqlx::query("SET LOCAL ROLE hpa_platform_admin")
        .execute(&mut *transaction)
        .await?;
    for spec in specs {
        if !table_exists(sqlite, spec.source).await? {
            report.missing_tables.insert(spec.source.to_string());
            continue;
        }
        if spec.tenant_mode == TenantMode::Injected && tenant_id.is_none() {
            return Err(ImportError::Invalid(format!(
                "business table {} requires an explicit tenant",
                spec.source
            )));
        }
        let table_report = import_table(&mut transaction, sqlite, spec, tenant_id).await?;
        if table_report.rows > 0 {
            report
                .tables
                .insert(format!("{}.{}", spec.schema, spec.target), table_report);
        }
    }
    transaction.commit().await?;
    Ok(())
}

async fn import_table(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    sqlite: &SqlitePool,
    spec: &TableSpec,
    tenant_id: Option<&str>,
) -> Result<TableImportReport, ImportError> {
    let source_columns: HashSet<String> = sqlx::query("SELECT name FROM pragma_table_info(?)")
        .bind(spec.source)
        .fetch_all(sqlite)
        .await?
        .into_iter()
        .map(|row| row.get::<String, _>("name"))
        .collect();
    let target_rows = sqlx::query(
        "SELECT column_name, data_type, is_nullable, column_default
         FROM information_schema.columns
         WHERE table_schema = $1 AND table_name = $2 AND is_generated = 'NEVER'
         ORDER BY ordinal_position",
    )
    .bind(spec.schema)
    .bind(spec.target)
    .fetch_all(&mut **transaction)
    .await?;
    if target_rows.is_empty() {
        return Err(ImportError::Invalid(format!(
            "target table {}.{} does not exist; run db migrate first",
            spec.schema, spec.target
        )));
    }

    let mut selected = Vec::new();
    for row in target_rows {
        let name: String = row.get("column_name");
        let data_type: String = row.get("data_type");
        let nullable: String = row.get("is_nullable");
        let default: Option<String> = row.get("column_default");
        if spec.tenant_mode == TenantMode::Injected && name == "tenant_id" {
            selected.push(SelectedColumn {
                name,
                data_type,
                source_name: None,
            });
        } else if source_columns.contains(&name) {
            selected.push(SelectedColumn {
                name: name.clone(),
                data_type,
                source_name: Some(name),
            });
        } else if nullable == "NO" && default.is_none() {
            return Err(ImportError::Invalid(format!(
                "{}.{} requires column {name}, but source table {} does not contain it",
                spec.schema, spec.target, spec.source
            )));
        }
    }
    let source_selected = selected
        .iter()
        .filter_map(|column| column.source_name.as_ref())
        .collect::<Vec<_>>();
    if source_selected.is_empty() {
        return Ok(TableImportReport::default());
    }

    let select_sql = format!(
        "SELECT {} FROM {}",
        source_selected
            .iter()
            .map(|column| quote_identifier(column))
            .collect::<Vec<_>>()
            .join(", "),
        quote_identifier(spec.source)
    );
    let placeholders = (1..=selected.len())
        .map(|index| format!("${index}"))
        .collect::<Vec<_>>()
        .join(", ");
    let insert_sql = format!(
        "INSERT INTO {}.{} ({}) VALUES ({}) ON CONFLICT ({}) DO NOTHING",
        quote_identifier(spec.schema),
        quote_identifier(spec.target),
        selected
            .iter()
            .map(|column| quote_identifier(&column.name))
            .collect::<Vec<_>>()
            .join(", "),
        placeholders,
        spec.conflict_columns
            .iter()
            .map(|column| quote_identifier(column))
            .collect::<Vec<_>>()
            .join(", ")
    );

    let mut report = TableImportReport::default();
    let mut rows = sqlx::query(AssertSqlSafe(select_sql)).fetch(sqlite);
    while let Some(row) = rows.try_next().await? {
        report.rows += 1;
        let mut values = Vec::with_capacity(selected.len());
        for column in &selected {
            let value = if column.source_name.is_none() {
                ImportValue::Text(tenant_id.expect("validated business tenant").to_string())
            } else {
                decode_value(
                    &row,
                    column.source_name.as_deref().expect("source column"),
                    &column.data_type,
                )
                .map_err(|error| {
                    ImportError::Invalid(format!(
                        "{}.{} row {} column {}: {error}",
                        spec.schema, spec.target, report.rows, column.name
                    ))
                })?
            };
            values.push(value);
        }
        let mut query = sqlx::query::<sqlx::Postgres>(AssertSqlSafe(insert_sql.clone()));
        for value in values {
            query = match value {
                ImportValue::Null(NullKind::Text) => query.bind(Option::<String>::None),
                ImportValue::Null(NullKind::SmallInt) => query.bind(Option::<i16>::None),
                ImportValue::Null(NullKind::Integer) => query.bind(Option::<i32>::None),
                ImportValue::Null(NullKind::BigInt) => query.bind(Option::<i64>::None),
                ImportValue::Null(NullKind::Boolean) => query.bind(Option::<bool>::None),
                ImportValue::Null(NullKind::Json) => query.bind(Option::<JsonValue>::None),
                ImportValue::Null(NullKind::Timestamp) => {
                    query.bind(Option::<OffsetDateTime>::None)
                }
                ImportValue::Null(NullKind::Bytes) => query.bind(Option::<Vec<u8>>::None),
                ImportValue::Text(value) => query.bind(value),
                ImportValue::SmallInt(value) => query.bind(value),
                ImportValue::Integer(value) => query.bind(value),
                ImportValue::BigInt(value) => query.bind(value),
                ImportValue::Boolean(value) => query.bind(value),
                ImportValue::Json(value) => query.bind(value),
                ImportValue::Timestamp(value) => query.bind(value),
                ImportValue::Bytes(value) => query.bind(value),
            };
        }
        report.inserted += query.execute(&mut **transaction).await?.rows_affected();
    }
    Ok(report)
}

struct SelectedColumn {
    name: String,
    data_type: String,
    source_name: Option<String>,
}

fn decode_value(
    row: &sqlx::sqlite::SqliteRow,
    column: &str,
    data_type: &str,
) -> Result<ImportValue, String> {
    match data_type {
        "text" | "character varying" | "character" => row
            .try_get::<Option<String>, _>(column)
            .map(|value| value.map_or(ImportValue::Null(NullKind::Text), ImportValue::Text))
            .map_err(|error| error.to_string()),
        "json" | "jsonb" => {
            let value = row
                .try_get::<Option<String>, _>(column)
                .map_err(|error| error.to_string())?;
            match value {
                Some(value) => serde_json::from_str(&value)
                    .map(ImportValue::Json)
                    .map_err(|error| format!("invalid JSON: {error}")),
                None => Ok(ImportValue::Null(NullKind::Json)),
            }
        }
        "smallint" => row
            .try_get::<Option<i64>, _>(column)
            .map_err(|error| error.to_string())?
            .map_or(Ok(ImportValue::Null(NullKind::SmallInt)), |value| {
                i16::try_from(value)
                    .map(ImportValue::SmallInt)
                    .map_err(|_| format!("{value} does not fit SMALLINT"))
            }),
        "integer" => row
            .try_get::<Option<i64>, _>(column)
            .map_err(|error| error.to_string())?
            .map_or(Ok(ImportValue::Null(NullKind::Integer)), |value| {
                i32::try_from(value)
                    .map(ImportValue::Integer)
                    .map_err(|_| format!("{value} does not fit INTEGER"))
            }),
        "bigint" => row
            .try_get::<Option<i64>, _>(column)
            .map(|value| value.map_or(ImportValue::Null(NullKind::BigInt), ImportValue::BigInt))
            .map_err(|error| error.to_string()),
        "boolean" => {
            let value = row
                .try_get::<Option<i64>, _>(column)
                .map_err(|error| error.to_string())?;
            match value {
                Some(0) => Ok(ImportValue::Boolean(false)),
                Some(1) => Ok(ImportValue::Boolean(true)),
                Some(value) => Err(format!("boolean value must be 0 or 1, got {value}")),
                None => Ok(ImportValue::Null(NullKind::Boolean)),
            }
        }
        "timestamp with time zone" | "timestamp without time zone" => {
            let text = row.try_get::<Option<String>, _>(column);
            if let Ok(value) = text {
                return value.map_or(Ok(ImportValue::Null(NullKind::Timestamp)), |value| {
                    parse_timestamp(&value).map(ImportValue::Timestamp)
                });
            }
            row.try_get::<Option<i64>, _>(column)
                .map_err(|error| error.to_string())?
                .map_or(Ok(ImportValue::Null(NullKind::Timestamp)), |milliseconds| {
                    timestamp_from_millis(milliseconds).map(ImportValue::Timestamp)
                })
        }
        "bytea" => row
            .try_get::<Option<Vec<u8>>, _>(column)
            .map(|value| value.map_or(ImportValue::Null(NullKind::Bytes), ImportValue::Bytes))
            .map_err(|error| error.to_string()),
        other => Err(format!("unsupported PostgreSQL type {other:?}")),
    }
}

fn parse_timestamp(value: &str) -> Result<OffsetDateTime, String> {
    if value.trim().is_empty() {
        return Err("timestamp is empty".to_string());
    }
    OffsetDateTime::parse(value, &Rfc3339)
        .or_else(|_| {
            let mut normalized = value.trim().replacen(' ', "T", 1);
            if !normalized.ends_with('Z')
                && !normalized
                    .get(19..)
                    .is_some_and(|suffix| suffix.contains('+') || suffix.contains('-'))
            {
                normalized.push('Z');
            }
            OffsetDateTime::parse(&normalized, &Rfc3339)
        })
        .map_err(|error| format!("invalid timestamp {value:?}: {error}"))
}

fn timestamp_from_millis(milliseconds: i64) -> Result<OffsetDateTime, String> {
    let nanoseconds = milliseconds
        .checked_mul(1_000_000)
        .ok_or_else(|| format!("timestamp {milliseconds}ms overflows nanoseconds"))?;
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(nanoseconds))
        .map_err(|error| format!("invalid millisecond timestamp {milliseconds}: {error}"))
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}
