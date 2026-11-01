use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read};
use std::path::{Component, Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{Signature as Ed25519Signature, Signer, SigningKey, Verifier, VerifyingKey};
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use tar::{Archive, Builder, Header};
use thiserror::Error;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::agent;
use crate::connector::{self, ConnectorManifest, ConnectorRuntime};
use crate::db::{AgentVersionRow, Db, DbError};
use crate::skills::{SkillService, SkillServiceError};

pub const PLUGIN_SCHEMA: &str = "hpa.plugin/v1";
pub const MANIFEST_FILE: &str = "hpa.plugin.json";
pub const MAX_PLUGIN_FILES: usize = 500;
pub const MAX_PLUGIN_BYTES: usize = 20 * 1024 * 1024;
pub const MAX_PLUGIN_FILE_BYTES: usize = 5 * 1024 * 1024;

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
}

impl PluginService {
    pub fn new(
        db: Db,
        data_dir: PathBuf,
        skills: SkillService,
        connectors: ConnectorRuntime,
    ) -> Self {
        Self {
            db,
            data_dir,
            skills,
            connectors,
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
        let rows = sqlx::query_as::<_, TrustedKeyRow>(
            "SELECT key_id, public_key, label, trust, created_at
             FROM plugin_trusted_keys ORDER BY created_at ASC",
        )
        .fetch_all(&self.db.pool)
        .await?;
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
                let trusted: Option<String> = sqlx::query_scalar(
                    "SELECT trust FROM plugin_trusted_keys WHERE key_id = ? AND public_key = ?",
                )
                .bind(&signature.key_id)
                .bind(&public_key)
                .fetch_optional(&self.db.pool)
                .await?;
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
            let row: Option<(String, String)> =
                sqlx::query_as("SELECT installed_version, status FROM plugins WHERE id = ?")
                    .bind(&dependency.id)
                    .fetch_optional(&self.db.pool)
                    .await?;
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
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT content_hash FROM plugin_packages WHERE plugin_id = ? AND version = ?",
        )
        .bind(&manifest.id)
        .bind(&manifest.version)
        .fetch_optional(&self.db.pool)
        .await?;
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
        self.package(&manifest.id, &manifest.version).await
    }

    async fn install_bundle(
        &self,
        bundle: PluginBundle,
        source: &str,
    ) -> Result<Value, PluginError> {
        let manifest = &bundle.manifest;
        let plugin: Option<PluginRow> = sqlx::query_as(
            "SELECT id, name, latest_version, status, installed_version, previous_version,
                    trust, license, created_at, updated_at
             FROM plugins WHERE id = ?",
        )
        .bind(&manifest.id)
        .fetch_optional(&self.db.pool)
        .await?;
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
        self.activate_version(&manifest.id, &manifest.version, previous.as_deref())
            .await?;
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
        sqlx::query(
            "UPDATE connector_definitions SET status = 'disabled'
             WHERE plugin_id = ? AND plugin_version = ?",
        )
        .bind(plugin_id)
        .bind(version)
        .execute(&self.db.pool)
        .await?;
        sqlx::query(
            "UPDATE mcp_servers SET enabled = 0 WHERE plugin_id = ? AND plugin_version = ?",
        )
        .bind(plugin_id)
        .bind(version)
        .execute(&self.db.pool)
        .await?;
        sqlx::query(
            "UPDATE plugin_capabilities SET status = 'disabled'
             WHERE plugin_id = ? AND version = ?",
        )
        .bind(plugin_id)
        .bind(version)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    async fn activate_existing(&self, plugin_id: &str, version: &str) -> Result<(), PluginError> {
        self.activate_version(plugin_id, version, None).await?;
        sqlx::query(
            "UPDATE mcp_servers SET enabled = 1 WHERE plugin_id = ? AND plugin_version = ?",
        )
        .bind(plugin_id)
        .bind(version)
        .execute(&self.db.pool)
        .await?;
        sqlx::query(
            "UPDATE plugin_capabilities SET status = 'enabled'
             WHERE plugin_id = ? AND version = ?",
        )
        .bind(plugin_id)
        .bind(version)
        .execute(&self.db.pool)
        .await?;
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
        let rows = sqlx::query_as::<_, PluginRow>(
            "SELECT id, name, latest_version, status, installed_version, previous_version,
                    trust, license, created_at, updated_at
             FROM plugins ORDER BY updated_at DESC",
        )
        .fetch_all(&self.db.pool)
        .await?;
        Ok(rows.iter().map(plugin_row_value).collect())
    }

    pub async fn detail(&self, id: &str) -> Result<Value, PluginError> {
        let plugin = sqlx::query_as::<_, PluginRow>(
            "SELECT id, name, latest_version, status, installed_version, previous_version,
                    trust, license, created_at, updated_at
             FROM plugins WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.db.pool)
        .await?
        .ok_or_else(|| PluginError::NotFound(format!("plugin `{id}` not found")))?;
        let packages = sqlx::query_as::<_, PluginPackageRow>(
            "SELECT plugin_id, version, name, description, license, publisher, manifest_json,
                    content_hash, package_path, signature_json, signer_key_id, signer_public_key,
                    trust, source, capabilities_json, sbom_json, created_at
             FROM plugin_packages WHERE plugin_id = ? ORDER BY created_at DESC",
        )
        .bind(id)
        .fetch_all(&self.db.pool)
        .await?;
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
        let rows = sqlx::query_as::<_, PluginCapabilityRow>(
            "SELECT plugin_id, version, kind, capability_id, capability_version, path, status,
                    config_json, permissions_json
             FROM plugin_capabilities WHERE plugin_id = ? AND version = ?
             ORDER BY kind ASC, capability_id ASC",
        )
        .bind(plugin_id)
        .bind(version)
        .fetch_all(&self.db.pool)
        .await?;
        Ok(rows.iter().map(capability_row_value).collect())
    }

    pub async fn enable(&self, id: &str) -> Result<Value, PluginError> {
        let plugin = self.plugin_row(id).await?;
        let version = plugin
            .installed_version
            .ok_or_else(|| PluginError::Conflict("plugin has no installed version".to_string()))?;
        self.activate_existing(id, &version).await?;
        sqlx::query("UPDATE plugins SET status = 'enabled', updated_at = ? WHERE id = ?")
            .bind(now_rfc3339())
            .bind(id)
            .execute(&self.db.pool)
            .await?;
        self.event(Some(id), Some(&version), "plugin.enabled", json!({}))
            .await?;
        self.detail(id).await
    }

    pub async fn disable(&self, id: &str) -> Result<Value, PluginError> {
        let plugin = self.plugin_row(id).await?;
        if let Some(version) = &plugin.installed_version {
            self.deactivate_version(id, version).await?;
        }
        sqlx::query("UPDATE plugins SET status = 'disabled', updated_at = ? WHERE id = ?")
            .bind(now_rfc3339())
            .bind(id)
            .execute(&self.db.pool)
            .await?;
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
        sqlx::query("DELETE FROM connector_definitions WHERE plugin_id = ?")
            .bind(id)
            .execute(&self.db.pool)
            .await?;
        sqlx::query("DELETE FROM mcp_servers WHERE plugin_id = ?")
            .bind(id)
            .execute(&self.db.pool)
            .await?;
        sqlx::query("UPDATE plugin_capabilities SET status = 'uninstalled' WHERE plugin_id = ?")
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
        let packages = sqlx::query_as::<_, PluginPackageRow>(
            "SELECT plugin_id, version, name, description, license, publisher, manifest_json,
                    content_hash, package_path, signature_json, signer_key_id, signer_public_key,
                    trust, source, capabilities_json, sbom_json, created_at
             FROM plugin_packages p WHERE p.version = (
                SELECT installed_version FROM plugins WHERE id = p.plugin_id AND status != 'uninstalled'
             )",
        )
        .fetch_all(&self.db.pool)
        .await?;
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
        let updated = sqlx::query(
            "UPDATE plugin_market_listings SET review_status = ?, review_note = ?, reviewed_at = ?
             WHERE plugin_id = ? AND version = ? AND review_status = 'pending'",
        )
        .bind(status)
        .bind(note)
        .bind(now_rfc3339())
        .bind(plugin_id)
        .bind(version)
        .execute(&self.db.pool)
        .await?;
        if updated.rows_affected() == 0 {
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
        let updated = sqlx::query(
            "UPDATE plugin_market_listings SET review_status = 'withdrawn', withdrawn_at = ?
             WHERE plugin_id = ? AND version = ? AND review_status != 'withdrawn'",
        )
        .bind(now_rfc3339())
        .bind(plugin_id)
        .bind(version)
        .execute(&self.db.pool)
        .await?;
        if updated.rows_affected() == 0 {
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
        let listings = sqlx::query_as::<_, MarketplaceListingRow>(
            "SELECT plugin_id, version, review_status, review_note, submitted_at, reviewed_at,
                    withdrawn_at, downloads
             FROM plugin_market_listings ORDER BY submitted_at DESC",
        )
        .fetch_all(&self.db.pool)
        .await?;
        let mut values = Vec::new();
        for listing in listings {
            let package = sqlx::query_as::<_, PluginPackageRow>(
                "SELECT plugin_id, version, name, description, license, publisher, manifest_json,
                        content_hash, package_path, signature_json, signer_key_id, signer_public_key,
                        trust, source, capabilities_json, sbom_json, created_at
                 FROM plugin_packages WHERE plugin_id = ? AND version = ?",
            )
            .bind(&listing.plugin_id)
            .bind(&listing.version)
            .fetch_optional(&self.db.pool)
            .await?;
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
        let listing = sqlx::query_as::<_, MarketplaceListingRow>(
            "SELECT plugin_id, version, review_status, review_note, submitted_at, reviewed_at,
                    withdrawn_at, downloads
             FROM plugin_market_listings WHERE plugin_id = ? AND version = ?",
        )
        .bind(plugin_id)
        .bind(version)
        .fetch_optional(&self.db.pool)
        .await?
        .ok_or_else(|| PluginError::NotFound("marketplace listing not found".to_string()))?;
        let package = self.package(plugin_id, version).await?;
        let manifest: Value = serde_json::from_str(&package.manifest_json)?;
        let reviews = sqlx::query_as::<_, MarketplaceReviewRow>(
            "SELECT id, plugin_id, version, rating, comment, created_at
             FROM plugin_reviews WHERE plugin_id = ? AND version = ? ORDER BY created_at DESC",
        )
        .bind(plugin_id)
        .bind(version)
        .fetch_all(&self.db.pool)
        .await?;
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
        let exists: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM plugin_market_listings WHERE plugin_id = ? AND version = ?",
        )
        .bind(plugin_id)
        .bind(version)
        .fetch_optional(&self.db.pool)
        .await?;
        if exists.is_none() {
            return Err(PluginError::NotFound(
                "marketplace listing not found".to_string(),
            ));
        }
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
        let row: (Option<f64>, i64) = sqlx::query_as(
            "SELECT AVG(rating), COUNT(*) FROM plugin_reviews WHERE plugin_id = ? AND version = ?",
        )
        .bind(plugin_id)
        .bind(version)
        .fetch_one(&self.db.pool)
        .await?;
        Ok(json!({"average": row.0, "count": row.1}))
    }

    pub async fn install_marketplace(
        &self,
        plugin_id: &str,
        version: &str,
    ) -> Result<Value, PluginError> {
        let listing = sqlx::query_as::<_, MarketplaceListingRow>(
            "SELECT plugin_id, version, review_status, review_note, submitted_at, reviewed_at,
                    withdrawn_at, downloads
             FROM plugin_market_listings WHERE plugin_id = ? AND version = ?",
        )
        .bind(plugin_id)
        .bind(version)
        .fetch_optional(&self.db.pool)
        .await?
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
        sqlx::query(
            "UPDATE plugin_market_listings SET downloads = downloads + 1
             WHERE plugin_id = ? AND version = ?",
        )
        .bind(plugin_id)
        .bind(version)
        .execute(&self.db.pool)
        .await?;
        Ok(detail)
    }

    pub async fn events(&self, plugin_id: &str, limit: i64) -> Result<Vec<Value>, PluginError> {
        let rows = sqlx::query_as::<_, PluginEventRow>(
            "SELECT id, at, plugin_id, version, kind, detail_json FROM plugin_events
             WHERE plugin_id = ? ORDER BY at DESC LIMIT ?",
        )
        .bind(plugin_id)
        .bind(limit.clamp(1, 200))
        .fetch_all(&self.db.pool)
        .await?;
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
        sqlx::query(
            "INSERT INTO plugin_events (id, at, plugin_id, version, kind, detail_json)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(format!("plevent_{}", Uuid::now_v7()))
        .bind(now_rfc3339())
        .bind(plugin_id)
        .bind(version)
        .bind(kind)
        .bind(serde_json::to_string(&detail)?)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    async fn plugin_row(&self, id: &str) -> Result<PluginRow, PluginError> {
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
            "network" | "filesystem" | "commands" | "credential"
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
    let needs_network = connector.kind == connector::ConnectorKind::Http;
    let needs_filesystem = connector.kind == connector::ConnectorKind::File;
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
    if !connector.permissions.iter().any(|permission| {
        (needs_network && permission.kind == "network")
            || (needs_filesystem && permission.kind == "filesystem")
    }) {
        return Err(PluginError::Invalid(format!(
            "connector `{}` must declare its own matching permission",
            connector.id
        )));
    }
    Ok(())
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

fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
