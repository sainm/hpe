use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{Signature as Ed25519Signature, Signer, SigningKey, Verifier, VerifyingKey};
use futures_util::StreamExt;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, Row};
use tar::{Archive, Builder, Header};
use thiserror::Error;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::agent;
use crate::connector::{self, ConnectorManifest, ConnectorRuntime};
use crate::db::{AgentVersionRow, Db, DbError};
use crate::skills::{SkillService, SkillServiceError};
use hpa_local::postgres_mcp_meta::{McpPluginServerInput, PostgresMcpMetaError};
use hpa_local::postgres_plugin::{
    MarketplaceListingRecord, MarketplaceReviewRecord, PluginCapabilityRecord, PluginEventRecord,
    PluginPackageRecord, PluginRecord, PostgresPluginError, PublisherKeyRecord,
    RemotePackageRecord, SourceKeyRecord, TrustedKeyRecord,
};

pub const PLUGIN_SCHEMA: &str = "hpa.plugin/v1";
pub const MANIFEST_FILE: &str = "hpa.plugin.json";
pub const MAX_PLUGIN_FILES: usize = 500;
pub const MAX_PLUGIN_BYTES: usize = 20 * 1024 * 1024;
pub const MAX_PLUGIN_FILE_BYTES: usize = 5 * 1024 * 1024;
pub const OSV_QUERY_URL: &str = "https://api.osv.dev/v1/query";

#[derive(Debug, Clone)]
pub enum VulnerabilitySource {
    Disabled,
    Offline {
        path: PathBuf,
        max_age_hours: i64,
    },
    Osv {
        endpoint: String,
        ecosystem: Option<String>,
    },
    Invalid(String),
}

#[derive(Debug, Clone)]
pub enum CosignTrust {
    Key(PathBuf),
    Keyless {
        certificate_identity: String,
        oidc_issuer: String,
    },
}

#[derive(Debug, Clone)]
pub enum ProvenanceVerifier {
    Disabled,
    Cosign { binary: PathBuf, trust: CosignTrust },
    Invalid(String),
}

#[derive(Debug, Clone)]
pub struct ProvenanceConfig {
    pub required: bool,
    pub verifier: ProvenanceVerifier,
}

impl ProvenanceConfig {
    pub fn from_environment() -> Self {
        let required = std::env::var("HPA_REQUIRE_PROVENANCE")
            .map(|value| matches!(value.as_str(), "1" | "true" | "required"))
            .unwrap_or(false);
        let Some(binary) = std::env::var_os("HPA_COSIGN_BIN").map(PathBuf::from) else {
            return Self {
                required,
                verifier: ProvenanceVerifier::Disabled,
            };
        };
        let public_key = std::env::var_os("HPA_COSIGN_PUBLIC_KEY").map(PathBuf::from);
        let certificate_identity = std::env::var("HPA_COSIGN_CERTIFICATE_IDENTITY").ok();
        let oidc_issuer = std::env::var("HPA_COSIGN_OIDC_ISSUER").ok();
        let trust = match (public_key, certificate_identity, oidc_issuer) {
            (Some(public_key), None, None) => CosignTrust::Key(public_key),
            (None, Some(certificate_identity), Some(oidc_issuer))
                if !certificate_identity.trim().is_empty() && !oidc_issuer.trim().is_empty() =>
            {
                CosignTrust::Keyless {
                    certificate_identity,
                    oidc_issuer,
                }
            }
            _ => {
                return Self {
                    required,
                    verifier: ProvenanceVerifier::Invalid(
                        "configure exactly one cosign trust mode: public key, or keyless identity plus OIDC issuer"
                            .to_string(),
                    ),
                };
            }
        };
        Self {
            required,
            verifier: ProvenanceVerifier::Cosign { binary, trust },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RemotePackageProvenance {
    kind: String,
    statement_path: String,
    statement_sha256: String,
    bundle_path: String,
    bundle_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RemotePackageOci {
    repository: String,
    digest: String,
}

impl VulnerabilitySource {
    pub fn from_environment(data_dir: &Path) -> Self {
        match std::env::var("HPA_VULNERABILITY_SOURCE")
            .unwrap_or_else(|_| "disabled".to_string())
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "" | "disabled" => Self::Disabled,
            "offline" => {
                let raw = std::env::var("HPA_VULNERABILITY_DB").unwrap_or_default();
                if raw.trim().is_empty() {
                    return Self::Invalid(
                        "HPA_VULNERABILITY_DB is required for offline vulnerability checks"
                            .to_string(),
                    );
                }
                let path = PathBuf::from(raw);
                let path = if path.is_absolute() {
                    path
                } else {
                    data_dir.join(path)
                };
                let max_age_hours = std::env::var("HPA_VULNERABILITY_MAX_AGE_HOURS")
                    .ok()
                    .and_then(|value| value.parse::<i64>().ok())
                    .unwrap_or(24)
                    .clamp(1, 720);
                Self::Offline {
                    path,
                    max_age_hours,
                }
            }
            "osv" | "online" => Self::Osv {
                endpoint: std::env::var("HPA_OSV_URL")
                    .unwrap_or_else(|_| OSV_QUERY_URL.to_string()),
                ecosystem: std::env::var("HPA_OSV_ECOSYSTEM")
                    .ok()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty()),
            },
            other => Self::Invalid(format!("unsupported HPA_VULNERABILITY_SOURCE `{other}`")),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OfflineVulnerabilityDb {
    schema_version: String,
    generated_at: String,
    records: Vec<OfflineAdvisory>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfflineAdvisory {
    id: String,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    modified: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    affected: Vec<OfflineAffected>,
}

#[derive(Debug, Deserialize)]
struct OfflineAffected {
    package: OfflinePackage,
    #[serde(default)]
    ranges: Vec<OfflineRange>,
    #[serde(default)]
    versions: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct OfflinePackage {
    ecosystem: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct OfflineRange {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    events: Vec<OfflineRangeEvent>,
}

#[derive(Debug, Deserialize)]
struct OfflineRangeEvent {
    #[serde(default)]
    introduced: Option<String>,
    #[serde(default)]
    fixed: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OsvQueryResponse {
    #[serde(default)]
    vulns: Vec<OsvAdvisory>,
}

#[derive(Debug, Deserialize)]
struct OsvAdvisory {
    id: String,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    modified: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
}

#[derive(Debug, Error)]
pub enum PluginError {
    #[error("{0}")]
    Db(#[from] DbError),
    #[error("{0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Skill(#[from] SkillServiceError),
    #[error("{0}")]
    Connector(#[from] connector::ConnectorError),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Frozen(String),
    #[error("{0}")]
    Signature(String),
    #[error("{0}")]
    License(String),
    #[error("{0}")]
    Dependency(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Forbidden(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginManifest {
    pub schema_version: String,
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
    pub license: String,
    #[serde(default)]
    pub publisher: Option<Publisher>,
    pub compatibility: Compatibility,
    #[serde(default)]
    pub capabilities: Vec<PluginCapability>,
    #[serde(default)]
    pub dependencies: Vec<PluginDependency>,
    #[serde(default)]
    pub permissions: Vec<PluginPermission>,
    #[serde(default)]
    pub integrity: Option<Integrity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<PluginSignature>,
    #[serde(default)]
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Publisher {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Compatibility {
    pub hpa: String,
    #[serde(default)]
    pub os: Vec<String>,
    #[serde(default)]
    pub runtimes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginCapability {
    pub kind: CapabilityKind,
    pub id: String,
    pub version: String,
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CapabilityKind {
    Connector,
    Mcp,
    Skill,
    Agent,
}

impl CapabilityKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Connector => "connector",
            Self::Mcp => "mcp",
            Self::Skill => "skill",
            Self::Agent => "agent",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginDependency {
    pub kind: String,
    pub id: String,
    pub range: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginPermission {
    pub kind: String,
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(default)]
    pub read: Vec<String>,
    #[serde(default)]
    pub write: Vec<String>,
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub provider: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Integrity {
    pub algorithm: String,
    pub digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSignature {
    pub algorithm: String,
    pub key_id: String,
    pub public_key: String,
    pub value: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct PluginRow {
    pub id: String,
    pub name: String,
    pub latest_version: String,
    pub status: String,
    pub installed_version: Option<String>,
    pub previous_version: Option<String>,
    pub trust: String,
    pub license: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct PluginPackageRow {
    pub plugin_id: String,
    pub version: String,
    pub name: String,
    pub description: Option<String>,
    pub license: String,
    pub publisher: Option<String>,
    pub manifest_json: String,
    pub content_hash: String,
    pub package_path: String,
    pub signature_json: Option<String>,
    pub signer_key_id: Option<String>,
    pub signer_public_key: Option<String>,
    pub trust: String,
    pub source: String,
    pub capabilities_json: String,
    pub sbom_json: String,
    pub created_at: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct PluginCapabilityRow {
    pub plugin_id: String,
    pub version: String,
    pub kind: String,
    pub capability_id: String,
    pub capability_version: String,
    pub path: String,
    pub status: String,
    pub config_json: String,
    pub permissions_json: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct MarketplaceListingRow {
    pub plugin_id: String,
    pub version: String,
    pub review_status: String,
    pub review_note: Option<String>,
    pub submitted_at: String,
    pub reviewed_at: Option<String>,
    pub withdrawn_at: Option<String>,
    pub downloads: i64,
}

#[derive(Debug, Clone, FromRow)]
pub struct MarketplaceReviewRow {
    pub id: String,
    pub plugin_id: String,
    pub version: String,
    pub rating: i64,
    pub comment: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct TrustedKeyRow {
    pub key_id: String,
    pub public_key: String,
    pub label: String,
    pub trust: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RemoteMarketplaceIndex {
    schema_version: String,
    source_id: String,
    sequence: i64,
    generated_at: String,
    #[serde(default)]
    packages: Vec<RemoteMarketplacePackage>,
    #[serde(default)]
    withdrawals: Vec<RemoteMarketplaceWithdrawal>,
    signature: RemoteMarketplaceSignature,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RemoteMarketplacePackage {
    plugin_id: String,
    version: String,
    path: String,
    sha256: String,
    publisher_id: String,
    signer_key_id: String,
    review_status: String,
    published_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provenance: Option<RemotePackageProvenance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    oci: Option<RemotePackageOci>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RemoteMarketplaceWithdrawal {
    plugin_id: String,
    version: String,
    reason: String,
    at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RemoteMarketplaceSignature {
    algorithm: String,
    key_id: String,
    public_key: String,
    value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteMarketplacePayload {
    schema_version: String,
    source_id: String,
    sequence: i64,
    generated_at: String,
    packages: Vec<RemoteMarketplacePackage>,
    withdrawals: Vec<RemoteMarketplaceWithdrawal>,
}

#[derive(Debug, Clone, FromRow)]
struct RemoteMarketplacePackageRow {
    source_id: String,
    plugin_id: String,
    version: String,
    publisher_id: String,
    signer_key_id: String,
    package_sha256: String,
    package_path: String,
    review_status: String,
    index_sequence: i64,
    published_at: Option<String>,
    withdrawn_at: Option<String>,
    revocation_reason: Option<String>,
    provenance_json: Option<String>,
    oci_json: Option<String>,
}

type RegisteredKeyMaterial = (String, Option<String>, Option<String>, Option<String>);

#[derive(Debug, Clone, FromRow)]
pub struct PluginEventRow {
    pub id: String,
    pub at: String,
    pub plugin_id: Option<String>,
    pub version: Option<String>,
    pub kind: String,
    pub detail_json: String,
}

#[derive(Debug, Clone)]
pub struct LocalKey {
    pub key_id: String,
    pub public_key: String,
    signing_key: SigningKey,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct BundleOptions {
    pub allow_unsigned: bool,
    pub allow_untrusted: bool,
    pub require_signature: bool,
}

#[derive(Debug, Clone)]
pub struct PluginBundle {
    pub manifest: PluginManifest,
    pub files: BTreeMap<String, Vec<u8>>,
    pub content_hash: String,
    pub trust: String,
}

#[derive(Clone)]
pub struct PluginService {
    pub db: Db,
    pub data_dir: PathBuf,
    pub skills: SkillService,
    pub connectors: ConnectorRuntime,
    vulnerability_source: VulnerabilitySource,
    provenance_config: ProvenanceConfig,
}

impl PluginService {
    pub fn new(
        db: Db,
        data_dir: PathBuf,
        skills: SkillService,
        connectors: ConnectorRuntime,
    ) -> Self {
        let vulnerability_source = VulnerabilitySource::from_environment(&data_dir);
        let provenance_config = ProvenanceConfig::from_environment();
        Self {
            db,
            data_dir,
            skills,
            connectors,
            vulnerability_source,
            provenance_config,
        }
    }

    #[cfg(test)]
    pub fn with_vulnerability_source(mut self, source: VulnerabilitySource) -> Self {
        self.vulnerability_source = source;
        self
    }

    #[cfg(test)]
    pub fn with_provenance_config(mut self, config: ProvenanceConfig) -> Self {
        self.provenance_config = config;
        self
    }

    pub async fn vulnerability_report(
        &self,
        plugin_id: &str,
        version: &str,
    ) -> Result<Value, PluginError> {
        validate_identifier(plugin_id, "plugin id")?;
        let parsed_version = Version::parse(version)
            .map_err(|error| PluginError::Invalid(format!("invalid plugin version: {error}")))?;
        match &self.vulnerability_source {
            VulnerabilitySource::Disabled => Ok(json!({
                "schemaVersion": "hpa.vulnerability-report/v1",
                "status": "disabled",
                "source": "disabled",
                "checkedAt": now_rfc3339(),
                "databaseGeneratedAt": null,
                "package": {"ecosystem": "HPA", "name": plugin_id, "version": version},
                "query": {"ecosystem": null, "name": plugin_id, "version": version},
                "vulnerabilities": []
            })),
            VulnerabilitySource::Invalid(message) => Err(PluginError::Invalid(message.clone())),
            VulnerabilitySource::Offline {
                path,
                max_age_hours,
            } => {
                let metadata = tokio::fs::metadata(path).await?;
                if metadata.len() > 20 * 1024 * 1024 {
                    return Err(PluginError::Invalid(
                        "offline vulnerability database exceeds 20 MiB".to_string(),
                    ));
                }
                let bytes = tokio::fs::read(path).await?;
                let database: OfflineVulnerabilityDb = serde_json::from_slice(&bytes)?;
                if database.schema_version != "hpa.vulnerability-db/v1" {
                    return Err(PluginError::Invalid(
                        "unsupported offline vulnerability database schema".to_string(),
                    ));
                }
                if database.records.len() > 100_000 {
                    return Err(PluginError::Invalid(
                        "offline vulnerability database has too many records".to_string(),
                    ));
                }
                let mut advisory_ids = BTreeSet::new();
                for advisory in &database.records {
                    if advisory.id.trim().is_empty()
                        || advisory.id.len() > 256
                        || !advisory_ids.insert(advisory.id.as_str())
                    {
                        return Err(PluginError::Invalid(
                            "offline vulnerability database has an invalid or duplicate advisory id"
                                .to_string(),
                        ));
                    }
                }
                let generated_at = OffsetDateTime::parse(&database.generated_at, &Rfc3339)
                    .map_err(|error| {
                        PluginError::Invalid(format!(
                            "offline vulnerability database generatedAt is invalid: {error}"
                        ))
                    })?;
                let age = OffsetDateTime::now_utc() - generated_at;
                if age < -time::Duration::minutes(5) {
                    return Err(PluginError::Forbidden(
                        "offline vulnerability database generatedAt is in the future".to_string(),
                    ));
                }
                if age > time::Duration::hours(*max_age_hours) {
                    return Err(PluginError::Forbidden(format!(
                        "offline vulnerability database is older than {max_age_hours} hours"
                    )));
                }
                let vulnerabilities = database
                    .records
                    .iter()
                    .filter(|advisory| advisory_affects(advisory, plugin_id, &parsed_version))
                    .map(offline_advisory_value)
                    .collect::<Vec<_>>();
                Ok(vulnerability_report_value(
                    "offline",
                    plugin_id,
                    version,
                    vulnerabilities,
                    Some(database.generated_at),
                    Some("HPA"),
                ))
            }
            VulnerabilitySource::Osv {
                endpoint,
                ecosystem,
            } => {
                validate_osv_endpoint(endpoint).await?;
                let client = reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .no_proxy()
                    .timeout(Duration::from_secs(20))
                    .build()
                    .map_err(|error| PluginError::Invalid(error.to_string()))?;
                let mut package = json!({"name": plugin_id});
                if let Some(ecosystem) = ecosystem {
                    package["ecosystem"] = Value::String(ecosystem.clone());
                }
                let response = client
                    .post(endpoint)
                    .json(&json!({
                        "package": package,
                        "version": version
                    }))
                    .send()
                    .await
                    .map_err(|error| {
                        PluginError::Conflict(format!("OSV vulnerability query failed: {error}"))
                    })?;
                if !response.status().is_success() {
                    return Err(PluginError::Conflict(format!(
                        "OSV vulnerability query returned HTTP {}",
                        response.status()
                    )));
                }
                if response
                    .content_length()
                    .is_some_and(|length| length > 2 * 1024 * 1024)
                {
                    return Err(PluginError::Invalid(
                        "OSV vulnerability response exceeds 2 MiB".to_string(),
                    ));
                }
                let bytes = response.bytes().await.map_err(|error| {
                    PluginError::Conflict(format!("cannot read OSV response: {error}"))
                })?;
                if bytes.len() > 2 * 1024 * 1024 {
                    return Err(PluginError::Invalid(
                        "OSV vulnerability response exceeds 2 MiB".to_string(),
                    ));
                }
                let body: OsvQueryResponse = serde_json::from_slice(&bytes)?;
                if body.vulns.len() > 10_000 {
                    return Err(PluginError::Invalid(
                        "OSV vulnerability response contains too many records".to_string(),
                    ));
                }
                let vulnerabilities = body
                    .vulns
                    .into_iter()
                    .map(osv_advisory_value)
                    .collect::<Vec<_>>();
                Ok(vulnerability_report_value(
                    "osv",
                    plugin_id,
                    version,
                    vulnerabilities,
                    None,
                    ecosystem.as_deref(),
                ))
            }
        }
    }

    pub async fn local_key(&self) -> Result<LocalKey, PluginError> {
        let keys_dir = self.data_dir.join("keys");
        tokio::fs::create_dir_all(&keys_dir).await?;
        let secret_path = keys_dir.join("plugin-ed25519.key");
        let signing_key = if secret_path.exists() {
            let encoded = tokio::fs::read_to_string(&secret_path).await?;
            let bytes = BASE64
                .decode(encoded.trim())
                .map_err(|error| PluginError::Signature(format!("invalid local key: {error}")))?;
            let bytes: [u8; 32] = bytes
                .try_into()
                .map_err(|_| PluginError::Signature("local key must be 32 bytes".to_string()))?;
            SigningKey::from_bytes(&bytes)
        } else {
            let mut bytes = [0u8; 32];
            getrandom::getrandom(&mut bytes)
                .map_err(|error| PluginError::Signature(format!("cannot generate key: {error}")))?;
            let key = SigningKey::from_bytes(&bytes);
            let encoded = BASE64.encode(bytes);
            tokio::fs::write(&secret_path, encoded).await?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ =
                    std::fs::set_permissions(&secret_path, std::fs::Permissions::from_mode(0o600));
            }
            key
        };
        let public = signing_key.verifying_key();
        let public_bytes = public.to_bytes();
        let public_key = BASE64.encode(public_bytes);
        let mut hasher = Sha256::new();
        hasher.update(public_bytes);
        let key_id = format!("ed25519-{}", &format!("{:x}", hasher.finalize())[..16]);
        let now = now_rfc3339();
        if let Some(store) = self.db.postgres_plugin() {
            store
                .upsert_trusted_key(&key_id, &public_key, "Local plugin key", "verified", &now)
                .await
                .map_err(pg_plugin_error)?;
        } else {
            sqlx::query(
                "INSERT INTO plugin_trusted_keys (key_id, public_key, label, trust, created_at)
                 VALUES (?, ?, 'Local plugin key', 'verified', ?)
                 ON CONFLICT(key_id) DO UPDATE SET public_key = excluded.public_key,
                    label = excluded.label, trust = excluded.trust",
            )
            .bind(&key_id)
            .bind(&public_key)
            .bind(&now)
            .execute(&self.db.pool)
            .await?;
        }
        tokio::fs::write(keys_dir.join("plugin-ed25519.pub"), &public_key).await?;
        Ok(LocalKey {
            key_id,
            public_key,
            signing_key,
        })
    }

    pub async fn key_info(&self) -> Result<Value, PluginError> {
        let key = self.local_key().await?;
        Ok(json!({
            "keyId": key.key_id,
            "publicKey": key.public_key,
            "algorithm": "ed25519",
            "trust": "verified",
        }))
    }

    pub async fn trust_key(
        &self,
        key_id: &str,
        public_key: &str,
        label: &str,
        trust: &str,
    ) -> Result<Value, PluginError> {
        if !matches!(trust, "verified" | "community") {
            return Err(PluginError::Invalid(
                "trust must be verified or community".to_string(),
            ));
        }
        decode_public_key(public_key)?;
        if key_id.trim().is_empty() {
            return Err(PluginError::Invalid("keyId is required".to_string()));
        }
        if let Some(store) = self.db.postgres_plugin() {
            store
                .upsert_trusted_key(key_id, public_key, label, trust, &now_rfc3339())
                .await
                .map_err(pg_plugin_error)?;
        } else {
            sqlx::query(
                "INSERT INTO plugin_trusted_keys (key_id, public_key, label, trust, created_at)
                 VALUES (?, ?, ?, ?, ?)
                 ON CONFLICT(key_id) DO UPDATE SET public_key = excluded.public_key,
                    label = excluded.label, trust = excluded.trust",
            )
            .bind(key_id)
            .bind(public_key)
            .bind(label)
            .bind(trust)
            .bind(now_rfc3339())
            .execute(&self.db.pool)
            .await?;
        }
        self.event(
            None,
            None,
            "key.trusted",
            json!({"keyId": key_id, "trust": trust}),
        )
        .await?;
        Ok(json!({"keyId": key_id, "trust": trust}))
    }

    pub async fn trusted_keys(&self) -> Result<Vec<Value>, PluginError> {
        let rows = if let Some(store) = self.db.postgres_plugin() {
            store
                .list_trusted_keys()
                .await
                .map_err(pg_plugin_error)?
                .into_iter()
                .map(trusted_key_row_from_pg)
                .collect::<Vec<_>>()
        } else {
            sqlx::query_as::<_, TrustedKeyRow>(
                "SELECT key_id, public_key, label, trust, created_at
                 FROM plugin_trusted_keys ORDER BY created_at ASC",
            )
            .fetch_all(&self.db.pool)
            .await?
        };
        Ok(rows
            .into_iter()
            .map(|row| {
                json!({
                    "keyId": row.key_id,
                    "publicKey": row.public_key,
                    "label": row.label,
                    "trust": row.trust,
                    "createdAt": row.created_at,
                })
            })
            .collect())
    }

    pub async fn marketplace_sources(&self) -> Result<Vec<Value>, PluginError> {
        if let Some(store) = self.db.postgres_plugin() {
            let sources = store.list_sources().await.map_err(pg_plugin_error)?;
            let mut values = Vec::with_capacity(sources.len());
            for source in sources {
                let keys = store
                    .source_keys(&source.id, true)
                    .await
                    .map_err(pg_plugin_error)?;
                let (package_count, approved_count, withdrawn_count) = store
                    .source_package_counts(&source.id)
                    .await
                    .map_err(pg_plugin_error)?;
                values.push(json!({
                    "id": source.id,
                    "label": source.label,
                    "indexUri": source.index_uri,
                    "enabled": source.enabled,
                    "lastSequence": source.last_sequence,
                    "lastRefreshAt": source.last_refresh_at,
                    "createdAt": source.created_at,
                    "updatedAt": source.updated_at,
                    "keys": keys.iter().map(source_key_value).collect::<Vec<_>>(),
                    "packageCount": package_count,
                    "approvedCount": approved_count,
                    "withdrawnCount": withdrawn_count,
                }));
            }
            return Ok(values);
        }
        let rows = sqlx::query(
            "SELECT id, label, index_uri, enabled, last_sequence, last_refresh_at,
                    created_at, updated_at
             FROM marketplace_sources
             ORDER BY lower(id)",
        )
        .fetch_all(&self.db.pool)
        .await?;
        let mut sources = Vec::with_capacity(rows.len());
        for row in rows {
            let source_id: String = row.get("id");
            let keys = sqlx::query(
                "SELECT key_id, label, valid_from, valid_until
                 FROM marketplace_source_keys
                 WHERE source_id = ? AND revoked_at IS NULL
                 ORDER BY created_at DESC",
            )
            .bind(&source_id)
            .fetch_all(&self.db.pool)
            .await?;
            let (package_count, approved_count, withdrawn_count): (i64, i64, i64) = sqlx::query_as(
                "SELECT
                         count(*),
                         COALESCE(sum(CASE WHEN review_status = 'approved'
                                           AND withdrawn_at IS NULL THEN 1 ELSE 0 END), 0),
                         COALESCE(sum(CASE WHEN withdrawn_at IS NOT NULL THEN 1 ELSE 0 END), 0)
                     FROM marketplace_remote_packages
                     WHERE source_id = ?",
            )
            .bind(&source_id)
            .fetch_one(&self.db.pool)
            .await?;
            sources.push(json!({
                "id": source_id,
                "label": row.get::<String, _>("label"),
                "indexUri": row.get::<String, _>("index_uri"),
                "enabled": row.get::<i64, _>("enabled") != 0,
                "lastSequence": row.get::<i64, _>("last_sequence"),
                "lastRefreshAt": row.get::<Option<String>, _>("last_refresh_at"),
                "createdAt": row.get::<String, _>("created_at"),
                "updatedAt": row.get::<String, _>("updated_at"),
                "keys": keys.iter().map(|key| json!({
                    "keyId": key.get::<String, _>("key_id"),
                    "label": key.get::<String, _>("label"),
                    "validFrom": key.get::<Option<String>, _>("valid_from"),
                    "validUntil": key.get::<Option<String>, _>("valid_until"),
                })).collect::<Vec<_>>(),
                "packageCount": package_count,
                "approvedCount": approved_count,
                "withdrawnCount": withdrawn_count,
            }));
        }
        Ok(sources)
    }

    pub async fn marketplace_publishers(&self) -> Result<Vec<Value>, PluginError> {
        if let Some(store) = self.db.postgres_plugin() {
            let publishers = store.list_publishers().await.map_err(pg_plugin_error)?;
            let mut values = Vec::with_capacity(publishers.len());
            for publisher in publishers {
                let keys = store
                    .publisher_keys(&publisher.id)
                    .await
                    .map_err(pg_plugin_error)?;
                values.push(json!({
                    "id": publisher.id,
                    "name": publisher.name,
                    "status": publisher.status,
                    "createdAt": publisher.created_at,
                    "updatedAt": publisher.updated_at,
                    "keys": keys.iter().map(publisher_key_value).collect::<Vec<_>>(),
                }));
            }
            return Ok(values);
        }
        let rows = sqlx::query(
            "SELECT id, name, status, created_at, updated_at
             FROM plugin_publishers
             ORDER BY lower(id)",
        )
        .fetch_all(&self.db.pool)
        .await?;
        let mut publishers = Vec::with_capacity(rows.len());
        for row in rows {
            let publisher_id: String = row.get("id");
            let keys = sqlx::query(
                "SELECT key_id, label, valid_from, valid_until, revoked_at, created_at
                 FROM plugin_publisher_keys
                 WHERE publisher_id = ?
                 ORDER BY created_at DESC",
            )
            .bind(&publisher_id)
            .fetch_all(&self.db.pool)
            .await?;
            publishers.push(json!({
                "id": publisher_id,
                "name": row.get::<String, _>("name"),
                "status": row.get::<String, _>("status"),
                "createdAt": row.get::<String, _>("created_at"),
                "updatedAt": row.get::<String, _>("updated_at"),
                "keys": keys.iter().map(|key| json!({
                    "keyId": key.get::<String, _>("key_id"),
                    "label": key.get::<String, _>("label"),
                    "validFrom": key.get::<Option<String>, _>("valid_from"),
                    "validUntil": key.get::<Option<String>, _>("valid_until"),
                    "revokedAt": key.get::<Option<String>, _>("revoked_at"),
                    "createdAt": key.get::<String, _>("created_at"),
                })).collect::<Vec<_>>(),
            }));
        }
        Ok(publishers)
    }

    pub async fn add_publisher(
        &self,
        publisher_id: &str,
        name: &str,
    ) -> Result<Value, PluginError> {
        validate_identifier(publisher_id, "publisher id")?;
        if name.trim().is_empty() {
            return Err(PluginError::Invalid(
                "publisher name is required".to_string(),
            ));
        }
        let now = now_rfc3339();
        if let Some(store) = self.db.postgres_plugin() {
            store
                .upsert_publisher(publisher_id, name, &now)
                .await
                .map_err(pg_plugin_error)?;
            return Ok(json!({"publisherId": publisher_id, "name": name, "status": "active"}));
        }
        sqlx::query(
            "INSERT INTO plugin_publishers (id, name, status, created_at, updated_at)
             VALUES (?, ?, 'active', ?, ?)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name,
                updated_at = excluded.updated_at",
        )
        .bind(publisher_id)
        .bind(name)
        .bind(&now)
        .bind(&now)
        .execute(&self.db.pool)
        .await?;
        Ok(json!({"publisherId": publisher_id, "name": name, "status": "active"}))
    }

    pub async fn add_publisher_key(
        &self,
        publisher_id: &str,
        key_id: &str,
        public_key: &str,
        label: &str,
        valid_from: Option<&str>,
        valid_until: Option<&str>,
    ) -> Result<Value, PluginError> {
        decode_public_key(public_key)?;
        let now = now_rfc3339();
        if let Some(store) = self.db.postgres_plugin() {
            if !store
                .publisher_active(publisher_id)
                .await
                .map_err(pg_plugin_error)?
            {
                return Err(PluginError::NotFound(
                    "active publisher not found".to_string(),
                ));
            }
            store
                .upsert_publisher_key(
                    publisher_id,
                    key_id,
                    public_key,
                    label,
                    valid_from,
                    valid_until,
                    &now,
                    &format!("{publisher_id}: {label}"),
                    true,
                )
                .await
                .map_err(pg_plugin_error)?;
            return Ok(json!({
                "publisherId": publisher_id,
                "keyId": key_id,
                "status": "active",
                "validFrom": valid_from,
                "validUntil": valid_until,
            }));
        }
        let exists: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM plugin_publishers WHERE id = ? AND status = 'active'",
        )
        .bind(publisher_id)
        .fetch_optional(&self.db.pool)
        .await?;
        if exists.is_none() {
            return Err(PluginError::NotFound(
                "active publisher not found".to_string(),
            ));
        }
        let now = now_rfc3339();
        sqlx::query(
            "INSERT INTO plugin_publisher_keys
             (publisher_id, key_id, public_key, label, valid_from, valid_until, revoked_at, created_at)
             VALUES (?, ?, ?, ?, ?, ?, NULL, ?)
             ON CONFLICT(publisher_id, key_id) DO UPDATE SET
                public_key = excluded.public_key,
                label = excluded.label,
                valid_from = excluded.valid_from,
                valid_until = excluded.valid_until,
                revoked_at = NULL",
        )
        .bind(publisher_id)
        .bind(key_id)
        .bind(public_key)
        .bind(label)
        .bind(valid_from)
        .bind(valid_until)
        .bind(&now)
        .execute(&self.db.pool)
        .await?;
        sqlx::query(
            "INSERT INTO plugin_trusted_keys (key_id, public_key, label, trust, created_at)
             VALUES (?, ?, ?, 'verified', ?)
             ON CONFLICT(key_id) DO UPDATE SET public_key = excluded.public_key,
                label = excluded.label, trust = 'verified'",
        )
        .bind(key_id)
        .bind(public_key)
        .bind(format!("{publisher_id}: {label}"))
        .bind(&now)
        .execute(&self.db.pool)
        .await?;
        Ok(json!({
            "publisherId": publisher_id,
            "keyId": key_id,
            "status": "active",
            "validFrom": valid_from,
            "validUntil": valid_until,
        }))
    }

    pub async fn revoke_publisher_key(
        &self,
        publisher_id: &str,
        key_id: &str,
    ) -> Result<Value, PluginError> {
        let revoked = if let Some(store) = self.db.postgres_plugin() {
            store
                .revoke_publisher_key(publisher_id, key_id, &now_rfc3339())
                .await
                .map_err(pg_plugin_error)?
        } else {
            sqlx::query(
                "UPDATE plugin_publisher_keys SET revoked_at = ?
                 WHERE publisher_id = ? AND key_id = ? AND revoked_at IS NULL",
            )
            .bind(now_rfc3339())
            .bind(publisher_id)
            .bind(key_id)
            .execute(&self.db.pool)
            .await?
            .rows_affected()
                == 1
        };
        if !revoked {
            return Err(PluginError::NotFound(
                "active publisher key not found".to_string(),
            ));
        }
        Ok(json!({"publisherId": publisher_id, "keyId": key_id, "revoked": true}))
    }

    pub async fn add_marketplace_source(
        &self,
        source_id: &str,
        label: &str,
        index_uri: &str,
        key_id: &str,
        public_key: &str,
        key_label: &str,
    ) -> Result<Value, PluginError> {
        validate_identifier(source_id, "marketplace source id")?;
        validate_remote_index_uri(index_uri)?;
        decode_public_key(public_key)?;
        let now = now_rfc3339();
        if let Some(store) = self.db.postgres_plugin() {
            store
                .upsert_source_with_key(
                    source_id, label, index_uri, key_id, public_key, key_label, &now,
                )
                .await
                .map_err(pg_plugin_error)?;
            return Ok(json!({
                "sourceId": source_id,
                "indexUri": index_uri,
                "keyId": key_id,
                "status": "active"
            }));
        }
        sqlx::query(
            "INSERT INTO marketplace_sources
             (id, label, index_uri, enabled, last_sequence, last_refresh_at, created_at, updated_at)
             VALUES (?, ?, ?, 1, 0, NULL, ?, ?)
             ON CONFLICT(id) DO UPDATE SET label = excluded.label,
                index_uri = excluded.index_uri, enabled = 1, updated_at = excluded.updated_at",
        )
        .bind(source_id)
        .bind(label)
        .bind(index_uri)
        .bind(&now)
        .bind(&now)
        .execute(&self.db.pool)
        .await?;
        sqlx::query(
            "INSERT INTO marketplace_source_keys
             (source_id, key_id, public_key, label, valid_from, valid_until, revoked_at, created_at)
             VALUES (?, ?, ?, ?, ?, NULL, NULL, ?)
             ON CONFLICT(source_id, key_id) DO UPDATE SET
                public_key = excluded.public_key,
                label = excluded.label,
                revoked_at = NULL",
        )
        .bind(source_id)
        .bind(key_id)
        .bind(public_key)
        .bind(key_label)
        .bind(&now)
        .bind(&now)
        .execute(&self.db.pool)
        .await?;
        Ok(json!({
            "sourceId": source_id,
            "indexUri": index_uri,
            "keyId": key_id,
            "status": "active"
        }))
    }

    pub async fn rotate_marketplace_source_key(
        &self,
        source_id: &str,
        key_id: &str,
        public_key: &str,
        label: &str,
    ) -> Result<Value, PluginError> {
        decode_public_key(public_key)?;
        let now = now_rfc3339();
        if let Some(store) = self.db.postgres_plugin() {
            store
                .rotate_source_key(source_id, key_id, public_key, label, &now)
                .await
                .map_err(pg_plugin_error)?;
            return Ok(json!({"sourceId": source_id, "keyId": key_id, "rotatedAt": now}));
        }
        sqlx::query(
            "UPDATE marketplace_source_keys SET revoked_at = ?
             WHERE source_id = ? AND revoked_at IS NULL",
        )
        .bind(&now)
        .bind(source_id)
        .execute(&self.db.pool)
        .await?;
        sqlx::query(
            "INSERT INTO marketplace_source_keys
             (source_id, key_id, public_key, label, valid_from, valid_until, revoked_at, created_at)
             VALUES (?, ?, ?, ?, ?, NULL, NULL, ?)",
        )
        .bind(source_id)
        .bind(key_id)
        .bind(public_key)
        .bind(label)
        .bind(&now)
        .bind(&now)
        .execute(&self.db.pool)
        .await?;
        Ok(json!({"sourceId": source_id, "keyId": key_id, "rotatedAt": now}))
    }

    pub async fn refresh_marketplace_source(&self, source_id: &str) -> Result<Value, PluginError> {
        let source: Option<(String, String, i64)> = if let Some(store) = self.db.postgres_plugin() {
            store
                .enabled_source(source_id)
                .await
                .map_err(pg_plugin_error)?
                .map(|source| (source.index_uri, source.label, source.last_sequence))
        } else {
            sqlx::query_as(
                "SELECT index_uri, label, last_sequence FROM marketplace_sources
                 WHERE id = ? AND enabled = 1",
            )
            .bind(source_id)
            .fetch_optional(&self.db.pool)
            .await?
        };
        let Some((index_uri, _, last_sequence)) = source else {
            return Err(PluginError::NotFound(
                "enabled marketplace source not found".to_string(),
            ));
        };
        let bytes = fetch_remote_bytes(Path::new(""), &index_uri).await?;
        if bytes.len() > 2 * 1024 * 1024 {
            return Err(PluginError::Invalid(
                "marketplace index exceeds 2 MiB".to_string(),
            ));
        }
        let index: RemoteMarketplaceIndex = serde_json::from_slice(&bytes)?;
        if index.schema_version != "hpa.marketplace.index/v1" {
            return Err(PluginError::Invalid(
                "unsupported marketplace index schema".to_string(),
            ));
        }
        if index.source_id != source_id {
            return Err(PluginError::Signature(
                "marketplace index sourceId does not match the configured source".to_string(),
            ));
        }
        if index.sequence < last_sequence {
            return Err(PluginError::Conflict(
                "marketplace index sequence rollback detected".to_string(),
            ));
        }
        self.verify_marketplace_index(source_id, &index).await?;

        let base = remote_index_base(&index_uri);
        let mut package_keys = BTreeSet::new();
        let mut accepted = 0usize;
        for package in &index.packages {
            validate_identifier(&package.plugin_id, "plugin id")?;
            Version::parse(&package.version).map_err(|error| {
                PluginError::Invalid(format!("invalid plugin version: {error}"))
            })?;
            if !matches!(package.review_status.as_str(), "approved" | "rejected") {
                return Err(PluginError::Invalid(
                    "remote package reviewStatus must be approved or rejected".to_string(),
                ));
            }
            if package.sha256.len() != 64
                || !package
                    .sha256
                    .chars()
                    .all(|character| character.is_ascii_hexdigit())
            {
                return Err(PluginError::Invalid(
                    "remote package sha256 is invalid".to_string(),
                ));
            }
            let package_bytes = fetch_remote_bytes(Path::new(&base), &package.path).await?;
            let actual_hash = format!("{:x}", Sha256::digest(&package_bytes));
            if actual_hash != package.sha256.to_ascii_lowercase() {
                return Err(PluginError::Signature(
                    "remote package hash does not match the signed index".to_string(),
                ));
            }
            self.verify_remote_package(package, &package_bytes).await?;
            let cache = self
                .data_dir
                .join("marketplace")
                .join("sources")
                .join(source_id)
                .join("packages")
                .join(&package.plugin_id)
                .join(format!("{}.hplugin", package.version));
            if let Some(parent) = cache.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            self.verify_remote_provenance(package, &base, &cache)
                .await?;
            let temporary = cache.with_extension("hplugin.tmp");
            tokio::fs::write(&temporary, &package_bytes).await?;
            tokio::fs::rename(&temporary, &cache).await?;
            if let Some(store) = self.db.postgres_plugin() {
                let provenance = package
                    .provenance
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?;
                let oci = package
                    .oci
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?;
                store
                    .upsert_remote_package(
                        source_id,
                        &package.plugin_id,
                        &package.version,
                        &package.publisher_id,
                        &package.signer_key_id,
                        &package.sha256,
                        &cache.to_string_lossy(),
                        &package.review_status,
                        index.sequence,
                        package.published_at.as_deref(),
                        provenance.as_deref(),
                        oci.as_deref(),
                    )
                    .await
                    .map_err(pg_plugin_error)?;
            } else {
                sqlx::query(
                    "INSERT INTO marketplace_remote_packages
                     (source_id, plugin_id, version, publisher_id, signer_key_id, package_sha256,
                      package_path, review_status, index_sequence, published_at, withdrawn_at,
                      revocation_reason, provenance_json, oci_json)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL, ?, ?)
                     ON CONFLICT(source_id, plugin_id, version) DO UPDATE SET
                        publisher_id = excluded.publisher_id,
                        signer_key_id = excluded.signer_key_id,
                        package_sha256 = excluded.package_sha256,
                        package_path = excluded.package_path,
                        review_status = excluded.review_status,
                        index_sequence = excluded.index_sequence,
                        published_at = excluded.published_at,
                        withdrawn_at = NULL,
                        revocation_reason = NULL,
                        provenance_json = excluded.provenance_json,
                        oci_json = excluded.oci_json",
                )
                .bind(source_id)
                .bind(&package.plugin_id)
                .bind(&package.version)
                .bind(&package.publisher_id)
                .bind(&package.signer_key_id)
                .bind(&package.sha256)
                .bind(cache.to_string_lossy().to_string())
                .bind(&package.review_status)
                .bind(index.sequence)
                .bind(&package.published_at)
                .bind(
                    package
                        .provenance
                        .as_ref()
                        .map(serde_json::to_string)
                        .transpose()?,
                )
                .bind(
                    package
                        .oci
                        .as_ref()
                        .map(serde_json::to_string)
                        .transpose()?,
                )
                .execute(&self.db.pool)
                .await?;
            }
            package_keys.insert((package.plugin_id.clone(), package.version.clone()));
            accepted += 1;
        }

        for withdrawal in &index.withdrawals {
            if let Some(store) = self.db.postgres_plugin() {
                store
                    .withdraw_remote_package(
                        source_id,
                        &withdrawal.plugin_id,
                        &withdrawal.version,
                        &withdrawal.at,
                        &withdrawal.reason,
                    )
                    .await
                    .map_err(pg_plugin_error)?;
            } else {
                sqlx::query(
                    "UPDATE marketplace_remote_packages
                     SET withdrawn_at = ?, revocation_reason = ?
                     WHERE source_id = ? AND plugin_id = ? AND version = ?",
                )
                .bind(&withdrawal.at)
                .bind(&withdrawal.reason)
                .bind(source_id)
                .bind(&withdrawal.plugin_id)
                .bind(&withdrawal.version)
                .execute(&self.db.pool)
                .await?;
            }
            let installed: Option<String> = if let Some(store) = self.db.postgres_plugin() {
                store
                    .plugin_installed_version(&withdrawal.plugin_id)
                    .await
                    .map_err(pg_plugin_error)?
                    .flatten()
            } else {
                sqlx::query_scalar("SELECT installed_version FROM plugins WHERE id = ?")
                    .bind(&withdrawal.plugin_id)
                    .fetch_optional(&self.db.pool)
                    .await?
                    .flatten()
            };
            if installed.as_deref() == Some(withdrawal.version.as_str()) {
                let _ = self.disable(&withdrawal.plugin_id).await;
            }
            self.event(
                Some(&withdrawal.plugin_id),
                Some(&withdrawal.version),
                "marketplace.remote_withdrawn",
                json!({"sourceId": source_id, "reason": withdrawal.reason, "at": withdrawal.at}),
            )
            .await?;
        }

        let existing: Vec<(String, String)> = if let Some(store) = self.db.postgres_plugin() {
            store
                .active_remote_packages(source_id)
                .await
                .map_err(pg_plugin_error)?
        } else {
            sqlx::query_as(
                "SELECT plugin_id, version FROM marketplace_remote_packages
                 WHERE source_id = ? AND withdrawn_at IS NULL",
            )
            .bind(source_id)
            .fetch_all(&self.db.pool)
            .await?
        };
        for (plugin_id, version) in existing {
            if !package_keys.contains(&(plugin_id.clone(), version.clone())) {
                if let Some(store) = self.db.postgres_plugin() {
                    store
                        .withdraw_remote_package(
                            source_id,
                            &plugin_id,
                            &version,
                            &now_rfc3339(),
                            "removed-from-index",
                        )
                        .await
                        .map_err(pg_plugin_error)?;
                } else {
                    sqlx::query(
                        "UPDATE marketplace_remote_packages
                         SET withdrawn_at = ?, revocation_reason = 'removed-from-index'
                         WHERE source_id = ? AND plugin_id = ? AND version = ?",
                    )
                    .bind(now_rfc3339())
                    .bind(source_id)
                    .bind(&plugin_id)
                    .bind(&version)
                    .execute(&self.db.pool)
                    .await?;
                }
            }
        }
        if let Some(store) = self.db.postgres_plugin() {
            store
                .update_source_sequence(source_id, index.sequence, &now_rfc3339())
                .await
                .map_err(pg_plugin_error)?;
        } else {
            sqlx::query(
                "UPDATE marketplace_sources
                 SET last_sequence = ?, last_refresh_at = ?, updated_at = ?
                 WHERE id = ?",
            )
            .bind(index.sequence)
            .bind(now_rfc3339())
            .bind(now_rfc3339())
            .bind(source_id)
            .execute(&self.db.pool)
            .await?;
        }
        Ok(json!({
            "sourceId": source_id,
            "sequence": index.sequence,
            "packages": accepted,
            "withdrawals": index.withdrawals.len()
        }))
    }

    pub async fn remote_marketplace_packages(
        &self,
        source_id: Option<&str>,
    ) -> Result<Vec<Value>, PluginError> {
        let rows = if let Some(store) = self.db.postgres_plugin() {
            store
                .remote_packages(source_id)
                .await
                .map_err(pg_plugin_error)?
                .into_iter()
                .map(remote_package_row_from_pg)
                .collect::<Vec<_>>()
        } else if let Some(source_id) = source_id {
            sqlx::query_as::<_, RemoteMarketplacePackageRow>(
                "SELECT * FROM marketplace_remote_packages
                 WHERE source_id = ? ORDER BY plugin_id, version DESC",
            )
            .bind(source_id)
            .fetch_all(&self.db.pool)
            .await?
        } else {
            sqlx::query_as::<_, RemoteMarketplacePackageRow>(
                "SELECT * FROM marketplace_remote_packages
                 ORDER BY source_id, plugin_id, version DESC",
            )
            .fetch_all(&self.db.pool)
            .await?
        };
        Ok(rows
            .into_iter()
            .map(|row| {
                json!({
                    "sourceId": row.source_id,
                    "pluginId": row.plugin_id,
                    "version": row.version,
                    "publisherId": row.publisher_id,
                    "signerKeyId": row.signer_key_id,
                    "packageSha256": row.package_sha256,
                    "reviewStatus": row.review_status,
                    "sequence": row.index_sequence,
                    "publishedAt": row.published_at,
                    "withdrawnAt": row.withdrawn_at,
                    "revocationReason": row.revocation_reason,
                    "provenance": row.provenance_json.and_then(|value| serde_json::from_str::<Value>(&value).ok()),
                    "oci": row.oci_json.and_then(|value| serde_json::from_str::<Value>(&value).ok()),
                })
            })
            .collect())
    }

    async fn remote_package_row(
        &self,
        source_id: &str,
        plugin_id: &str,
        version: &str,
    ) -> Result<RemoteMarketplacePackageRow, PluginError> {
        let row = if let Some(store) = self.db.postgres_plugin() {
            store
                .remote_package(source_id, plugin_id, version)
                .await
                .map_err(pg_plugin_error)?
                .map(remote_package_row_from_pg)
        } else {
            sqlx::query_as::<_, RemoteMarketplacePackageRow>(
                "SELECT * FROM marketplace_remote_packages
                 WHERE source_id = ? AND plugin_id = ? AND version = ?",
            )
            .bind(source_id)
            .bind(plugin_id)
            .bind(version)
            .fetch_optional(&self.db.pool)
            .await?
        };
        row.ok_or_else(|| PluginError::NotFound("remote package not found".to_string()))
    }

    pub async fn install_remote_package(
        &self,
        source_id: &str,
        plugin_id: &str,
        version: &str,
    ) -> Result<Value, PluginError> {
        let mut package = self
            .remote_package_row(source_id, plugin_id, version)
            .await?;
        if package.review_status != "approved" || package.withdrawn_at.is_some() {
            return Err(PluginError::Forbidden(
                "remote package is not approved or was withdrawn".to_string(),
            ));
        }
        let bytes = match tokio::fs::read(&package.package_path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.refresh_marketplace_source(source_id).await?;
                package = self
                    .remote_package_row(source_id, plugin_id, version)
                    .await?;
                if package.review_status != "approved" || package.withdrawn_at.is_some() {
                    return Err(PluginError::Forbidden(
                        "remote package is not approved or was withdrawn".to_string(),
                    ));
                }
                tokio::fs::read(&package.package_path).await?
            }
            Err(error) => return Err(error.into()),
        };
        let actual = format!("{:x}", Sha256::digest(&bytes));
        if actual != package.package_sha256 {
            return Err(PluginError::Signature(
                "cached remote package hash changed".to_string(),
            ));
        }
        let index_package = RemoteMarketplacePackage {
            plugin_id: plugin_id.to_string(),
            version: version.to_string(),
            path: String::new(),
            sha256: package.package_sha256.clone(),
            publisher_id: package.publisher_id.clone(),
            signer_key_id: package.signer_key_id.clone(),
            review_status: package.review_status.clone(),
            published_at: package.published_at.clone(),
            provenance: package
                .provenance_json
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?,
            oci: package
                .oci_json
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?,
        };
        self.verify_cached_provenance(&index_package, Path::new(&package.package_path))
            .await?;
        self.verify_remote_package(&index_package, &bytes).await?;
        let bundle = self
            .load_source(
                Path::new(&package.package_path),
                BundleOptions {
                    allow_unsigned: false,
                    allow_untrusted: true,
                    require_signature: true,
                },
            )
            .await?;
        self.install_bundle(bundle, &format!("remote:{source_id}"))
            .await
    }

    async fn verify_marketplace_index(
        &self,
        source_id: &str,
        index: &RemoteMarketplaceIndex,
    ) -> Result<(), PluginError> {
        if index.signature.algorithm != "ed25519" {
            return Err(PluginError::Signature(
                "marketplace index must use Ed25519".to_string(),
            ));
        }
        let registered: Option<RegisteredKeyMaterial> =
            if let Some(store) = self.db.postgres_plugin() {
                store
                    .source_key(source_id, &index.signature.key_id)
                    .await
                    .map_err(pg_plugin_error)?
                    .map(registered_key_tuple)
            } else {
                sqlx::query_as(
                    "SELECT public_key, valid_from, valid_until, revoked_at
                     FROM marketplace_source_keys WHERE source_id = ? AND key_id = ?",
                )
                .bind(source_id)
                .bind(&index.signature.key_id)
                .fetch_optional(&self.db.pool)
                .await?
            };
        let Some((public_key, valid_from, valid_until, revoked_at)) = registered else {
            return Err(PluginError::Signature(
                "marketplace index key is not registered for this source".to_string(),
            ));
        };
        if revoked_at.is_some()
            || public_key != index.signature.public_key
            || !time_window_active(valid_from.as_deref(), valid_until.as_deref())
        {
            return Err(PluginError::Signature(
                "marketplace source key is revoked, expired, or mismatched".to_string(),
            ));
        }
        let payload = RemoteMarketplacePayload {
            schema_version: index.schema_version.clone(),
            source_id: index.source_id.clone(),
            sequence: index.sequence,
            generated_at: index.generated_at.clone(),
            packages: index.packages.clone(),
            withdrawals: index.withdrawals.clone(),
        };
        let payload_bytes = serde_json::to_vec(&payload)?;
        verify_ed25519(&public_key, &payload_bytes, &index.signature.value)
    }

    async fn verify_remote_package(
        &self,
        package: &RemoteMarketplacePackage,
        bytes: &[u8],
    ) -> Result<(), PluginError> {
        let files = read_archive_files(bytes)?;
        let manifest_bytes = files
            .get(MANIFEST_FILE)
            .ok_or_else(|| PluginError::Invalid("remote package has no manifest".to_string()))?;
        let manifest: PluginManifest = serde_json::from_slice(manifest_bytes)?;
        validate_manifest_structure(&manifest)?;
        if manifest.id != package.plugin_id || manifest.version != package.version {
            return Err(PluginError::Signature(
                "remote package identity does not match the signed index".to_string(),
            ));
        }
        let publisher = manifest
            .publisher
            .as_ref()
            .ok_or_else(|| PluginError::Signature("remote package has no publisher".to_string()))?;
        let publisher_active = if let Some(store) = self.db.postgres_plugin() {
            store
                .publisher_active(&package.publisher_id)
                .await
                .map_err(pg_plugin_error)?
        } else {
            sqlx::query_scalar::<_, i64>(
                "SELECT 1 FROM plugin_publishers WHERE id = ? AND status = 'active'",
            )
            .bind(&package.publisher_id)
            .fetch_optional(&self.db.pool)
            .await?
            .is_some()
        };
        if publisher.id != package.publisher_id || !publisher_active {
            return Err(PluginError::Signature(
                "remote package publisher is unknown or suspended".to_string(),
            ));
        }
        let registration: Option<RegisteredKeyMaterial> =
            if let Some(store) = self.db.postgres_plugin() {
                store
                    .publisher_key(&package.publisher_id, &package.signer_key_id)
                    .await
                    .map_err(pg_plugin_error)?
                    .map(registered_key_tuple)
            } else {
                sqlx::query_as(
                    "SELECT public_key, valid_from, valid_until, revoked_at
                     FROM plugin_publisher_keys WHERE publisher_id = ? AND key_id = ?",
                )
                .bind(&package.publisher_id)
                .bind(&package.signer_key_id)
                .fetch_optional(&self.db.pool)
                .await?
            };
        let Some((public_key, valid_from, valid_until, revoked_at)) = registration else {
            return Err(PluginError::Signature(
                "remote package signer is not registered for the publisher".to_string(),
            ));
        };
        if revoked_at.is_some()
            || !time_window_active(valid_from.as_deref(), valid_until.as_deref())
        {
            return Err(PluginError::Signature(
                "remote package signer is revoked or expired".to_string(),
            ));
        }
        let content_hash = compute_content_hash(&files)?;
        let signature = manifest
            .signature
            .as_ref()
            .ok_or_else(|| PluginError::Signature("remote package must be signed".to_string()))?;
        if signature.key_id != package.signer_key_id || signature.public_key != public_key {
            return Err(PluginError::Signature(
                "remote package signer does not match publisher key registry".to_string(),
            ));
        }
        let canonical = canonical_manifest(&manifest)?;
        verify_signature(signature, &manifest, &content_hash, &canonical)?;
        if !hpa_skill::allowed_license(&manifest.license) {
            return Err(PluginError::License(format!(
                "remote package license `{}` is not allowed",
                manifest.license
            )));
        }
        Ok(())
    }

    async fn verify_remote_provenance(
        &self,
        package: &RemoteMarketplacePackage,
        base: &str,
        cache: &Path,
    ) -> Result<(), PluginError> {
        let Some(provenance) = &package.provenance else {
            if self.provenance_config.required || package.oci.is_some() {
                return Err(PluginError::Forbidden(
                    "remote package requires signed provenance".to_string(),
                ));
            }
            return Ok(());
        };
        if provenance.kind != "cosign-v1" {
            return Err(PluginError::Signature(format!(
                "unsupported provenance kind `{}`",
                provenance.kind
            )));
        }
        let statement = fetch_remote_bytes(Path::new(base), &provenance.statement_path).await?;
        let bundle = fetch_remote_bytes(Path::new(base), &provenance.bundle_path).await?;
        verify_provenance_hashes(
            &statement,
            &bundle,
            &provenance.statement_sha256,
            &provenance.bundle_sha256,
        )?;
        validate_provenance_statement(&statement, &package.sha256, package.oci.as_ref())?;
        self.verify_cosign_files(&statement, &bundle).await?;
        let statement_cache = cache.with_extension("provenance.json");
        let bundle_cache = cache.with_extension("provenance.bundle.json");
        tokio::fs::write(&statement_cache, statement).await?;
        tokio::fs::write(&bundle_cache, bundle).await?;
        Ok(())
    }

    async fn verify_cached_provenance(
        &self,
        package: &RemoteMarketplacePackage,
        package_path: &Path,
    ) -> Result<(), PluginError> {
        let Some(provenance) = &package.provenance else {
            if self.provenance_config.required || package.oci.is_some() {
                return Err(PluginError::Forbidden(
                    "remote package requires signed provenance".to_string(),
                ));
            }
            return Ok(());
        };
        let statement_path = package_path.with_extension("provenance.json");
        let bundle_path = package_path.with_extension("provenance.bundle.json");
        let statement = tokio::fs::read(&statement_path).await?;
        let bundle = tokio::fs::read(&bundle_path).await?;
        verify_provenance_hashes(
            &statement,
            &bundle,
            &provenance.statement_sha256,
            &provenance.bundle_sha256,
        )?;
        validate_provenance_statement(&statement, &package.sha256, package.oci.as_ref())?;
        self.verify_cosign_files(&statement, &bundle).await
    }

    async fn verify_cosign_files(
        &self,
        statement: &[u8],
        bundle: &[u8],
    ) -> Result<(), PluginError> {
        let (binary, trust) = match &self.provenance_config.verifier {
            ProvenanceVerifier::Disabled => {
                return Err(PluginError::Forbidden(
                    "cosign provenance verifier is not configured".to_string(),
                ));
            }
            ProvenanceVerifier::Invalid(message) => {
                return Err(PluginError::Invalid(message.clone()));
            }
            ProvenanceVerifier::Cosign { binary, trust } => (binary, trust),
        };
        if !binary.is_absolute() || !binary.is_file() {
            return Err(PluginError::Invalid(
                "HPA_COSIGN_BIN must be an absolute file path".to_string(),
            ));
        }
        let directory = self.data_dir.join("tmp").join("cosign");
        tokio::fs::create_dir_all(&directory).await?;
        let id = Uuid::now_v7();
        let statement_path = directory.join(format!("{id}.statement.json"));
        let bundle_path = directory.join(format!("{id}.bundle.json"));
        tokio::fs::write(&statement_path, statement).await?;
        tokio::fs::write(&bundle_path, bundle).await?;
        let mut command = tokio::process::Command::new(binary);
        command.arg("verify-blob").arg("--bundle").arg(&bundle_path);
        match trust {
            CosignTrust::Key(public_key) => {
                if !public_key.is_absolute() || !public_key.is_file() {
                    let _ = tokio::fs::remove_file(&statement_path).await;
                    let _ = tokio::fs::remove_file(&bundle_path).await;
                    return Err(PluginError::Invalid(
                        "HPA_COSIGN_PUBLIC_KEY must be an absolute file path".to_string(),
                    ));
                }
                command
                    .arg("--key")
                    .arg(public_key)
                    .arg("--insecure-ignore-tlog=true");
            }
            CosignTrust::Keyless {
                certificate_identity,
                oidc_issuer,
            } => {
                command
                    .arg("--certificate-identity")
                    .arg(certificate_identity)
                    .arg("--certificate-oidc-issuer")
                    .arg(oidc_issuer);
            }
        }
        command
            .arg(&statement_path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let result = tokio::time::timeout(Duration::from_secs(20), command.output()).await;
        let _ = tokio::fs::remove_file(&statement_path).await;
        let _ = tokio::fs::remove_file(&bundle_path).await;
        match result {
            Ok(Ok(output)) if output.status.success() => Ok(()),
            Ok(Ok(output)) => Err(PluginError::Signature(format!(
                "cosign provenance verification failed: {}",
                String::from_utf8_lossy(&output.stderr[..output.stderr.len().min(1024)])
            ))),
            Ok(Err(error)) => Err(PluginError::Signature(format!(
                "cannot execute cosign: {error}"
            ))),
            Err(_) => Err(PluginError::Signature(
                "cosign provenance verification timed out".to_string(),
            )),
        }
    }

    pub async fn sign_directory(&self, path: &Path) -> Result<Value, PluginError> {
        let files = read_directory_files(path)?;
        let manifest_bytes = files
            .get(MANIFEST_FILE)
            .ok_or_else(|| PluginError::Invalid(format!("missing {MANIFEST_FILE}")))?;
        let mut manifest: PluginManifest = serde_json::from_slice(manifest_bytes)?;
        validate_manifest_structure(&manifest)?;
        let content_hash = compute_content_hash(&files)?;
        manifest.integrity = Some(Integrity {
            algorithm: "sha256".to_string(),
            digest: content_hash.clone(),
        });
        manifest.signature = None;
        let canonical = canonical_manifest(&manifest)?;
        let key = self.local_key().await?;
        let payload = signature_payload(&manifest, &content_hash, &canonical);
        let signature = key.signing_key.sign(payload.as_bytes());
        manifest.signature = Some(PluginSignature {
            algorithm: "ed25519".to_string(),
            key_id: key.key_id.clone(),
            public_key: key.public_key.clone(),
            value: BASE64.encode(signature.to_bytes()),
        });
        let bytes = serde_json::to_vec_pretty(&manifest)?;
        std::fs::write(path.join(MANIFEST_FILE), bytes)?;
        Ok(json!({
            "signed": true,
            "keyId": key.key_id,
            "publicKey": key.public_key,
            "contentHash": content_hash,
            "manifest": manifest,
        }))
    }

    pub async fn pack_directory(&self, path: &Path, output: &Path) -> Result<Value, PluginError> {
        let files = read_directory_files(path)?;
        let bundle = self.load_files(files, BundleOptions::default()).await?;
        let bytes = pack_files(&bundle.files)?;
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(output, &bytes)?;
        Ok(json!({
            "package": output,
            "pluginId": bundle.manifest.id,
            "version": bundle.manifest.version,
            "contentHash": bundle.content_hash,
            "trust": bundle.trust,
            "bytes": bytes.len(),
        }))
    }

    pub async fn inspect(&self, path: &Path, options: BundleOptions) -> Result<Value, PluginError> {
        let bundle = self.load_source(path, options).await?;
        let key = self.local_key().await?;
        let sbom = build_sbom(&bundle, &key);
        Ok(json!({
            "valid": true,
            "manifest": bundle.manifest,
            "contentHash": bundle.content_hash,
            "trust": bundle.trust,
            "sbom": sbom,
        }))
    }

    pub async fn install(&self, path: &Path, options: BundleOptions) -> Result<Value, PluginError> {
        let bundle = self.load_source(path, options).await?;
        let source = "local";
        self.install_bundle(bundle, source).await
    }

    pub async fn create_sample(&self) -> Result<Value, PluginError> {
        let mut files = BTreeMap::new();
        let manifest = json!({
            "schemaVersion": PLUGIN_SCHEMA,
            "id": "plugin_sample_kit",
            "name": "Sample Capability Kit",
            "version": "1.0.0",
            "description": "A signed local sample plugin with a connector, an instructions-only skill and an agent draft.",
            "license": "Apache-2.0",
            "publisher": {"id": "local", "name": "Local Workspace"},
            "compatibility": {"hpa": ">=0.1.0, <1.0.0", "os": []},
            "capabilities": [
                {"kind": "connector", "id": "sample_http", "version": "1.0.0", "path": "connectors/sample-http.connector.json"},
                {"kind": "skill", "id": "skill_sample_guide", "version": "1.0.0", "path": "skills/sample-guide"},
                {"kind": "agent", "id": "agent_plugin_sample", "version": "1.0.0", "path": "agents/sample.agent.json"}
            ],
            "dependencies": [],
            "permissions": [{"kind": "network", "hosts": ["127.0.0.1"]}]
        });
        files.insert(
            MANIFEST_FILE.to_string(),
            serde_json::to_vec_pretty(&manifest)?,
        );
        files.insert(
            "connectors/sample-http.connector.json".to_string(),
            serde_json::to_vec_pretty(&json!({
                "schemaVersion": connector::CONNECTOR_SCHEMA,
                "id": "sample_http",
                "version": "1.0.0",
                "name": "Sample HTTP",
                "kind": "http",
                "description": "A declarative HTTP connector contributed by the sample plugin.",
                "auth": [{"kind": "bearer"}],
                "operations": [{
                    "name": "ping",
                    "description": "Call the root path of the connection base URL.",
                    "risk": "read",
                    "inputSchema": {"type": "object", "properties": {}},
                    "http": {"method": "GET", "path": "/"}
                }],
                "permissions": [{"kind": "network", "hosts": ["127.0.0.1"]}]
            }))?,
        );
        files.insert(
            "skills/sample-guide/SKILL.md".to_string(),
            b"---\nname: sample-guide\ndescription: A sample instructions-only skill contributed by a plugin.\nlicense: Apache-2.0\n---\n\n# Sample Guide\n\nUse this guide to confirm that plugin-provided skills become available as drafts in the Skill Registry.\n"
                .to_vec(),
        );
        files.insert(
            "skills/sample-guide/hpa.skill.json".to_string(),
            serde_json::to_vec_pretty(&json!({
                "schemaVersion": "hpa.skill/v1",
                "id": "skill_sample_guide",
                "name": "sample-guide",
                "version": "1.0.0",
                "license": "Apache-2.0",
                "entry": {"kind": "instructions"},
                "scripts": [],
                "tests": []
            }))?,
        );
        files.insert(
            "agents/sample.agent.json".to_string(),
            serde_json::to_vec_pretty(&json!({
                "schemaVersion": agent::AGENT_SCHEMA,
                "id": "agent_plugin_sample",
                "name": "Plugin Sample Agent",
                "version": "1.0.0",
                "description": "Imported by the sample plugin as a draft AgentVersion.",
                "tools": ["workflow.list"],
                "planner": {
                    "kind": "deterministic",
                    "plan": [
                        {"kind": "tool", "tool": "workflow.list", "arguments": {}},
                        {"kind": "respond", "output": {"workflows": {"$last": "workflows"}}}
                    ]
                },
                "subAgents": []
            }))?,
        );
        let signed = self.sign_files(files).await?;
        let archive = pack_files(&signed)?;
        let dir = self.data_dir.join("plugins").join("samples");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("plugin_sample_kit-1.0.0.hplugin");
        std::fs::write(&path, &archive)?;
        let bundle = self.load_source(&path, BundleOptions::default()).await?;
        self.install_bundle(bundle, "sample").await
    }

    async fn sign_files(
        &self,
        mut files: BTreeMap<String, Vec<u8>>,
    ) -> Result<BTreeMap<String, Vec<u8>>, PluginError> {
        let manifest_bytes = files
            .get(MANIFEST_FILE)
            .ok_or_else(|| PluginError::Invalid(format!("missing {MANIFEST_FILE}")))?;
        let mut manifest: PluginManifest = serde_json::from_slice(manifest_bytes)?;
        validate_manifest_structure(&manifest)?;
        let content_hash = compute_content_hash(&files)?;
        manifest.integrity = Some(Integrity {
            algorithm: "sha256".to_string(),
            digest: content_hash.clone(),
        });
        manifest.signature = None;
        let canonical = canonical_manifest(&manifest)?;
        let key = self.local_key().await?;
        let signature = key
            .signing_key
            .sign(signature_payload(&manifest, &content_hash, &canonical).as_bytes());
        manifest.signature = Some(PluginSignature {
            algorithm: "ed25519".to_string(),
            key_id: key.key_id,
            public_key: key.public_key,
            value: BASE64.encode(signature.to_bytes()),
        });
        files.insert(
            MANIFEST_FILE.to_string(),
            serde_json::to_vec_pretty(&manifest)?,
        );
        Ok(files)
    }

    async fn load_source(
        &self,
        path: &Path,
        options: BundleOptions,
    ) -> Result<PluginBundle, PluginError> {
        let files = if path.is_dir() {
            read_directory_files(path)?
        } else {
            let bytes = tokio::fs::read(path).await?;
            read_archive_files(&bytes)?
        };
        self.load_files(files, options).await
    }

    async fn load_files(
        &self,
        files: BTreeMap<String, Vec<u8>>,
        options: BundleOptions,
    ) -> Result<PluginBundle, PluginError> {
        let manifest_bytes = files
            .get(MANIFEST_FILE)
            .ok_or_else(|| PluginError::Invalid(format!("missing {MANIFEST_FILE}")))?;
        let manifest: PluginManifest = serde_json::from_slice(manifest_bytes)?;
        validate_manifest_structure(&manifest)?;
        validate_capability_paths(&files, &manifest)?;
        let content_hash = compute_content_hash(&files)?;
        let integrity = manifest
            .integrity
            .as_ref()
            .ok_or_else(|| PluginError::Invalid("manifest integrity is required".to_string()))?;
        if integrity.algorithm != "sha256" || integrity.digest != content_hash {
            return Err(PluginError::Signature(
                "manifest integrity digest does not match the package content".to_string(),
            ));
        }
        let trust = match &manifest.signature {
            Some(signature) => {
                let canonical = canonical_manifest(&manifest)?;
                let public_key = verify_signature(signature, &manifest, &content_hash, &canonical)?;
                let trusted: Option<String> = if let Some(store) = self.db.postgres_plugin() {
                    store
                        .trusted_key_trust(&signature.key_id, &public_key)
                        .await
                        .map_err(pg_plugin_error)?
                } else {
                    sqlx::query_scalar(
                        "SELECT trust FROM plugin_trusted_keys WHERE key_id = ? AND public_key = ?",
                    )
                    .bind(&signature.key_id)
                    .bind(&public_key)
                    .fetch_optional(&self.db.pool)
                    .await?
                };
                match trusted.as_deref() {
                    Some("verified") => "verified".to_string(),
                    _ => "community".to_string(),
                }
            }
            None => {
                if options.require_signature {
                    return Err(PluginError::Signature(
                        "this operation requires a signed plugin".to_string(),
                    ));
                }
                if !options.allow_unsigned {
                    return Err(PluginError::Signature(
                        "plugin is unsigned; pass allowUnsigned for a local development install"
                            .to_string(),
                    ));
                }
                "untrusted".to_string()
            }
        };
        if trust == "community" && !options.allow_untrusted && !options.require_signature {
            return Err(PluginError::Signature(format!(
                "plugin is signed by an unknown key `{}`; trust it or pass allowUntrusted",
                manifest
                    .signature
                    .as_ref()
                    .map(|signature| signature.key_id.as_str())
                    .unwrap_or("")
            )));
        }
        if !hpa_skill::allowed_license(&manifest.license) {
            return Err(PluginError::License(format!(
                "license `{}` is not in the local allowlist",
                manifest.license
            )));
        }
        let requirement = VersionReq::parse(&manifest.compatibility.hpa).map_err(|error| {
            PluginError::Invalid(format!("invalid compatibility.hpa range: {error}"))
        })?;
        let current = Version::parse(env!("CARGO_PKG_VERSION"))
            .map_err(|error| PluginError::Invalid(error.to_string()))?;
        if !requirement.matches(&current) {
            return Err(PluginError::Invalid(format!(
                "plugin requires hpa {} but runtime is {current}",
                manifest.compatibility.hpa
            )));
        }
        if !manifest.compatibility.os.is_empty()
            && !manifest
                .compatibility
                .os
                .iter()
                .any(|os| os == std::env::consts::OS)
        {
            return Err(PluginError::Invalid(format!(
                "plugin does not support {}",
                std::env::consts::OS
            )));
        }
        for dependency in &manifest.dependencies {
            if dependency.kind != "plugin" {
                return Err(PluginError::Dependency(format!(
                    "unsupported dependency kind `{}`",
                    dependency.kind
                )));
            }
            let row: Option<(String, String)> = if let Some(store) = self.db.postgres_plugin() {
                store
                    .plugin(&dependency.id)
                    .await
                    .map_err(pg_plugin_error)?
                    .map(|plugin| (plugin.installed_version.unwrap_or_default(), plugin.status))
            } else {
                sqlx::query_as("SELECT installed_version, status FROM plugins WHERE id = ?")
                    .bind(&dependency.id)
                    .fetch_optional(&self.db.pool)
                    .await?
            };
            let Some((installed, status)) = row else {
                return Err(PluginError::Dependency(format!(
                    "plugin dependency `{}` is not installed",
                    dependency.id
                )));
            };
            if status != "enabled" || installed.is_empty() {
                return Err(PluginError::Dependency(format!(
                    "plugin dependency `{}` is not enabled",
                    dependency.id
                )));
            }
            let range = VersionReq::parse(&dependency.range).map_err(|error| {
                PluginError::Dependency(format!(
                    "invalid range for dependency `{}`: {error}",
                    dependency.id
                ))
            })?;
            let version = Version::parse(&installed).map_err(|error| {
                PluginError::Dependency(format!(
                    "installed dependency `{}` has invalid version: {error}",
                    dependency.id
                ))
            })?;
            if !range.matches(&version) {
                return Err(PluginError::Dependency(format!(
                    "dependency `{}` requires {} but {} is installed",
                    dependency.id, dependency.range, installed
                )));
            }
        }
        self.validate_capability_content(&files, &manifest).await?;
        Ok(PluginBundle {
            manifest,
            files,
            content_hash,
            trust,
        })
    }

    async fn validate_capability_content(
        &self,
        files: &BTreeMap<String, Vec<u8>>,
        manifest: &PluginManifest,
    ) -> Result<(), PluginError> {
        for capability in &manifest.capabilities {
            match capability.kind {
                CapabilityKind::Connector => {
                    let bytes = files.get(&capability.path).ok_or_else(|| {
                        PluginError::Invalid(format!(
                            "connector capability path `{}` is missing",
                            capability.path
                        ))
                    })?;
                    let connector: ConnectorManifest = serde_json::from_slice(bytes)?;
                    connector::validate_manifest(&connector)?;
                    if connector.id != capability.id || connector.version != capability.version {
                        return Err(PluginError::Invalid(format!(
                            "connector capability `{}@{}` does not match `{}@{}`",
                            capability.id, capability.version, connector.id, connector.version
                        )));
                    }
                    validate_capability_permissions(manifest, &connector)?;
                }
                CapabilityKind::Mcp => {
                    let bytes = files.get(&capability.path).ok_or_else(|| {
                        PluginError::Invalid(format!(
                            "MCP capability path `{}` is missing",
                            capability.path
                        ))
                    })?;
                    let mcp: McpContribution = serde_json::from_slice(bytes)?;
                    mcp.validate(manifest, capability)?;
                }
                CapabilityKind::Skill => {
                    let prefix = format!("{}/", capability.path.trim_end_matches('/'));
                    if !files.contains_key(&format!("{prefix}SKILL.md")) {
                        return Err(PluginError::Invalid(format!(
                            "skill capability `{}` is missing SKILL.md",
                            capability.id
                        )));
                    }
                }
                CapabilityKind::Agent => {
                    let bytes = files.get(&capability.path).ok_or_else(|| {
                        PluginError::Invalid(format!(
                            "agent capability path `{}` is missing",
                            capability.path
                        ))
                    })?;
                    let config: Value = serde_json::from_slice(bytes)?;
                    let (definition, _) = agent::canonical_agent_value(&config)
                        .map_err(|issues| PluginError::Invalid(issues.join("; ")))?;
                    if definition.id != capability.id || definition.version != capability.version {
                        return Err(PluginError::Invalid(format!(
                            "agent capability `{}@{}` does not match `{}@{}`",
                            capability.id, capability.version, definition.id, definition.version
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    async fn store_package(
        &self,
        bundle: &PluginBundle,
        source: &str,
    ) -> Result<PluginPackageRow, PluginError> {
        let manifest = &bundle.manifest;
        let store = self.db.postgres_plugin();
        let existing: Option<String> = if let Some(store) = &store {
            store
                .package_content_hash(&manifest.id, &manifest.version)
                .await
                .map_err(pg_plugin_error)?
        } else {
            sqlx::query_scalar(
                "SELECT content_hash FROM plugin_packages WHERE plugin_id = ? AND version = ?",
            )
            .bind(&manifest.id)
            .bind(&manifest.version)
            .fetch_optional(&self.db.pool)
            .await?
        };
        if let Some(existing) = existing
            && existing != bundle.content_hash
        {
            return Err(PluginError::Frozen(format!(
                "plugin `{}@{}` is frozen with different content",
                manifest.id, manifest.version
            )));
        }
        let package_path = self.package_path(&manifest.id, &manifest.version);
        if package_path.exists() {
            if self.package_hash(&package_path).as_deref() != Some(bundle.content_hash.as_str()) {
                return Err(PluginError::Frozen(format!(
                    "plugin package `{}@{}` already exists with different content",
                    manifest.id, manifest.version
                )));
            }
        } else {
            if let Some(parent) = package_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let archive = pack_files(&bundle.files)?;
            let temp = package_path.with_extension("hplugin.tmp");
            std::fs::write(&temp, archive)?;
            std::fs::rename(&temp, &package_path)?;
        }
        let key = self.local_key().await?;
        let sbom = build_sbom(bundle, &key);
        let signature_json = manifest
            .signature
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        if let Some(store) = &store {
            store
                .upsert_package(
                    &manifest.id,
                    &manifest.version,
                    &manifest.name,
                    manifest.description.as_deref(),
                    &manifest.license,
                    manifest
                        .publisher
                        .as_ref()
                        .map(|publisher| publisher.name.as_str()),
                    &serde_json::to_string(manifest)?,
                    &bundle.content_hash,
                    &package_path.to_string_lossy(),
                    signature_json.as_deref(),
                    manifest
                        .signature
                        .as_ref()
                        .map(|signature| signature.key_id.as_str()),
                    manifest
                        .signature
                        .as_ref()
                        .map(|signature| signature.public_key.as_str()),
                    &bundle.trust,
                    source,
                    &serde_json::to_string(&manifest.capabilities)?,
                    &serde_json::to_string(&sbom)?,
                    &now_rfc3339(),
                )
                .await
                .map_err(pg_plugin_error)?;
        } else {
            sqlx::query(
                "INSERT INTO plugin_packages
                 (plugin_id, version, name, description, license, publisher, manifest_json, content_hash,
                  package_path, signature_json, signer_key_id, signer_public_key, trust, source,
                  capabilities_json, sbom_json, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(plugin_id, version) DO UPDATE SET
                    name = excluded.name,
                    description = excluded.description,
                    license = excluded.license,
                    publisher = excluded.publisher,
                    manifest_json = excluded.manifest_json,
                    content_hash = excluded.content_hash,
                    package_path = excluded.package_path,
                    signature_json = excluded.signature_json,
                    signer_key_id = excluded.signer_key_id,
                    signer_public_key = excluded.signer_public_key,
                    trust = excluded.trust,
                    source = excluded.source,
                    capabilities_json = excluded.capabilities_json,
                    sbom_json = excluded.sbom_json",
            )
            .bind(&manifest.id)
            .bind(&manifest.version)
            .bind(&manifest.name)
            .bind(&manifest.description)
            .bind(&manifest.license)
            .bind(manifest.publisher.as_ref().map(|publisher| publisher.name.clone()))
            .bind(serde_json::to_string(manifest)?)
            .bind(&bundle.content_hash)
            .bind(package_path.to_string_lossy())
            .bind(&signature_json)
            .bind(manifest.signature.as_ref().map(|signature| signature.key_id.clone()))
            .bind(manifest.signature.as_ref().map(|signature| signature.public_key.clone()))
            .bind(&bundle.trust)
            .bind(source)
            .bind(serde_json::to_string(&manifest.capabilities)?)
            .bind(serde_json::to_string(&sbom)?)
            .bind(now_rfc3339())
            .execute(&self.db.pool)
            .await?;
        }
        self.package(&manifest.id, &manifest.version).await
    }

    async fn install_bundle(
        &self,
        bundle: PluginBundle,
        source: &str,
    ) -> Result<Value, PluginError> {
        let manifest = &bundle.manifest;
        let vulnerability = self
            .vulnerability_report(&manifest.id, &manifest.version)
            .await?;
        if vulnerability["status"] == "vulnerable" {
            let ids = vulnerability["vulnerabilities"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item["id"].as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            return Err(PluginError::Forbidden(format!(
                "plugin `{}@{}` is blocked by vulnerabilities: {ids}",
                manifest.id, manifest.version
            )));
        }
        let plugin: Option<PluginRow> = if let Some(store) = self.db.postgres_plugin() {
            store
                .plugin(&manifest.id)
                .await
                .map_err(pg_plugin_error)?
                .map(plugin_row_from_pg)
        } else {
            sqlx::query_as(
                "SELECT id, name, latest_version, status, installed_version, previous_version,
                        trust, license, created_at, updated_at
                 FROM plugins WHERE id = ?",
            )
            .bind(&manifest.id)
            .fetch_optional(&self.db.pool)
            .await?
        };
        if let Some(plugin) = &plugin
            && let Some(installed) = &plugin.installed_version
        {
            if installed == &manifest.version {
                return self.detail(&manifest.id).await;
            }
            let installed_version = Version::parse(installed).map_err(|error| {
                PluginError::Invalid(format!("installed version is invalid: {error}"))
            })?;
            let incoming = Version::parse(&manifest.version).map_err(|error| {
                PluginError::Invalid(format!("incoming version is invalid: {error}"))
            })?;
            if incoming <= installed_version {
                return Err(PluginError::Conflict(format!(
                    "plugin `{}` version `{}` is not newer than installed `{installed}`; use rollback",
                    manifest.id, manifest.version
                )));
            }
        }
        self.store_package(&bundle, source).await?;
        let installed_dir = self.installed_dir(&manifest.id, &manifest.version);
        if !installed_dir.exists() {
            write_files(&installed_dir, &bundle.files)?;
        }
        let now = now_rfc3339();
        let previous = plugin.and_then(|plugin| plugin.installed_version);
        if let Some(store) = self.db.postgres_plugin() {
            store
                .upsert_plugin_summary(
                    &manifest.id,
                    &manifest.name,
                    &manifest.version,
                    "installed",
                    &bundle.trust,
                    &manifest.license,
                    &now,
                    &now,
                )
                .await
                .map_err(pg_plugin_error)?;
        } else {
            sqlx::query(
                "INSERT INTO plugins
                 (id, name, latest_version, status, installed_version, previous_version, trust, license, created_at, updated_at)
                 VALUES (?, ?, ?, 'installed', NULL, NULL, ?, ?, ?, ?)
                 ON CONFLICT(id) DO UPDATE SET
                    name = excluded.name,
                    latest_version = excluded.latest_version,
                    trust = excluded.trust,
                    license = excluded.license,
                    updated_at = excluded.updated_at",
            )
            .bind(&manifest.id)
            .bind(&manifest.name)
            .bind(&manifest.version)
            .bind(&bundle.trust)
            .bind(&manifest.license)
            .bind(&now)
            .bind(&now)
            .execute(&self.db.pool)
            .await?;
        }
        self.activate_version(&manifest.id, &manifest.version, previous.as_deref())
            .await?;
        if let Some(store) = self.db.postgres_plugin() {
            store
                .set_plugin_installed(
                    &manifest.id,
                    &manifest.version,
                    previous.as_deref(),
                    &manifest.version,
                    &now,
                )
                .await
                .map_err(pg_plugin_error)?;
        } else {
            sqlx::query(
                "UPDATE plugins SET installed_version = ?, previous_version = ?, status = 'enabled',
                        latest_version = ?, updated_at = ? WHERE id = ?",
            )
            .bind(&manifest.version)
            .bind(&previous)
            .bind(&manifest.version)
            .bind(&now)
            .bind(&manifest.id)
            .execute(&self.db.pool)
            .await?;
        }
        self.event(
            Some(&manifest.id),
            Some(&manifest.version),
            if previous.is_some() {
                "plugin.upgraded"
            } else {
                "plugin.installed"
            },
            json!({"trust": bundle.trust, "source": source, "previousVersion": previous}),
        )
        .await?;
        self.detail(&manifest.id).await
    }

    async fn activate_version(
        &self,
        plugin_id: &str,
        version: &str,
        previous: Option<&str>,
    ) -> Result<(), PluginError> {
        if let Some(previous) = previous {
            self.deactivate_version(plugin_id, previous).await?;
        }
        let package = self.package(plugin_id, version).await?;
        let manifest: PluginManifest = serde_json::from_str(&package.manifest_json)?;
        let installed_dir = self.installed_dir(plugin_id, version);
        for capability in &manifest.capabilities {
            let permissions_json = serde_json::to_string(&manifest.permissions)?;
            match capability.kind {
                CapabilityKind::Connector => {
                    let path = installed_dir.join(&capability.path);
                    let bytes = std::fs::read(&path)?;
                    let connector: ConnectorManifest = serde_json::from_slice(&bytes)?;
                    connector::validate_manifest(&connector)?;
                    validate_capability_permissions(&manifest, &connector)?;
                    self.connectors
                        .upsert_definition(&connector, Some(plugin_id), Some(version), "enabled")
                        .await?;
                    self.upsert_capability(
                        plugin_id,
                        version,
                        capability,
                        "enabled",
                        &serde_json::to_string(&connector)?,
                        &permissions_json,
                    )
                    .await?;
                }
                CapabilityKind::Mcp => {
                    let path = installed_dir.join(&capability.path);
                    let bytes = std::fs::read(&path)?;
                    let mcp: McpContribution = serde_json::from_slice(&bytes)?;
                    mcp.validate(&manifest, capability)?;
                    self.upsert_mcp_server(plugin_id, version, capability, &mcp)
                        .await?;
                    self.upsert_capability(
                        plugin_id,
                        version,
                        capability,
                        "enabled",
                        &serde_json::to_string(&mcp)?,
                        &permissions_json,
                    )
                    .await?;
                }
                CapabilityKind::Skill => {
                    let path = installed_dir.join(&capability.path);
                    let outcome = self.skills.import(&path).await?;
                    if outcome.summary.id != capability.id
                        || outcome.version.version != capability.version
                    {
                        return Err(PluginError::Invalid(format!(
                            "skill capability `{}@{}` resolved to `{}@{}`",
                            capability.id,
                            capability.version,
                            outcome.summary.id,
                            outcome.version.version
                        )));
                    }
                    if !outcome
                        .summary
                        .license
                        .as_deref()
                        .is_some_and(hpa_skill::allowed_license)
                    {
                        return Err(PluginError::License(format!(
                            "skill capability `{}` has a license outside the allowlist",
                            capability.id
                        )));
                    }
                    self.upsert_capability(
                        plugin_id,
                        version,
                        capability,
                        "enabled",
                        &serde_json::to_string(&json!({
                            "skillId": outcome.summary.id,
                            "skillVersion": outcome.summary.latest_version
                        }))?,
                        &permissions_json,
                    )
                    .await?;
                }
                CapabilityKind::Agent => {
                    let path = installed_dir.join(&capability.path);
                    let bytes = std::fs::read(&path)?;
                    let config: Value = serde_json::from_slice(&bytes)?;
                    let (definition, hash) = agent::canonical_agent_value(&config)
                        .map_err(|issues| PluginError::Invalid(issues.join("; ")))?;
                    if definition.id != capability.id || definition.version != capability.version {
                        return Err(PluginError::Invalid(format!(
                            "agent capability `{}@{}` does not match its config",
                            capability.id, capability.version
                        )));
                    }
                    self.db
                        .upsert_agent_version(&AgentVersionRow {
                            agent_id: definition.id.clone(),
                            version: definition.version.clone(),
                            name: definition.name.clone(),
                            description: definition.description.clone(),
                            config_json: serde_json::to_string(&definition)?,
                            content_hash: hash,
                            status: "imported".to_string(),
                            created_at: now_rfc3339(),
                            published_at: None,
                        })
                        .await?;
                    self.upsert_capability(
                        plugin_id,
                        version,
                        capability,
                        "enabled",
                        &serde_json::to_string(&definition)?,
                        &permissions_json,
                    )
                    .await?;
                }
            }
        }
        Ok(())
    }

    async fn deactivate_version(&self, plugin_id: &str, version: &str) -> Result<(), PluginError> {
        self.connectors
            .set_plugin_definitions_status(plugin_id, version, "disabled")
            .await?;
        if let Some(store) = self.db.postgres_mcp_meta() {
            store
                .set_plugin_servers_enabled(plugin_id, version, false)
                .await
                .map_err(pg_mcp_error)?;
        } else {
            sqlx::query(
                "UPDATE mcp_servers SET enabled = 0 WHERE plugin_id = ? AND plugin_version = ?",
            )
            .bind(plugin_id)
            .bind(version)
            .execute(&self.db.pool)
            .await?;
        }
        if let Some(store) = self.db.postgres_plugin() {
            store
                .set_capabilities_status(plugin_id, version, "disabled")
                .await
                .map_err(pg_plugin_error)?;
        } else {
            sqlx::query(
                "UPDATE plugin_capabilities SET status = 'disabled'
                 WHERE plugin_id = ? AND version = ?",
            )
            .bind(plugin_id)
            .bind(version)
            .execute(&self.db.pool)
            .await?;
        }
        Ok(())
    }

    async fn activate_existing(&self, plugin_id: &str, version: &str) -> Result<(), PluginError> {
        self.activate_version(plugin_id, version, None).await?;
        if let Some(store) = self.db.postgres_mcp_meta() {
            store
                .set_plugin_servers_enabled(plugin_id, version, true)
                .await
                .map_err(pg_mcp_error)?;
        } else {
            sqlx::query(
                "UPDATE mcp_servers SET enabled = 1 WHERE plugin_id = ? AND plugin_version = ?",
            )
            .bind(plugin_id)
            .bind(version)
            .execute(&self.db.pool)
            .await?;
        }
        if let Some(store) = self.db.postgres_plugin() {
            store
                .set_capabilities_status(plugin_id, version, "enabled")
                .await
                .map_err(pg_plugin_error)?;
        } else {
            sqlx::query(
                "UPDATE plugin_capabilities SET status = 'enabled'
                 WHERE plugin_id = ? AND version = ?",
            )
            .bind(plugin_id)
            .bind(version)
            .execute(&self.db.pool)
            .await?;
        }
        Ok(())
    }

    async fn upsert_capability(
        &self,
        plugin_id: &str,
        version: &str,
        capability: &PluginCapability,
        status: &str,
        config_json: &str,
        permissions_json: &str,
    ) -> Result<(), PluginError> {
        if let Some(store) = self.db.postgres_plugin() {
            return store
                .upsert_capability(
                    plugin_id,
                    version,
                    capability.kind.as_str(),
                    &capability.id,
                    &capability.version,
                    &capability.path,
                    status,
                    config_json,
                    permissions_json,
                )
                .await
                .map_err(pg_plugin_error);
        }
        sqlx::query(
            "INSERT INTO plugin_capabilities
             (plugin_id, version, kind, capability_id, capability_version, path, status, config_json, permissions_json)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(plugin_id, version, kind, capability_id) DO UPDATE SET
                capability_version = excluded.capability_version,
                path = excluded.path,
                status = excluded.status,
                config_json = excluded.config_json,
                permissions_json = excluded.permissions_json",
        )
        .bind(plugin_id)
        .bind(version)
        .bind(capability.kind.as_str())
        .bind(&capability.id)
        .bind(&capability.version)
        .bind(&capability.path)
        .bind(status)
        .bind(config_json)
        .bind(permissions_json)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    async fn upsert_mcp_server(
        &self,
        plugin_id: &str,
        version: &str,
        capability: &PluginCapability,
        mcp: &McpContribution,
    ) -> Result<(), PluginError> {
        let id = plugin_server_id(plugin_id, &capability.id);
        if let Some(store) = self.db.postgres_mcp_meta() {
            if let Some((owner, _)) = store.plugin_server_owner(&id).await.map_err(pg_mcp_error)?
                && owner.as_deref() != Some(plugin_id)
            {
                return Err(PluginError::Conflict(format!(
                    "MCP server id `{id}` already exists or belongs to another plugin"
                )));
            }
            let now = now_rfc3339();
            return store
                .upsert_plugin_server(&McpPluginServerInput {
                    id,
                    name: mcp.name.clone(),
                    transport: mcp.transport.clone(),
                    command_json: serde_json::to_string(&mcp.command)?,
                    url: mcp.url.clone(),
                    allowed_tools_json: serde_json::to_string(&mcp.allowed_tools)?,
                    network_json: serde_json::to_string(&mcp.network)?,
                    credential_env: mcp.credential_env.clone(),
                    plugin_id: plugin_id.to_string(),
                    plugin_version: version.to_string(),
                    created_at: now.clone(),
                    updated_at: now,
                })
                .await
                .map_err(pg_mcp_error);
        }
        let existing: Option<(Option<String>, Option<String>)> =
            sqlx::query_as("SELECT plugin_id, plugin_version FROM mcp_servers WHERE id = ?")
                .bind(&id)
                .fetch_optional(&self.db.pool)
                .await?;
        if let Some((owner, _)) = &existing
            && owner.as_deref() != Some(plugin_id)
        {
            return Err(PluginError::Conflict(format!(
                "MCP server id `{id}` already exists or belongs to another plugin"
            )));
        }
        sqlx::query(
            "INSERT INTO mcp_servers
             (id, name, transport, command_json, url, allowed_tools_json, network_json,
              credential_env, created_at, updated_at, plugin_id, plugin_version, enabled)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                transport = excluded.transport,
                command_json = excluded.command_json,
                url = excluded.url,
                allowed_tools_json = excluded.allowed_tools_json,
                network_json = excluded.network_json,
                credential_env = excluded.credential_env,
                updated_at = excluded.updated_at,
                plugin_id = excluded.plugin_id,
                plugin_version = excluded.plugin_version,
                enabled = 1",
        )
        .bind(&id)
        .bind(&mcp.name)
        .bind(&mcp.transport)
        .bind(serde_json::to_string(&mcp.command)?)
        .bind(&mcp.url)
        .bind(serde_json::to_string(&mcp.allowed_tools)?)
        .bind(serde_json::to_string(&mcp.network)?)
        .bind(&mcp.credential_env)
        .bind(now_rfc3339())
        .bind(now_rfc3339())
        .bind(plugin_id)
        .bind(version)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    pub async fn list(&self) -> Result<Vec<Value>, PluginError> {
        let rows = if let Some(store) = self.db.postgres_plugin() {
            store
                .list_plugins()
                .await
                .map_err(pg_plugin_error)?
                .into_iter()
                .map(plugin_row_from_pg)
                .collect::<Vec<_>>()
        } else {
            sqlx::query_as::<_, PluginRow>(
                "SELECT id, name, latest_version, status, installed_version, previous_version,
                        trust, license, created_at, updated_at
                 FROM plugins ORDER BY updated_at DESC",
            )
            .fetch_all(&self.db.pool)
            .await?
        };
        Ok(rows.iter().map(plugin_row_value).collect())
    }

    pub async fn detail(&self, id: &str) -> Result<Value, PluginError> {
        let plugin = if let Some(store) = self.db.postgres_plugin() {
            store
                .plugin(id)
                .await
                .map_err(pg_plugin_error)?
                .map(plugin_row_from_pg)
        } else {
            sqlx::query_as::<_, PluginRow>(
                "SELECT id, name, latest_version, status, installed_version, previous_version,
                        trust, license, created_at, updated_at
                 FROM plugins WHERE id = ?",
            )
            .bind(id)
            .fetch_optional(&self.db.pool)
            .await?
        }
        .ok_or_else(|| PluginError::NotFound(format!("plugin `{id}` not found")))?;
        let packages = if let Some(store) = self.db.postgres_plugin() {
            store
                .list_packages(id)
                .await
                .map_err(pg_plugin_error)?
                .into_iter()
                .map(package_row_from_pg)
                .collect::<Vec<_>>()
        } else {
            sqlx::query_as::<_, PluginPackageRow>(
                "SELECT plugin_id, version, name, description, license, publisher, manifest_json,
                        content_hash, package_path, signature_json, signer_key_id, signer_public_key,
                        trust, source, capabilities_json, sbom_json, created_at
                 FROM plugin_packages WHERE plugin_id = ? ORDER BY created_at DESC",
            )
            .bind(id)
            .fetch_all(&self.db.pool)
            .await?
        };
        let installed_version = plugin.installed_version.as_deref();
        let capabilities = if let Some(version) = installed_version {
            self.capabilities(id, version).await?
        } else {
            Vec::new()
        };
        Ok(json!({
            "plugin": plugin_row_value(&plugin),
            "packages": packages.iter().map(package_value).collect::<Vec<_>>(),
            "installedPackage": installed_version.and_then(|version| packages.iter().find(|package| package.version == version)).map(package_value),
            "capabilities": capabilities,
            "sbom": installed_version.and_then(|version| packages.iter().find(|package| package.version == version)).and_then(|package| serde_json::from_str::<Value>(&package.sbom_json).ok()),
            "events": self.events(id, 50).await?,
        }))
    }

    pub async fn package_detail(&self, id: &str, version: &str) -> Result<Value, PluginError> {
        let package = self.package(id, version).await?;
        let manifest: Value = serde_json::from_str(&package.manifest_json)?;
        Ok(json!({
            "package": package_value(&package),
            "manifest": manifest,
            "capabilities": self.capabilities(id, version).await?,
            "sbom": serde_json::from_str::<Value>(&package.sbom_json).unwrap_or(Value::Null),
        }))
    }

    async fn capabilities(
        &self,
        plugin_id: &str,
        version: &str,
    ) -> Result<Vec<Value>, PluginError> {
        let rows = if let Some(store) = self.db.postgres_plugin() {
            store
                .capabilities(plugin_id, version)
                .await
                .map_err(pg_plugin_error)?
                .into_iter()
                .map(capability_row_from_pg)
                .collect::<Vec<_>>()
        } else {
            sqlx::query_as::<_, PluginCapabilityRow>(
                "SELECT plugin_id, version, kind, capability_id, capability_version, path, status,
                        config_json, permissions_json
                 FROM plugin_capabilities WHERE plugin_id = ? AND version = ?
                 ORDER BY kind ASC, capability_id ASC",
            )
            .bind(plugin_id)
            .bind(version)
            .fetch_all(&self.db.pool)
            .await?
        };
        Ok(rows.iter().map(capability_row_value).collect())
    }

    pub async fn enable(&self, id: &str) -> Result<Value, PluginError> {
        let plugin = self.plugin_row(id).await?;
        let version = plugin
            .installed_version
            .ok_or_else(|| PluginError::Conflict("plugin has no installed version".to_string()))?;
        self.activate_existing(id, &version).await?;
        if let Some(store) = self.db.postgres_plugin() {
            store
                .set_plugin_status(id, "enabled", &now_rfc3339())
                .await
                .map_err(pg_plugin_error)?;
        } else {
            sqlx::query("UPDATE plugins SET status = 'enabled', updated_at = ? WHERE id = ?")
                .bind(now_rfc3339())
                .bind(id)
                .execute(&self.db.pool)
                .await?;
        }
        self.event(Some(id), Some(&version), "plugin.enabled", json!({}))
            .await?;
        self.detail(id).await
    }

    pub async fn disable(&self, id: &str) -> Result<Value, PluginError> {
        let plugin = self.plugin_row(id).await?;
        if let Some(version) = &plugin.installed_version {
            self.deactivate_version(id, version).await?;
        }
        if let Some(store) = self.db.postgres_plugin() {
            store
                .set_plugin_status(id, "disabled", &now_rfc3339())
                .await
                .map_err(pg_plugin_error)?;
        } else {
            sqlx::query("UPDATE plugins SET status = 'disabled', updated_at = ? WHERE id = ?")
                .bind(now_rfc3339())
                .bind(id)
                .execute(&self.db.pool)
                .await?;
        }
        self.event(
            Some(id),
            plugin.installed_version.as_deref(),
            "plugin.disabled",
            json!({}),
        )
        .await?;
        self.detail(id).await
    }

    pub async fn rollback(&self, id: &str) -> Result<Value, PluginError> {
        let plugin = self.plugin_row(id).await?;
        let previous = plugin
            .previous_version
            .clone()
            .ok_or_else(|| PluginError::Conflict("plugin has no previous version".to_string()))?;
        let current = plugin
            .installed_version
            .clone()
            .ok_or_else(|| PluginError::Conflict("plugin has no installed version".to_string()))?;
        self.deactivate_version(id, &current).await?;
        self.activate_version(id, &previous, None).await?;
        if let Some(store) = self.db.postgres_plugin() {
            store
                .set_plugin_rollback(id, &previous, &current, &now_rfc3339())
                .await
                .map_err(pg_plugin_error)?;
        } else {
            sqlx::query(
                "UPDATE plugins SET status = 'enabled', installed_version = ?, previous_version = ?, updated_at = ?
                 WHERE id = ?",
            )
            .bind(&previous)
            .bind(&current)
            .bind(now_rfc3339())
            .bind(id)
            .execute(&self.db.pool)
            .await?;
        }
        self.event(
            Some(id),
            Some(&previous),
            "plugin.rolledback",
            json!({"from": current}),
        )
        .await?;
        self.detail(id).await
    }

    pub async fn uninstall(&self, id: &str, force: bool) -> Result<Value, PluginError> {
        let plugin = self.plugin_row(id).await?;
        if !force {
            let dependents = self.dependents(id).await?;
            if !dependents.is_empty() {
                return Err(PluginError::Conflict(format!(
                    "plugin `{id}` is required by: {}",
                    dependents.join(", ")
                )));
            }
        }
        if let Some(version) = &plugin.installed_version {
            self.deactivate_version(id, version).await?;
        }
        self.connectors.delete_plugin_definitions(id).await?;
        if let Some(store) = self.db.postgres_mcp_meta() {
            store
                .delete_plugin_servers(id)
                .await
                .map_err(pg_mcp_error)?;
        } else {
            sqlx::query("DELETE FROM mcp_servers WHERE plugin_id = ?")
                .bind(id)
                .execute(&self.db.pool)
                .await?;
        }
        if let Some(store) = self.db.postgres_plugin() {
            store
                .set_capabilities_uninstalled(id)
                .await
                .map_err(pg_plugin_error)?;
            store
                .set_plugin_uninstalled(id, &now_rfc3339())
                .await
                .map_err(pg_plugin_error)?;
        } else {
            sqlx::query(
                "UPDATE plugin_capabilities SET status = 'uninstalled' WHERE plugin_id = ?",
            )
            .bind(id)
            .execute(&self.db.pool)
            .await?;
            sqlx::query(
                "UPDATE plugins SET status = 'uninstalled', installed_version = NULL,
                        previous_version = NULL, updated_at = ? WHERE id = ?",
            )
            .bind(now_rfc3339())
            .bind(id)
            .execute(&self.db.pool)
            .await?;
        }
        let installed = self.data_dir.join("plugins").join("installed").join(id);
        if installed.exists() {
            let _ = std::fs::remove_dir_all(&installed);
        }
        self.event(
            Some(id),
            plugin.installed_version.as_deref(),
            "plugin.uninstalled",
            json!({"force": force}),
        )
        .await?;
        self.detail(id).await
    }

    async fn dependents(&self, id: &str) -> Result<Vec<String>, PluginError> {
        let packages = if let Some(store) = self.db.postgres_plugin() {
            store
                .installed_packages()
                .await
                .map_err(pg_plugin_error)?
                .into_iter()
                .map(package_row_from_pg)
                .collect::<Vec<_>>()
        } else {
            sqlx::query_as::<_, PluginPackageRow>(
                "SELECT plugin_id, version, name, description, license, publisher, manifest_json,
                        content_hash, package_path, signature_json, signer_key_id, signer_public_key,
                        trust, source, capabilities_json, sbom_json, created_at
                 FROM plugin_packages p WHERE p.version = (
                    SELECT installed_version FROM plugins WHERE id = p.plugin_id AND status != 'uninstalled'
                 )",
            )
            .fetch_all(&self.db.pool)
            .await?
        };
        let mut dependents = Vec::new();
        for package in packages {
            if package.plugin_id == id {
                continue;
            }
            let Ok(manifest) = serde_json::from_str::<PluginManifest>(&package.manifest_json)
            else {
                continue;
            };
            if manifest
                .dependencies
                .iter()
                .any(|dependency| dependency.id == id)
            {
                dependents.push(format!("{}@{}", package.plugin_id, package.version));
            }
        }
        Ok(dependents)
    }

    pub async fn set_marketplace_listing(
        &self,
        plugin_id: &str,
        version: &str,
        status: &str,
        note: Option<&str>,
    ) -> Result<Value, PluginError> {
        let allowed = ["approved", "rejected"];
        if !allowed.contains(&status) {
            return Err(PluginError::Invalid(
                "review status must be approved or rejected".to_string(),
            ));
        }
        let updated = if let Some(store) = self.db.postgres_plugin() {
            store
                .review_listing(plugin_id, version, status, note, &now_rfc3339())
                .await
                .map_err(pg_plugin_error)? as usize
        } else {
            sqlx::query(
                "UPDATE plugin_market_listings SET review_status = ?, review_note = ?, reviewed_at = ?
                 WHERE plugin_id = ? AND version = ? AND review_status = 'pending'",
            )
            .bind(status)
            .bind(note)
            .bind(now_rfc3339())
            .bind(plugin_id)
            .bind(version)
            .execute(&self.db.pool)
            .await?
            .rows_affected() as usize
        };
        if updated == 0 {
            return Err(PluginError::Conflict(
                "listing is not pending review".to_string(),
            ));
        }
        self.event(
            Some(plugin_id),
            Some(version),
            "marketplace.reviewed",
            json!({"status": status, "note": note}),
        )
        .await?;
        self.marketplace_detail(plugin_id, version).await
    }

    pub async fn withdraw_marketplace(
        &self,
        plugin_id: &str,
        version: &str,
    ) -> Result<Value, PluginError> {
        let updated = if let Some(store) = self.db.postgres_plugin() {
            store
                .withdraw_listing(plugin_id, version, &now_rfc3339())
                .await
                .map_err(pg_plugin_error)? as usize
        } else {
            sqlx::query(
                "UPDATE plugin_market_listings SET review_status = 'withdrawn', withdrawn_at = ?
                 WHERE plugin_id = ? AND version = ? AND review_status != 'withdrawn'",
            )
            .bind(now_rfc3339())
            .bind(plugin_id)
            .bind(version)
            .execute(&self.db.pool)
            .await?
            .rows_affected() as usize
        };
        if updated == 0 {
            return Err(PluginError::NotFound(
                "active listing not found".to_string(),
            ));
        }
        self.event(
            Some(plugin_id),
            Some(version),
            "marketplace.withdrawn",
            json!({}),
        )
        .await?;
        self.marketplace_detail(plugin_id, version).await
    }

    pub async fn publish_marketplace(
        &self,
        path: &Path,
        options: BundleOptions,
    ) -> Result<Value, PluginError> {
        let bundle = self
            .load_source(
                path,
                BundleOptions {
                    allow_unsigned: options.allow_unsigned,
                    allow_untrusted: true,
                    require_signature: true,
                },
            )
            .await?;
        if bundle.manifest.signature.is_none() {
            return Err(PluginError::Signature(
                "marketplace submissions must be signed".to_string(),
            ));
        }
        self.store_package(&bundle, "marketplace").await?;
        let plugin_id = &bundle.manifest.id;
        let version = &bundle.manifest.version;
        let now = now_rfc3339();
        if let Some(store) = self.db.postgres_plugin() {
            store
                .upsert_listing_pending(plugin_id, version, &now)
                .await
                .map_err(pg_plugin_error)?;
            store
                .upsert_plugin_summary(
                    plugin_id,
                    &bundle.manifest.name,
                    version,
                    "available",
                    &bundle.trust,
                    &bundle.manifest.license,
                    &now,
                    &now,
                )
                .await
                .map_err(pg_plugin_error)?;
        } else {
            sqlx::query(
                "INSERT INTO plugin_market_listings
                 (plugin_id, version, review_status, review_note, submitted_at, reviewed_at, withdrawn_at, downloads)
                 VALUES (?, ?, 'pending', NULL, ?, NULL, NULL, 0)
                 ON CONFLICT(plugin_id, version) DO UPDATE SET
                    review_status = 'pending',
                    review_note = NULL,
                    submitted_at = excluded.submitted_at,
                    reviewed_at = NULL,
                    withdrawn_at = NULL",
            )
            .bind(plugin_id)
            .bind(version)
            .bind(&now)
            .execute(&self.db.pool)
            .await?;
            sqlx::query(
                "INSERT INTO plugins
                 (id, name, latest_version, status, installed_version, previous_version, trust, license, created_at, updated_at)
                 VALUES (?, ?, ?, 'available', NULL, NULL, ?, ?, ?, ?)
                 ON CONFLICT(id) DO UPDATE SET
                    latest_version = excluded.latest_version,
                    trust = excluded.trust,
                    license = excluded.license,
                    updated_at = excluded.updated_at",
            )
            .bind(plugin_id)
            .bind(&bundle.manifest.name)
            .bind(version)
            .bind(&bundle.trust)
            .bind(&bundle.manifest.license)
            .bind(&now)
            .bind(&now)
            .execute(&self.db.pool)
            .await?;
        }
        self.event(
            Some(plugin_id),
            Some(version),
            "marketplace.submitted",
            json!({"trust": bundle.trust}),
        )
        .await?;
        self.marketplace_detail(plugin_id, version).await
    }

    pub async fn marketplace_list(&self) -> Result<Vec<Value>, PluginError> {
        let listings = if let Some(store) = self.db.postgres_plugin() {
            store
                .list_listings()
                .await
                .map_err(pg_plugin_error)?
                .into_iter()
                .map(listing_row_from_pg)
                .collect::<Vec<_>>()
        } else {
            sqlx::query_as::<_, MarketplaceListingRow>(
                "SELECT plugin_id, version, review_status, review_note, submitted_at, reviewed_at,
                        withdrawn_at, downloads
                 FROM plugin_market_listings ORDER BY submitted_at DESC",
            )
            .fetch_all(&self.db.pool)
            .await?
        };
        let mut values = Vec::new();
        for listing in listings {
            let package = if let Some(store) = self.db.postgres_plugin() {
                store
                    .package(&listing.plugin_id, &listing.version)
                    .await
                    .map_err(pg_plugin_error)?
                    .map(package_row_from_pg)
            } else {
                sqlx::query_as::<_, PluginPackageRow>(
                    "SELECT plugin_id, version, name, description, license, publisher, manifest_json,
                            content_hash, package_path, signature_json, signer_key_id, signer_public_key,
                            trust, source, capabilities_json, sbom_json, created_at
                     FROM plugin_packages WHERE plugin_id = ? AND version = ?",
                )
                .bind(&listing.plugin_id)
                .bind(&listing.version)
                .fetch_optional(&self.db.pool)
                .await?
            };
            let Some(package) = package else {
                continue;
            };
            let rating = self.rating(&listing.plugin_id, &listing.version).await?;
            values.push(json!({
                "pluginId": listing.plugin_id,
                "version": listing.version,
                "name": package.name,
                "description": package.description,
                "license": package.license,
                "publisher": package.publisher,
                "trust": package.trust,
                "contentHash": package.content_hash,
                "reviewStatus": listing.review_status,
                "reviewNote": listing.review_note,
                "submittedAt": listing.submitted_at,
                "reviewedAt": listing.reviewed_at,
                "withdrawnAt": listing.withdrawn_at,
                "downloads": listing.downloads,
                "rating": rating,
            }));
        }
        Ok(values)
    }

    pub async fn marketplace_detail(
        &self,
        plugin_id: &str,
        version: &str,
    ) -> Result<Value, PluginError> {
        let listing = if let Some(store) = self.db.postgres_plugin() {
            store
                .listing(plugin_id, version)
                .await
                .map_err(pg_plugin_error)?
                .map(listing_row_from_pg)
        } else {
            sqlx::query_as::<_, MarketplaceListingRow>(
                "SELECT plugin_id, version, review_status, review_note, submitted_at, reviewed_at,
                        withdrawn_at, downloads
                 FROM plugin_market_listings WHERE plugin_id = ? AND version = ?",
            )
            .bind(plugin_id)
            .bind(version)
            .fetch_optional(&self.db.pool)
            .await?
        }
        .ok_or_else(|| PluginError::NotFound("marketplace listing not found".to_string()))?;
        let package = self.package(plugin_id, version).await?;
        let manifest: Value = serde_json::from_str(&package.manifest_json)?;
        let reviews = if let Some(store) = self.db.postgres_plugin() {
            store
                .reviews(plugin_id, version)
                .await
                .map_err(pg_plugin_error)?
                .into_iter()
                .map(review_row_from_pg)
                .collect::<Vec<_>>()
        } else {
            sqlx::query_as::<_, MarketplaceReviewRow>(
                "SELECT id, plugin_id, version, rating, comment, created_at
                 FROM plugin_reviews WHERE plugin_id = ? AND version = ? ORDER BY created_at DESC",
            )
            .bind(plugin_id)
            .bind(version)
            .fetch_all(&self.db.pool)
            .await?
        };
        Ok(json!({
            "pluginId": plugin_id,
            "version": version,
            "manifest": manifest,
            "package": package_value(&package),
            "reviewStatus": listing.review_status,
            "reviewNote": listing.review_note,
            "submittedAt": listing.submitted_at,
            "reviewedAt": listing.reviewed_at,
            "withdrawnAt": listing.withdrawn_at,
            "downloads": listing.downloads,
            "rating": self.rating(plugin_id, version).await?,
            "reviews": reviews.iter().map(review_value).collect::<Vec<_>>(),
            "sbom": serde_json::from_str::<Value>(&package.sbom_json).unwrap_or(Value::Null),
        }))
    }

    pub async fn rate_marketplace(
        &self,
        plugin_id: &str,
        version: &str,
        rating: i64,
        comment: Option<&str>,
    ) -> Result<Value, PluginError> {
        if !(1..=5).contains(&rating) {
            return Err(PluginError::Invalid("rating must be 1..5".to_string()));
        }
        let exists = if let Some(store) = self.db.postgres_plugin() {
            store
                .listing_exists(plugin_id, version)
                .await
                .map_err(pg_plugin_error)?
        } else {
            sqlx::query_scalar::<_, i64>(
                "SELECT 1 FROM plugin_market_listings WHERE plugin_id = ? AND version = ?",
            )
            .bind(plugin_id)
            .bind(version)
            .fetch_optional(&self.db.pool)
            .await?
            .is_some()
        };
        if !exists {
            return Err(PluginError::NotFound(
                "marketplace listing not found".to_string(),
            ));
        }
        if let Some(store) = self.db.postgres_plugin() {
            store
                .insert_review(
                    &format!("plreview_{}", Uuid::now_v7()),
                    plugin_id,
                    version,
                    rating,
                    comment,
                    &now_rfc3339(),
                )
                .await
                .map_err(pg_plugin_error)?;
        } else {
            sqlx::query(
                "INSERT INTO plugin_reviews (id, plugin_id, version, rating, comment, created_at)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(format!("plreview_{}", Uuid::now_v7()))
            .bind(plugin_id)
            .bind(version)
            .bind(rating)
            .bind(comment)
            .bind(now_rfc3339())
            .execute(&self.db.pool)
            .await?;
        }
        self.event(
            Some(plugin_id),
            Some(version),
            "marketplace.rated",
            json!({"rating": rating}),
        )
        .await?;
        self.marketplace_detail(plugin_id, version).await
    }

    async fn rating(&self, plugin_id: &str, version: &str) -> Result<Value, PluginError> {
        let row: (Option<f64>, i64) = if let Some(store) = self.db.postgres_plugin() {
            let (average, count) = store
                .rating(plugin_id, version)
                .await
                .map_err(pg_plugin_error)?;
            (Some(average), count)
        } else {
            sqlx::query_as(
                "SELECT AVG(rating), COUNT(*) FROM plugin_reviews WHERE plugin_id = ? AND version = ?",
            )
            .bind(plugin_id)
            .bind(version)
            .fetch_one(&self.db.pool)
            .await?
        };
        Ok(json!({"average": row.0, "count": row.1}))
    }

    pub async fn install_marketplace(
        &self,
        plugin_id: &str,
        version: &str,
    ) -> Result<Value, PluginError> {
        let listing = if let Some(store) = self.db.postgres_plugin() {
            store
                .listing(plugin_id, version)
                .await
                .map_err(pg_plugin_error)?
                .map(listing_row_from_pg)
        } else {
            sqlx::query_as::<_, MarketplaceListingRow>(
                "SELECT plugin_id, version, review_status, review_note, submitted_at, reviewed_at,
                        withdrawn_at, downloads
                 FROM plugin_market_listings WHERE plugin_id = ? AND version = ?",
            )
            .bind(plugin_id)
            .bind(version)
            .fetch_optional(&self.db.pool)
            .await?
        }
        .ok_or_else(|| PluginError::NotFound("marketplace listing not found".to_string()))?;
        if listing.review_status != "approved" || listing.withdrawn_at.is_some() {
            return Err(PluginError::Forbidden(
                "marketplace package is not approved or was withdrawn".to_string(),
            ));
        }
        let package = self.package(plugin_id, version).await?;
        let bundle = self
            .load_source(
                Path::new(&package.package_path),
                BundleOptions {
                    allow_unsigned: false,
                    allow_untrusted: true,
                    require_signature: true,
                },
            )
            .await?;
        let detail = self.install_bundle(bundle, "marketplace").await?;
        if let Some(store) = self.db.postgres_plugin() {
            store
                .increment_downloads(plugin_id, version)
                .await
                .map_err(pg_plugin_error)?;
        } else {
            sqlx::query(
                "UPDATE plugin_market_listings SET downloads = downloads + 1
                 WHERE plugin_id = ? AND version = ?",
            )
            .bind(plugin_id)
            .bind(version)
            .execute(&self.db.pool)
            .await?;
        }
        Ok(detail)
    }

    pub async fn events(&self, plugin_id: &str, limit: i64) -> Result<Vec<Value>, PluginError> {
        let rows = if let Some(store) = self.db.postgres_plugin() {
            store
                .events(plugin_id, limit)
                .await
                .map_err(pg_plugin_error)?
                .into_iter()
                .map(event_row_from_pg)
                .collect::<Vec<_>>()
        } else {
            sqlx::query_as::<_, PluginEventRow>(
                "SELECT id, at, plugin_id, version, kind, detail_json FROM plugin_events
                 WHERE plugin_id = ? ORDER BY at DESC LIMIT ?",
            )
            .bind(plugin_id)
            .bind(limit.clamp(1, 200))
            .fetch_all(&self.db.pool)
            .await?
        };
        Ok(rows
            .into_iter()
            .map(|row| {
                json!({
                    "id": row.id,
                    "at": row.at,
                    "pluginId": row.plugin_id,
                    "version": row.version,
                    "kind": row.kind,
                    "detail": serde_json::from_str::<Value>(&row.detail_json).unwrap_or(Value::Null),
                })
            })
            .collect())
    }

    pub async fn event(
        &self,
        plugin_id: Option<&str>,
        version: Option<&str>,
        kind: &str,
        detail: Value,
    ) -> Result<(), PluginError> {
        let id = format!("plevent_{}", Uuid::now_v7());
        let at = now_rfc3339();
        let detail_json = serde_json::to_string(&detail)?;
        if let Some(store) = self.db.postgres_plugin() {
            store
                .insert_event(&id, &at, plugin_id, version, kind, &detail_json)
                .await
                .map_err(pg_plugin_error)?;
            return Ok(());
        }
        let mut transaction = self.db.pool.begin().await?;
        sqlx::query(
            "INSERT INTO plugin_events (id, at, plugin_id, version, kind, detail_json)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&at)
        .bind(plugin_id)
        .bind(version)
        .bind(kind)
        .bind(&detail_json)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO marketplace_event_stream
             (id, at, plugin_id, version, kind, detail_json)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&at)
        .bind(plugin_id)
        .bind(version)
        .bind(kind)
        .bind(&detail_json)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    async fn plugin_row(&self, id: &str) -> Result<PluginRow, PluginError> {
        if let Some(store) = self.db.postgres_plugin() {
            return store
                .plugin(id)
                .await
                .map_err(pg_plugin_error)?
                .map(plugin_row_from_pg)
                .ok_or_else(|| PluginError::NotFound(format!("plugin `{id}` not found")));
        }
        sqlx::query_as::<_, PluginRow>(
            "SELECT id, name, latest_version, status, installed_version, previous_version,
                    trust, license, created_at, updated_at
             FROM plugins WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.db.pool)
        .await?
        .ok_or_else(|| PluginError::NotFound(format!("plugin `{id}` not found")))
    }

    async fn package(
        &self,
        plugin_id: &str,
        version: &str,
    ) -> Result<PluginPackageRow, PluginError> {
        if let Some(store) = self.db.postgres_plugin() {
            return store
                .package(plugin_id, version)
                .await
                .map_err(pg_plugin_error)?
                .map(package_row_from_pg)
                .ok_or_else(|| {
                    PluginError::NotFound(format!(
                        "plugin package `{plugin_id}@{version}` not found"
                    ))
                });
        }
        sqlx::query_as::<_, PluginPackageRow>(
            "SELECT plugin_id, version, name, description, license, publisher, manifest_json,
                    content_hash, package_path, signature_json, signer_key_id, signer_public_key,
                    trust, source, capabilities_json, sbom_json, created_at
             FROM plugin_packages WHERE plugin_id = ? AND version = ?",
        )
        .bind(plugin_id)
        .bind(version)
        .fetch_optional(&self.db.pool)
        .await?
        .ok_or_else(|| {
            PluginError::NotFound(format!("plugin package `{plugin_id}@{version}` not found"))
        })
    }

    fn package_path(&self, plugin_id: &str, version: &str) -> PathBuf {
        self.data_dir
            .join("plugins")
            .join("packages")
            .join(plugin_id)
            .join(format!("{version}.hplugin"))
    }

    fn installed_dir(&self, plugin_id: &str, version: &str) -> PathBuf {
        self.data_dir
            .join("plugins")
            .join("installed")
            .join(plugin_id)
            .join(version)
    }

    fn package_hash(&self, path: &Path) -> Option<String> {
        let bytes = std::fs::read(path).ok()?;
        let files = read_archive_files(&bytes).ok()?;
        compute_content_hash(&files).ok()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpContribution {
    pub name: String,
    pub transport: String,
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub network: Vec<String>,
    #[serde(default)]
    pub credential_env: Option<String>,
}

impl McpContribution {
    fn validate(
        &self,
        manifest: &PluginManifest,
        capability: &PluginCapability,
    ) -> Result<(), PluginError> {
        if self.name.trim().is_empty() {
            return Err(PluginError::Invalid(format!(
                "MCP capability `{}` requires a name",
                capability.id
            )));
        }
        if !matches!(self.transport.as_str(), "stdio" | "http") {
            return Err(PluginError::Invalid(format!(
                "MCP capability `{}` has unsupported transport",
                capability.id
            )));
        }
        if self.transport == "stdio" && self.command.is_empty() {
            return Err(PluginError::Invalid(format!(
                "MCP capability `{}` requires a command",
                capability.id
            )));
        }
        if self.transport == "http"
            && !self
                .url
                .as_deref()
                .is_some_and(|url| url.starts_with("http://") || url.starts_with("https://"))
        {
            return Err(PluginError::Invalid(format!(
                "MCP capability `{}` requires an http(s) URL",
                capability.id
            )));
        }
        let command_allowed = self.command.first().is_some_and(|command| {
            manifest.permissions.iter().any(|permission| {
                permission.kind == "commands"
                    && permission.commands.iter().any(|allowed| allowed == command)
            })
        });
        if self.transport == "stdio" && !command_allowed {
            return Err(PluginError::Invalid(
                "plugin must declare the MCP command in a commands permission".to_string(),
            ));
        }
        if let Some(name) = self.credential_env.as_deref()
            && !valid_env_name(name)
        {
            return Err(PluginError::Invalid(format!(
                "MCP capability `{}` has an invalid credentialEnv",
                capability.id
            )));
        }
        Ok(())
    }
}

fn validate_manifest_structure(manifest: &PluginManifest) -> Result<(), PluginError> {
    let mut issues = Vec::new();
    if manifest.schema_version != PLUGIN_SCHEMA {
        issues.push(format!("schemaVersion must be `{PLUGIN_SCHEMA}`"));
    }
    if manifest.id.trim().is_empty()
        || !manifest.id.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        })
    {
        issues.push("id must contain only letters, digits, dot, dash or underscore".to_string());
    }
    if manifest.name.trim().is_empty() {
        issues.push("name is required".to_string());
    }
    if semver::Version::parse(&manifest.version).is_err() {
        issues.push("version must be semver".to_string());
    }
    if manifest.license.trim().is_empty() {
        issues.push("license is required".to_string());
    }
    if manifest.compatibility.hpa.trim().is_empty() {
        issues.push("compatibility.hpa is required".to_string());
    }
    let mut capability_ids = BTreeSet::new();
    for capability in &manifest.capabilities {
        if !capability_ids.insert((capability.kind.as_str(), capability.id.as_str())) {
            issues.push(format!(
                "duplicate capability `{}:{}`",
                capability.kind.as_str(),
                capability.id
            ));
        }
        if capability.id.trim().is_empty() || capability.version.trim().is_empty() {
            issues.push("capability id and version are required".to_string());
        }
        if semver::Version::parse(&capability.version).is_err() {
            issues.push(format!(
                "capability `{}` version must be semver",
                capability.id
            ));
        }
        if !valid_relative_path(&capability.path) {
            issues.push(format!(
                "capability `{}` path must be relative and cannot escape",
                capability.id
            ));
        }
    }
    for dependency in &manifest.dependencies {
        if dependency.kind != "plugin" || dependency.id.trim().is_empty() {
            issues.push("plugin dependencies require kind=plugin and id".to_string());
        }
        if VersionReq::parse(&dependency.range).is_err() {
            issues.push(format!(
                "dependency `{}` has an invalid range",
                dependency.id
            ));
        }
    }
    for permission in &manifest.permissions {
        if !matches!(
            permission.kind.as_str(),
            "network" | "filesystem" | "database" | "commands" | "credential"
        ) {
            issues.push(format!("unsupported permission `{}`", permission.kind));
        }
    }
    if issues.is_empty() {
        Ok(())
    } else {
        Err(PluginError::Invalid(issues.join("; ")))
    }
}

fn validate_capability_paths(
    files: &BTreeMap<String, Vec<u8>>,
    manifest: &PluginManifest,
) -> Result<(), PluginError> {
    for capability in &manifest.capabilities {
        match capability.kind {
            CapabilityKind::Skill => {
                let prefix = format!("{}/", capability.path.trim_end_matches('/'));
                if !files.contains_key(&format!("{prefix}SKILL.md")) {
                    return Err(PluginError::Invalid(format!(
                        "skill capability `{}` is missing SKILL.md",
                        capability.id
                    )));
                }
            }
            _ => {
                if !files.contains_key(&capability.path) {
                    return Err(PluginError::Invalid(format!(
                        "capability `{}` path `{}` is missing",
                        capability.id, capability.path
                    )));
                }
            }
        }
    }
    Ok(())
}

fn validate_capability_permissions(
    plugin: &PluginManifest,
    connector: &ConnectorManifest,
) -> Result<(), PluginError> {
    let needs_network = matches!(
        connector.kind,
        connector::ConnectorKind::Http
            | connector::ConnectorKind::Mq
            | connector::ConnectorKind::S3
    );
    let needs_filesystem = matches!(
        connector.kind,
        connector::ConnectorKind::File | connector::ConnectorKind::Git
    );
    let needs_database = connector.kind == connector::ConnectorKind::Database;
    if needs_network
        && !plugin
            .permissions
            .iter()
            .any(|permission| permission.kind == "network")
    {
        return Err(PluginError::Invalid(format!(
            "plugin must declare network permission for connector `{}`",
            connector.id
        )));
    }
    if needs_filesystem
        && !plugin
            .permissions
            .iter()
            .any(|permission| permission.kind == "filesystem")
    {
        return Err(PluginError::Invalid(format!(
            "plugin must declare filesystem permission for connector `{}`",
            connector.id
        )));
    }
    if needs_database
        && !plugin
            .permissions
            .iter()
            .any(|permission| permission.kind == "database")
    {
        return Err(PluginError::Invalid(format!(
            "plugin must declare database permission for connector `{}`",
            connector.id
        )));
    }
    if !connector.permissions.iter().any(|permission| {
        (needs_network && permission.kind == "network")
            || (needs_filesystem && permission.kind == "filesystem")
            || (needs_database && permission.kind == "database")
    }) {
        return Err(PluginError::Invalid(format!(
            "connector `{}` must declare its own matching permission",
            connector.id
        )));
    }
    Ok(())
}

fn advisory_affects(advisory: &OfflineAdvisory, plugin_id: &str, version: &Version) -> bool {
    advisory.affected.iter().any(|affected| {
        if !affected.package.ecosystem.eq_ignore_ascii_case("HPA")
            || affected.package.name != plugin_id
        {
            return false;
        }
        if affected.versions.iter().any(|candidate| {
            Version::parse(candidate)
                .map(|candidate| candidate == *version)
                .unwrap_or(false)
        }) {
            return true;
        }
        affected.ranges.iter().any(|range| {
            if range.kind != "SEMVER" {
                return false;
            }
            let mut affected_version = false;
            for event in &range.events {
                if let Some(introduced) = &event.introduced {
                    let introduced = if introduced == "0" {
                        Some(Version::new(0, 0, 0))
                    } else {
                        Version::parse(introduced).ok()
                    };
                    if introduced.is_some_and(|introduced| *version >= introduced) {
                        affected_version = true;
                    }
                }
                if let Some(fixed) = &event.fixed
                    && Version::parse(fixed).is_ok_and(|fixed| *version >= fixed)
                {
                    affected_version = false;
                }
            }
            affected_version
        })
    })
}

fn offline_advisory_value(advisory: &OfflineAdvisory) -> Value {
    json!({
        "id": advisory.id,
        "summary": advisory.summary.clone(),
        "modified": advisory.modified.clone(),
        "aliases": advisory.aliases.clone()
    })
}

fn osv_advisory_value(advisory: OsvAdvisory) -> Value {
    json!({
        "id": advisory.id,
        "summary": advisory.summary,
        "modified": advisory.modified,
        "aliases": advisory.aliases
    })
}

fn vulnerability_report_value(
    source: &str,
    plugin_id: &str,
    version: &str,
    vulnerabilities: Vec<Value>,
    database_generated_at: Option<String>,
    query_ecosystem: Option<&str>,
) -> Value {
    json!({
        "schemaVersion": "hpa.vulnerability-report/v1",
        "status": if vulnerabilities.is_empty() { "clean" } else { "vulnerable" },
        "source": source,
        "checkedAt": now_rfc3339(),
        "databaseGeneratedAt": database_generated_at,
        "package": {"ecosystem": "HPA", "name": plugin_id, "version": version},
        "query": {
            "ecosystem": query_ecosystem,
            "name": plugin_id,
            "version": version
        },
        "vulnerabilities": vulnerabilities
    })
}

async fn validate_osv_endpoint(raw_url: &str) -> Result<(), PluginError> {
    let url = reqwest::Url::parse(raw_url)
        .map_err(|error| PluginError::Invalid(format!("invalid OSV URL: {error}")))?;
    if !matches!(url.scheme(), "http" | "https") || url.username() != "" || url.password().is_some()
    {
        return Err(PluginError::Forbidden(
            "OSV URL must be HTTP(S) without userinfo".to_string(),
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| PluginError::Invalid("OSV URL has no host".to_string()))?;
    let loopback = matches!(host, "localhost" | "127.0.0.1" | "::1");
    if url.scheme() == "http" && !loopback {
        return Err(PluginError::Forbidden(
            "OSV must use HTTPS unless it is loopback".to_string(),
        ));
    }
    if !loopback {
        let port = url.port_or_known_default().unwrap_or(443);
        let mut resolved = false;
        for address in tokio::net::lookup_host((host, port))
            .await
            .map_err(|error| PluginError::Invalid(format!("cannot resolve OSV host: {error}")))?
        {
            resolved = true;
            let blocked = match address.ip() {
                std::net::IpAddr::V4(ip) => {
                    ip.is_loopback()
                        || ip.is_private()
                        || ip.is_link_local()
                        || ip.is_unspecified()
                        || ip.is_documentation()
                }
                std::net::IpAddr::V6(ip) => {
                    ip.is_loopback()
                        || ip.is_unique_local()
                        || ip.is_unicast_link_local()
                        || ip.is_unspecified()
                        || ip.is_multicast()
                }
            };
            if blocked {
                return Err(PluginError::Forbidden(
                    "OSV host resolves to a private or blocked address".to_string(),
                ));
            }
        }
        if !resolved {
            return Err(PluginError::Invalid(
                "OSV host did not resolve to an address".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_identifier(value: &str, label: &str) -> Result<(), PluginError> {
    if value.is_empty()
        || value.len() > 128
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
    {
        return Err(PluginError::Invalid(format!("{label} is invalid")));
    }
    Ok(())
}

fn verify_provenance_hashes(
    statement: &[u8],
    bundle: &[u8],
    statement_sha256: &str,
    bundle_sha256: &str,
) -> Result<(), PluginError> {
    if statement.len() > 4 * 1024 * 1024 || bundle.len() > 4 * 1024 * 1024 {
        return Err(PluginError::Invalid(
            "provenance statement or bundle exceeds 4 MiB".to_string(),
        ));
    }
    for (name, bytes, expected) in [
        ("statement", statement, statement_sha256),
        ("bundle", bundle, bundle_sha256),
    ] {
        if expected.len() != 64
            || !expected
                .chars()
                .all(|character| character.is_ascii_hexdigit())
            || format!("{:x}", Sha256::digest(bytes)) != expected.to_ascii_lowercase()
        {
            return Err(PluginError::Signature(format!(
                "provenance {name} hash does not match the signed index"
            )));
        }
    }
    Ok(())
}

fn validate_provenance_statement(
    statement: &[u8],
    package_sha256: &str,
    oci: Option<&RemotePackageOci>,
) -> Result<(), PluginError> {
    let value: Value = serde_json::from_slice(statement)?;
    if !matches!(
        value.get("_type").and_then(Value::as_str),
        Some("https://in-toto.io/Statement/v1") | Some("https://in-toto.io/Statement/v0.1")
    ) {
        return Err(PluginError::Signature(
            "provenance statement type is not supported".to_string(),
        ));
    }
    if !value
        .get("predicateType")
        .and_then(Value::as_str)
        .is_some_and(|value| {
            value == "https://slsa.dev/provenance/v1"
                || value == "https://slsa.dev/provenance/v0.2"
                || value == "https://hpa.dev/provenance/plugin/v1"
        })
    {
        return Err(PluginError::Signature(
            "provenance predicateType is not supported".to_string(),
        ));
    }
    let subjects = value
        .get("subject")
        .and_then(Value::as_array)
        .ok_or_else(|| PluginError::Signature("provenance has no subject".to_string()))?;
    let package_bound = subjects.iter().any(|subject| {
        subject
            .pointer("/digest/sha256")
            .and_then(Value::as_str)
            .is_some_and(|digest| digest.eq_ignore_ascii_case(package_sha256))
    });
    if !package_bound {
        return Err(PluginError::Signature(
            "provenance subject does not bind the package sha256".to_string(),
        ));
    }
    if let Some(oci) = oci {
        if !valid_oci_repository(&oci.repository) || !valid_sha256_digest(&oci.digest) {
            return Err(PluginError::Signature(
                "OCI provenance repository or digest is invalid".to_string(),
            ));
        }
        let oci_bound = subjects.iter().any(|subject| {
            subject
                .pointer("/digest/sha256")
                .and_then(Value::as_str)
                .is_some_and(|digest| {
                    oci.digest
                        .strip_prefix("sha256:")
                        .is_some_and(|expected| digest.eq_ignore_ascii_case(expected))
                })
        });
        if !oci_bound {
            return Err(PluginError::Signature(
                "provenance subject does not bind the OCI manifest digest".to_string(),
            ));
        }
    }
    Ok(())
}

fn valid_sha256_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .chars()
                .all(|character| character.is_ascii_hexdigit())
    })
}

fn valid_oci_repository(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.contains("://")
        && value.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '.' | '/' | '_' | '-')
        })
}

fn validate_remote_index_uri(uri: &str) -> Result<(), PluginError> {
    if uri.starts_with("http://") || uri.starts_with("https://") {
        let url = reqwest::Url::parse(uri)
            .map_err(|error| PluginError::Invalid(format!("invalid marketplace URL: {error}")))?;
        if !url.username().is_empty() || url.password().is_some() {
            return Err(PluginError::Forbidden(
                "marketplace URL must not contain userinfo".to_string(),
            ));
        }
        if url.scheme() == "http"
            && !url
                .host_str()
                .is_some_and(|host| matches!(host, "localhost" | "127.0.0.1" | "::1"))
        {
            return Err(PluginError::Forbidden(
                "remote marketplace must use HTTPS unless it is loopback".to_string(),
            ));
        }
        Ok(())
    } else if uri.trim().is_empty() || uri.contains('\0') {
        Err(PluginError::Invalid(
            "marketplace index URI is invalid".to_string(),
        ))
    } else {
        Ok(())
    }
}

async fn fetch_remote_bytes(base: &Path, reference: &str) -> Result<Vec<u8>, PluginError> {
    if base.as_os_str().is_empty() {
        if reference.starts_with("http://") || reference.starts_with("https://") {
            return fetch_http_bytes(reference, None).await;
        }
        let target = std::fs::canonicalize(reference).map_err(|error| {
            PluginError::NotFound(format!("marketplace index not found: {error}"))
        })?;
        if !target.is_file() {
            return Err(PluginError::Invalid(
                "marketplace index is not a file".to_string(),
            ));
        }
        let bytes = std::fs::read(target)?;
        if bytes.len() > 2 * 1024 * 1024 {
            return Err(PluginError::Invalid(
                "marketplace index exceeds 2 MiB".to_string(),
            ));
        }
        return Ok(bytes);
    }
    if let Some(base) = base.to_str()
        && (base.starts_with("http://") || base.starts_with("https://"))
    {
        let base_url = reqwest::Url::parse(base)
            .map_err(|error| PluginError::Invalid(format!("invalid marketplace URL: {error}")))?;
        let url = base_url
            .join(reference)
            .map_err(|error| PluginError::Invalid(format!("invalid package URL: {error}")))?;
        if url.scheme() != base_url.scheme()
            || url.host_str() != base_url.host_str()
            || url.port_or_known_default() != base_url.port_or_known_default()
        {
            return Err(PluginError::Forbidden(
                "marketplace package URL must stay on the index origin".to_string(),
            ));
        }
        return fetch_http_bytes(url.as_str(), Some(&base_url)).await;
    }
    let root = std::fs::canonicalize(base).map_err(|error| {
        PluginError::Invalid(format!("cannot open offline marketplace mirror: {error}"))
    })?;
    let path = Path::new(reference);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(PluginError::Forbidden(
            "marketplace package path escapes the index directory".to_string(),
        ));
    }
    let target = std::fs::canonicalize(root.join(path)).map_err(|error| {
        PluginError::NotFound(format!("marketplace package not found: {error}"))
    })?;
    if !target.starts_with(&root) || !target.is_file() {
        return Err(PluginError::Forbidden(
            "marketplace package path escapes the index directory".to_string(),
        ));
    }
    let bytes = std::fs::read(&target)?;
    if bytes.len() > MAX_PLUGIN_BYTES {
        return Err(PluginError::Invalid(
            "marketplace package exceeds 20 MiB".to_string(),
        ));
    }
    Ok(bytes)
}

async fn fetch_http_bytes(
    raw_url: &str,
    expected_origin: Option<&reqwest::Url>,
) -> Result<Vec<u8>, PluginError> {
    let url = reqwest::Url::parse(raw_url)
        .map_err(|error| PluginError::Invalid(format!("invalid marketplace URL: {error}")))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(PluginError::Forbidden(
            "marketplace URL must be HTTP(S) without userinfo".to_string(),
        ));
    }
    if url.scheme() == "http"
        && !url
            .host_str()
            .is_some_and(|host| matches!(host, "localhost" | "127.0.0.1" | "::1"))
    {
        return Err(PluginError::Forbidden(
            "remote marketplace must use HTTPS".to_string(),
        ));
    }
    if let Some(expected) = expected_origin
        && (url.scheme() != expected.scheme()
            || url.host_str() != expected.host_str()
            || url.port_or_known_default() != expected.port_or_known_default())
    {
        return Err(PluginError::Forbidden(
            "marketplace package URL changed origin".to_string(),
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| PluginError::Invalid("marketplace URL has no host".to_string()))?;
    let port = url.port_or_known_default().unwrap_or(443);
    if !matches!(host, "localhost" | "127.0.0.1" | "::1") {
        for address in tokio::net::lookup_host((host, port))
            .await
            .map_err(|error| {
                PluginError::Invalid(format!("cannot resolve marketplace host: {error}"))
            })?
        {
            let blocked = match address.ip() {
                std::net::IpAddr::V4(ip) => {
                    ip.is_loopback()
                        || ip.is_private()
                        || ip.is_link_local()
                        || ip.is_unspecified()
                        || ip.is_documentation()
                }
                std::net::IpAddr::V6(ip) => {
                    ip.is_unique_local()
                        || ip.is_unicast_link_local()
                        || ip.is_unspecified()
                        || ip.is_multicast()
                }
            };
            if blocked {
                return Err(PluginError::Forbidden(
                    "marketplace host resolves to a private or blocked address".to_string(),
                ));
            }
        }
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|error| PluginError::Invalid(error.to_string()))?;
    let response =
        client.get(url).send().await.map_err(|error| {
            PluginError::Invalid(format!("marketplace download failed: {error}"))
        })?;
    if !response.status().is_success() {
        return Err(PluginError::Invalid(format!(
            "marketplace download returned HTTP {}",
            response.status().as_u16()
        )));
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_PLUGIN_BYTES as u64)
    {
        return Err(PluginError::Invalid(
            "marketplace response exceeds 20 MiB".to_string(),
        ));
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream
        .next()
        .await
        .transpose()
        .map_err(|error| PluginError::Invalid(format!("marketplace read failed: {error}")))?
    {
        if bytes.len() + chunk.len() > MAX_PLUGIN_BYTES {
            return Err(PluginError::Invalid(
                "marketplace response exceeds 20 MiB".to_string(),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn remote_index_base(uri: &str) -> String {
    if uri.starts_with("http://") || uri.starts_with("https://") {
        return uri.to_string();
    }
    Path::new(uri)
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_string_lossy()
        .to_string()
}

fn time_window_active(valid_from: Option<&str>, valid_until: Option<&str>) -> bool {
    let now = OffsetDateTime::now_utc();
    let from_ok = valid_from
        .is_none_or(|value| OffsetDateTime::parse(value, &Rfc3339).is_ok_and(|value| value <= now));
    let until_ok = valid_until
        .is_none_or(|value| OffsetDateTime::parse(value, &Rfc3339).is_ok_and(|value| value >= now));
    from_ok && until_ok
}

fn verify_ed25519(public_key: &str, payload: &[u8], signature: &str) -> Result<(), PluginError> {
    let public = VerifyingKey::from_bytes(&decode_public_key(public_key)?)
        .map_err(|error| PluginError::Signature(format!("invalid Ed25519 public key: {error}")))?;
    let signature = BASE64
        .decode(signature)
        .map_err(|error| PluginError::Signature(format!("invalid Ed25519 signature: {error}")))?;
    let signature: [u8; 64] = signature
        .try_into()
        .map_err(|_| PluginError::Signature("Ed25519 signature must be 64 bytes".to_string()))?;
    public
        .verify(payload, &Ed25519Signature::from_bytes(&signature))
        .map_err(|_| PluginError::Signature("Ed25519 signature verification failed".to_string()))
}

fn canonical_manifest(manifest: &PluginManifest) -> Result<String, PluginError> {
    let mut unsigned = manifest.clone();
    unsigned.signature = None;
    Ok(serde_json::to_string(&unsigned)?)
}

fn signature_payload(manifest: &PluginManifest, digest: &str, canonical: &str) -> String {
    format!(
        "{PLUGIN_SCHEMA}\n{}@{}\n{}\n{}",
        manifest.id, manifest.version, digest, canonical
    )
}

fn verify_signature(
    signature: &PluginSignature,
    manifest: &PluginManifest,
    digest: &str,
    canonical: &str,
) -> Result<String, PluginError> {
    if signature.algorithm != "ed25519" {
        return Err(PluginError::Signature(
            "only ed25519 signatures are supported".to_string(),
        ));
    }
    let public_bytes = decode_public_key(&signature.public_key)?;
    let verifying = VerifyingKey::from_bytes(&public_bytes)
        .map_err(|error| PluginError::Signature(format!("invalid public key: {error}")))?;
    let value = BASE64
        .decode(&signature.value)
        .map_err(|error| PluginError::Signature(format!("invalid signature: {error}")))?;
    let value: [u8; 64] = value
        .try_into()
        .map_err(|_| PluginError::Signature("signature must be 64 bytes".to_string()))?;
    let signature_value = Ed25519Signature::from_bytes(&value);
    let payload = signature_payload(manifest, digest, canonical);
    verifying
        .verify(payload.as_bytes(), &signature_value)
        .map_err(|_| PluginError::Signature("signature verification failed".to_string()))?;
    Ok(signature.public_key.clone())
}

fn decode_public_key(encoded: &str) -> Result<[u8; 32], PluginError> {
    let bytes = BASE64
        .decode(encoded)
        .map_err(|error| PluginError::Signature(format!("invalid public key: {error}")))?;
    bytes
        .try_into()
        .map_err(|_| PluginError::Signature("public key must be 32 bytes".to_string()))
}

fn compute_content_hash(files: &BTreeMap<String, Vec<u8>>) -> Result<String, PluginError> {
    let mut hasher = Sha256::new();
    for (path, bytes) in files {
        if path == MANIFEST_FILE {
            continue;
        }
        let mut file_hasher = Sha256::new();
        file_hasher.update(bytes);
        let file_hash = format!("{:x}", file_hasher.finalize());
        hasher.update(path.as_bytes());
        hasher.update([0]);
        hasher.update(bytes.len().to_string().as_bytes());
        hasher.update([0]);
        hasher.update(file_hash.as_bytes());
        hasher.update([0]);
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn read_directory_files(root: &Path) -> Result<BTreeMap<String, Vec<u8>>, PluginError> {
    let mut files = BTreeMap::new();
    let mut total = 0usize;
    read_directory_inner(root, root, &mut files, &mut total)?;
    if !files.contains_key(MANIFEST_FILE) {
        return Err(PluginError::Invalid(format!("missing {MANIFEST_FILE}")));
    }
    Ok(files)
}

fn read_directory_inner(
    root: &Path,
    directory: &Path,
    files: &mut BTreeMap<String, Vec<u8>>,
    total: &mut usize,
) -> Result<(), PluginError> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name == ".DS_Store" {
            continue;
        }
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(PluginError::Invalid(format!(
                "symlinks are not allowed: {}",
                path.display()
            )));
        }
        if metadata.is_dir() {
            read_directory_inner(root, &path, files, total)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(PluginError::Invalid(format!(
                "unsupported file type: {}",
                path.display()
            )));
        }
        if metadata.len() as usize > MAX_PLUGIN_FILE_BYTES {
            return Err(PluginError::Invalid(format!(
                "file exceeds {MAX_PLUGIN_FILE_BYTES} bytes: {}",
                path.display()
            )));
        }
        if files.len() >= MAX_PLUGIN_FILES {
            return Err(PluginError::Invalid(format!(
                "plugin exceeds {MAX_PLUGIN_FILES} files"
            )));
        }
        *total += metadata.len() as usize;
        if *total > MAX_PLUGIN_BYTES {
            return Err(PluginError::Invalid(format!(
                "plugin exceeds {MAX_PLUGIN_BYTES} bytes"
            )));
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| PluginError::Invalid("cannot compute relative path".to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        if !valid_relative_path(&relative) {
            return Err(PluginError::Invalid(format!(
                "invalid package path `{relative}`"
            )));
        }
        files.insert(relative, std::fs::read(&path)?);
    }
    Ok(())
}

fn read_archive_files(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, PluginError> {
    let mut archive = Archive::new(Cursor::new(bytes));
    let mut files = BTreeMap::new();
    let mut total = 0usize;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_type = entry.header().entry_type();
        if entry_type.is_dir() {
            continue;
        }
        if !entry_type.is_file() {
            return Err(PluginError::Invalid(
                "plugin archives may only contain regular files".to_string(),
            ));
        }
        let path = entry
            .path()
            .map_err(|error| PluginError::Invalid(format!("invalid archive path: {error}")))?
            .to_string_lossy()
            .replace('\\', "/");
        if !valid_relative_path(&path) {
            return Err(PluginError::Invalid(format!(
                "invalid package path `{path}`"
            )));
        }
        if files.len() >= MAX_PLUGIN_FILES {
            return Err(PluginError::Invalid(format!(
                "plugin exceeds {MAX_PLUGIN_FILES} files"
            )));
        }
        let size = entry.header().size().unwrap_or(0) as usize;
        if size > MAX_PLUGIN_FILE_BYTES {
            return Err(PluginError::Invalid(format!(
                "file exceeds {MAX_PLUGIN_FILE_BYTES} bytes: {path}"
            )));
        }
        let mut data = Vec::with_capacity(size);
        entry.read_to_end(&mut data)?;
        total += data.len();
        if total > MAX_PLUGIN_BYTES {
            return Err(PluginError::Invalid(format!(
                "plugin exceeds {MAX_PLUGIN_BYTES} bytes"
            )));
        }
        files.insert(path, data);
    }
    if !files.contains_key(MANIFEST_FILE) {
        return Err(PluginError::Invalid(format!("missing {MANIFEST_FILE}")));
    }
    Ok(files)
}

fn pack_files(files: &BTreeMap<String, Vec<u8>>) -> Result<Vec<u8>, PluginError> {
    let mut builder = Builder::new(Vec::new());
    for (path, bytes) in files {
        let mut header = Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_cksum();
        builder.append_data(&mut header, path, bytes.as_slice())?;
    }
    builder.finish()?;
    Ok(builder.into_inner()?)
}

fn write_files(root: &Path, files: &BTreeMap<String, Vec<u8>>) -> Result<(), PluginError> {
    let temp = root.with_extension("tmp");
    if temp.exists() {
        std::fs::remove_dir_all(&temp)?;
    }
    std::fs::create_dir_all(&temp)?;
    for (path, bytes) in files {
        if !valid_relative_path(path) {
            return Err(PluginError::Invalid(format!(
                "invalid package path `{path}`"
            )));
        }
        let target = temp.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, bytes)?;
    }
    if root.exists() {
        std::fs::remove_dir_all(root)?;
    }
    if let Some(parent) = root.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(&temp, root)?;
    Ok(())
}

fn build_sbom(bundle: &PluginBundle, key: &LocalKey) -> Value {
    let mut files = Vec::new();
    let mut warnings = Vec::new();
    for (path, bytes) in &bundle.files {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        files.push(json!({
            "path": path,
            "size": bytes.len(),
            "sha256": format!("{:x}", hasher.finalize()),
        }));
        if let Ok(text) = std::str::from_utf8(bytes) {
            for pattern in [
                "child_process",
                "eval(",
                "curl | sh",
                "wget | sh",
                "rm -rf /",
                "-----begin private key-----",
            ] {
                if text.to_ascii_lowercase().contains(pattern) {
                    warnings.push(json!({"path": path, "pattern": pattern}));
                }
            }
        }
    }
    json!({
        "schemaVersion": "hpa.sbom/v1",
        "pluginId": bundle.manifest.id,
        "version": bundle.manifest.version,
        "license": bundle.manifest.license,
        "contentHash": bundle.content_hash,
        "trust": bundle.trust,
        "signerKeyId": bundle.manifest.signature.as_ref().map(|signature| signature.key_id.clone()),
        "publicKey": bundle.manifest.signature.as_ref().map(|signature| signature.public_key.clone()),
        "localKeyId": key.key_id,
        "files": files,
        "capabilities": &bundle.manifest.capabilities,
        "dependencies": &bundle.manifest.dependencies,
        "permissions": &bundle.manifest.permissions,
        "gates": {
            "license": hpa_skill::allowed_license(&bundle.manifest.license),
            "signature": bundle.manifest.signature.is_some(),
            "staticScan": if warnings.is_empty() { "pass" } else { "warnings" },
        },
        "warnings": warnings,
    })
}

fn valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn valid_relative_path(path: &str) -> bool {
    let candidate = Path::new(path);
    !path.is_empty()
        && !candidate.is_absolute()
        && candidate
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

fn plugin_server_id(plugin_id: &str, capability_id: &str) -> String {
    let value = format!("{plugin_id}_{capability_id}");
    format!(
        "plugin_{}",
        value
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                    character
                } else {
                    '_'
                }
            })
            .collect::<String>()
    )
}

fn plugin_row_value(row: &PluginRow) -> Value {
    json!({
        "id": row.id,
        "name": row.name,
        "latestVersion": row.latest_version,
        "status": row.status,
        "installedVersion": row.installed_version,
        "previousVersion": row.previous_version,
        "trust": row.trust,
        "license": row.license,
        "createdAt": row.created_at,
        "updatedAt": row.updated_at,
    })
}

fn package_value(row: &PluginPackageRow) -> Value {
    json!({
        "pluginId": row.plugin_id,
        "version": row.version,
        "name": row.name,
        "description": row.description,
        "license": row.license,
        "publisher": row.publisher,
        "contentHash": row.content_hash,
        "packagePath": row.package_path,
        "signature": row.signature_json.as_ref().and_then(|value| serde_json::from_str::<Value>(value).ok()),
        "signerKeyId": row.signer_key_id,
        "signerPublicKey": row.signer_public_key,
        "trust": row.trust,
        "source": row.source,
        "capabilities": serde_json::from_str::<Value>(&row.capabilities_json).unwrap_or(Value::Null),
        "createdAt": row.created_at,
    })
}

fn capability_row_value(row: &PluginCapabilityRow) -> Value {
    json!({
        "pluginId": row.plugin_id,
        "version": row.version,
        "kind": row.kind,
        "capabilityId": row.capability_id,
        "capabilityVersion": row.capability_version,
        "path": row.path,
        "status": row.status,
        "config": serde_json::from_str::<Value>(&row.config_json).unwrap_or(Value::Null),
        "permissions": serde_json::from_str::<Value>(&row.permissions_json).unwrap_or(Value::Null),
    })
}

fn review_value(row: &MarketplaceReviewRow) -> Value {
    json!({
        "id": row.id,
        "pluginId": row.plugin_id,
        "version": row.version,
        "rating": row.rating,
        "comment": row.comment,
        "createdAt": row.created_at,
    })
}

fn pg_plugin_error(error: PostgresPluginError) -> PluginError {
    match error {
        PostgresPluginError::Database(error) => PluginError::Sqlx(error),
        PostgresPluginError::Json(error) => PluginError::Json(error),
        PostgresPluginError::Invalid(message) => PluginError::Invalid(message),
    }
}

fn pg_mcp_error(error: PostgresMcpMetaError) -> PluginError {
    match error {
        PostgresMcpMetaError::Database(error) => PluginError::Sqlx(error),
        PostgresMcpMetaError::Json(error) => PluginError::Json(error),
        PostgresMcpMetaError::Invalid(message) => PluginError::Invalid(message),
    }
}

fn plugin_row_from_pg(record: PluginRecord) -> PluginRow {
    PluginRow {
        id: record.id,
        name: record.name,
        latest_version: record.latest_version,
        status: record.status,
        installed_version: record.installed_version,
        previous_version: record.previous_version,
        trust: record.trust,
        license: record.license,
        created_at: record.created_at,
        updated_at: record.updated_at,
    }
}

fn package_row_from_pg(record: PluginPackageRecord) -> PluginPackageRow {
    PluginPackageRow {
        plugin_id: record.plugin_id,
        version: record.version,
        name: record.name,
        description: record.description,
        license: record.license,
        publisher: record.publisher,
        manifest_json: record.manifest_json,
        content_hash: record.content_hash,
        package_path: record.package_path,
        signature_json: record.signature_json,
        signer_key_id: record.signer_key_id,
        signer_public_key: record.signer_public_key,
        trust: record.trust,
        source: record.source,
        capabilities_json: record.capabilities_json,
        sbom_json: record.sbom_json,
        created_at: record.created_at,
    }
}

fn capability_row_from_pg(record: PluginCapabilityRecord) -> PluginCapabilityRow {
    PluginCapabilityRow {
        plugin_id: record.plugin_id,
        version: record.version,
        kind: record.kind,
        capability_id: record.capability_id,
        capability_version: record.capability_version,
        path: record.path,
        status: record.status,
        config_json: record.config_json,
        permissions_json: record.permissions_json,
    }
}

fn listing_row_from_pg(record: MarketplaceListingRecord) -> MarketplaceListingRow {
    MarketplaceListingRow {
        plugin_id: record.plugin_id,
        version: record.version,
        review_status: record.review_status,
        review_note: record.review_note,
        submitted_at: record.submitted_at,
        reviewed_at: record.reviewed_at,
        withdrawn_at: record.withdrawn_at,
        downloads: record.downloads,
    }
}

fn review_row_from_pg(record: MarketplaceReviewRecord) -> MarketplaceReviewRow {
    MarketplaceReviewRow {
        id: record.id,
        plugin_id: record.plugin_id,
        version: record.version,
        rating: record.rating,
        comment: record.comment,
        created_at: record.created_at,
    }
}

fn trusted_key_row_from_pg(record: TrustedKeyRecord) -> TrustedKeyRow {
    TrustedKeyRow {
        key_id: record.key_id,
        public_key: record.public_key,
        label: record.label,
        trust: record.trust,
        created_at: record.created_at,
    }
}

fn event_row_from_pg(record: PluginEventRecord) -> PluginEventRow {
    PluginEventRow {
        id: record.id,
        at: record.at,
        plugin_id: record.plugin_id,
        version: record.version,
        kind: record.kind,
        detail_json: record.detail_json,
    }
}

fn remote_package_row_from_pg(record: RemotePackageRecord) -> RemoteMarketplacePackageRow {
    RemoteMarketplacePackageRow {
        source_id: record.source_id,
        plugin_id: record.plugin_id,
        version: record.version,
        publisher_id: record.publisher_id,
        signer_key_id: record.signer_key_id,
        package_sha256: record.package_sha256,
        package_path: record.package_path,
        review_status: record.review_status,
        index_sequence: record.index_sequence,
        published_at: record.published_at,
        withdrawn_at: record.withdrawn_at,
        revocation_reason: record.revocation_reason,
        provenance_json: record.provenance_json,
        oci_json: record.oci_json,
    }
}

fn source_key_value(key: &SourceKeyRecord) -> Value {
    json!({
        "keyId": key.key_id,
        "label": key.label,
        "validFrom": key.valid_from,
        "validUntil": key.valid_until,
    })
}

fn publisher_key_value(key: &PublisherKeyRecord) -> Value {
    json!({
        "keyId": key.key_id,
        "label": key.label,
        "validFrom": key.valid_from,
        "validUntil": key.valid_until,
        "revokedAt": key.revoked_at,
        "createdAt": key.created_at,
    })
}

fn registered_key_tuple(key: hpa_local::postgres_plugin::RegisteredKey) -> RegisteredKeyMaterial {
    (
        key.public_key,
        key.valid_from,
        key.valid_until,
        key.revoked_at,
    )
}

fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn manifest_paths_and_hashes_are_deterministic() {
        let mut files = BTreeMap::new();
        files.insert(
            MANIFEST_FILE.to_string(),
            br#"{"schemaVersion":"hpa.plugin/v1","id":"plugin_test","name":"Test","version":"1.0.0","license":"Apache-2.0","compatibility":{"hpa":">=0.1.0 <1.0.0"}}"#
                .to_vec(),
        );
        files.insert("skills/demo/SKILL.md".to_string(), b"hello".to_vec());
        let first = compute_content_hash(&files).expect("hash");
        let second = compute_content_hash(&files).expect("hash");
        assert_eq!(first, second);
        assert!(valid_relative_path("skills/demo/SKILL.md"));
        assert!(!valid_relative_path("../escape"));
        assert!(!valid_relative_path("/absolute"));
    }

    #[test]
    fn archive_roundtrip_preserves_files() {
        let mut files = BTreeMap::new();
        files.insert(MANIFEST_FILE.to_string(), b"{}".to_vec());
        files.insert("skills/demo/SKILL.md".to_string(), b"hello".to_vec());
        let archive = pack_files(&files).expect("pack");
        let loaded = read_archive_files(&archive).expect("read");
        assert_eq!(loaded, files);
    }

    #[test]
    fn mcp_server_ids_are_stable() {
        assert_eq!(plugin_server_id("plugin_a", "echo"), "plugin_plugin_a_echo");
        assert_eq!(plugin_server_id("plugin.a", "echo"), "plugin_plugin_a_echo");
    }

    #[tokio::test]
    async fn offline_vulnerability_database_blocks_install() {
        let data_dir = std::env::temp_dir().join(format!("hpa-vuln-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&data_dir).expect("data dir");
        let db = Db::open(&data_dir.join("hpa.db")).await.expect("db");
        let database_path = data_dir.join("vulnerabilities.json");
        std::fs::write(
            &database_path,
            serde_json::to_vec(&json!({
                "schemaVersion": "hpa.vulnerability-db/v1",
                "generatedAt": now_rfc3339(),
                "records": [{
                    "id": "HPA-TEST-2026-0001",
                    "summary": "Controlled vulnerable plugin range",
                    "modified": "2026-11-02T00:00:00Z",
                    "affected": [{
                        "package": {"ecosystem": "HPA", "name": "plugin_test"},
                        "ranges": [{
                            "type": "SEMVER",
                            "events": [{"introduced": "1.0.0"}, {"fixed": "1.1.0"}]
                        }]
                    }]
                }]
            }))
            .expect("vulnerability db"),
        )
        .expect("write vulnerability db");
        let service = PluginService::new(
            db.clone(),
            data_dir.clone(),
            SkillService::new(db.clone(), data_dir.clone()),
            ConnectorRuntime::new(db.clone()),
        )
        .with_vulnerability_source(VulnerabilitySource::Offline {
            path: database_path,
            max_age_hours: 24,
        });
        let report = service
            .vulnerability_report("plugin_test", "1.0.0")
            .await
            .expect("vulnerability report");
        assert_eq!(report["status"], "vulnerable");
        assert_eq!(report["vulnerabilities"][0]["id"], "HPA-TEST-2026-0001");

        let files = BTreeMap::from([(
            MANIFEST_FILE.to_string(),
            br#"{"schemaVersion":"hpa.plugin/v1","id":"plugin_test","name":"Test","version":"1.0.0","license":"Apache-2.0","compatibility":{"hpa":">=0.1.0 <1.0.0"}}"#
                .to_vec(),
        )]);
        let manifest: PluginManifest =
            serde_json::from_slice(&files[MANIFEST_FILE]).expect("manifest");
        let bundle = PluginBundle {
            manifest,
            content_hash: compute_content_hash(&files).expect("hash"),
            files,
            trust: "verified".to_string(),
        };
        let error = service
            .install_bundle(bundle, "test")
            .await
            .expect_err("vulnerable install must fail");
        assert!(error.to_string().contains("HPA-TEST-2026-0001"), "{error}");

        std::fs::write(
            service.data_dir.join("vulnerabilities.json"),
            serde_json::to_vec(&json!({
                "schemaVersion": "hpa.vulnerability-db/v1",
                "generatedAt": "2020-01-01T00:00:00Z",
                "records": []
            }))
            .expect("stale vulnerability db"),
        )
        .expect("write stale vulnerability db");
        let stale = service
            .vulnerability_report("plugin_test", "1.0.0")
            .await
            .expect_err("stale offline database must fail");
        assert!(matches!(stale, PluginError::Forbidden(_)), "{stale}");
    }

    #[tokio::test]
    async fn osv_query_returns_vulnerabilities() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("OSV listener");
        let address = listener.local_addr().expect("OSV address");
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("OSV connection");
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request);
            let body = json!({
                "vulns": [{
                    "id": "HPA-OSV-2026-0001",
                    "summary": "Controlled online advisory",
                    "modified": "2026-11-02T00:00:00Z"
                }]
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).expect("OSV response");
        });

        let data_dir = std::env::temp_dir().join(format!("hpa-osv-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&data_dir).expect("data dir");
        let db = Db::open(&data_dir.join("hpa.db")).await.expect("db");
        let service = PluginService::new(
            db.clone(),
            data_dir.clone(),
            SkillService::new(db.clone(), data_dir.clone()),
            ConnectorRuntime::new(db.clone()),
        )
        .with_vulnerability_source(VulnerabilitySource::Osv {
            endpoint: format!("http://{address}/v1/query"),
            ecosystem: None,
        });
        let report = service
            .vulnerability_report("plugin_online", "1.0.0")
            .await
            .expect("OSV report");
        assert_eq!(report["status"], "vulnerable");
        assert_eq!(report["source"], "osv");
        assert_eq!(report["vulnerabilities"][0]["id"], "HPA-OSV-2026-0001");
    }

    #[tokio::test]
    async fn cosign_provenance_verifies_statement_and_oci_digest() {
        let Ok(cosign) = std::env::var("HPA_COSIGN_BIN") else {
            eprintln!("skipped: HPA_COSIGN_BIN is not set");
            return;
        };
        let directory = std::env::temp_dir().join(format!("hpa-cosign-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&directory).expect("cosign dir");
        let key_prefix = directory.join("cosign");
        let generated = Command::new(&cosign)
            .args(["generate-key-pair", "--output-key-prefix"])
            .arg(&key_prefix)
            .env("COSIGN_PASSWORD", "test-password")
            .output()
            .expect("generate cosign key");
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let public_key = key_prefix.with_extension("pub");
        let private_key = key_prefix.with_extension("key");
        let statement_path = directory.join("statement.json");
        let bundle_path = directory.join("statement.bundle.json");
        let package_digest = format!("{:x}", Sha256::digest(b"package"));
        let oci_digest = format!("sha256:{:x}", Sha256::digest(b"oci-manifest"));
        let statement_bytes = serde_json::to_vec(&json!({
            "_type": "https://in-toto.io/Statement/v1",
            "subject": [
                {"name": "plugin_test", "digest": {"sha256": package_digest.clone()}},
                {"name": "registry.example/hpa/plugin_test", "digest": {"sha256": oci_digest.trim_start_matches("sha256:").to_string()}}
            ],
            "predicateType": "https://hpa.dev/provenance/plugin/v1",
            "predicate": {"builder": {"id": "https://builder.example/hpa"}}
        }))
        .expect("statement");
        std::fs::write(&statement_path, &statement_bytes).expect("write statement");
        let signed = Command::new(&cosign)
            .args([
                "sign-blob",
                "--yes",
                "--tlog-upload=false",
                "--key",
                private_key.to_str().expect("private key"),
                "--bundle",
                bundle_path.to_str().expect("bundle"),
                statement_path.to_str().expect("statement"),
            ])
            .env("COSIGN_PASSWORD", "test-password")
            .output()
            .expect("sign statement");
        assert!(
            signed.status.success(),
            "{}",
            String::from_utf8_lossy(&signed.stderr)
        );
        let bundle = std::fs::read(&bundle_path).expect("bundle bytes");
        validate_provenance_statement(
            &statement_bytes,
            &package_digest,
            Some(&RemotePackageOci {
                repository: "registry.example/hpa/plugin_test".to_string(),
                digest: oci_digest,
            }),
        )
        .expect("valid statement");
        let data_dir = directory.join("data");
        std::fs::create_dir_all(&data_dir).expect("data dir");
        let db = Db::open(&data_dir.join("hpa.db")).await.expect("db");
        let service = PluginService::new(
            db.clone(),
            data_dir.clone(),
            SkillService::new(db.clone(), data_dir.clone()),
            ConnectorRuntime::new(db.clone()),
        )
        .with_provenance_config(ProvenanceConfig {
            required: true,
            verifier: ProvenanceVerifier::Cosign {
                binary: PathBuf::from(cosign),
                trust: CosignTrust::Key(public_key),
            },
        });
        service
            .verify_cosign_files(&statement_bytes, &bundle)
            .await
            .expect("cosign verifies");
        let missing = service
            .verify_remote_provenance(
                &RemoteMarketplacePackage {
                    plugin_id: "plugin_test".to_string(),
                    version: "1.0.0".to_string(),
                    path: "package.hplugin".to_string(),
                    sha256: package_digest,
                    publisher_id: "publisher_test".to_string(),
                    signer_key_id: "key_test".to_string(),
                    review_status: "approved".to_string(),
                    published_at: None,
                    provenance: None,
                    oci: None,
                },
                "",
                &directory.join("cache.hplugin"),
            )
            .await
            .expect_err("required provenance must be present");
        assert!(matches!(missing, PluginError::Forbidden(_)), "{missing}");
        let mut tampered = statement_bytes;
        tampered.push(b' ');
        let error = service
            .verify_cosign_files(&tampered, &bundle)
            .await
            .expect_err("tampered statement must fail");
        assert!(matches!(error, PluginError::Signature(_)), "{error}");
    }
}
