use serde_json::Value;
use sqlx::Row;
use sqlx::postgres::{PgPool, PgRow};
use sqlx::types::Json;
use thiserror::Error;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

#[derive(Debug, Error)]
pub enum PostgresPluginError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid plugin metadata: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone)]
pub struct TrustedKeyRecord {
    pub key_id: String,
    pub public_key: String,
    pub label: String,
    pub trust: String,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct PluginPackageRecord {
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

#[derive(Debug, Clone)]
pub struct PluginRecord {
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

#[derive(Debug, Clone)]
pub struct PluginCapabilityRecord {
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

#[derive(Debug, Clone)]
pub struct MarketplaceListingRecord {
    pub plugin_id: String,
    pub version: String,
    pub review_status: String,
    pub review_note: Option<String>,
    pub submitted_at: String,
    pub reviewed_at: Option<String>,
    pub withdrawn_at: Option<String>,
    pub downloads: i64,
}

#[derive(Debug, Clone)]
pub struct MarketplaceReviewRecord {
    pub id: String,
    pub plugin_id: String,
    pub version: String,
    pub rating: i64,
    pub comment: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct PluginEventRecord {
    pub id: String,
    pub at: String,
    pub plugin_id: Option<String>,
    pub version: Option<String>,
    pub kind: String,
    pub detail_json: String,
}

#[derive(Debug, Clone)]
pub struct MarketplaceSourceRecord {
    pub id: String,
    pub label: String,
    pub index_uri: String,
    pub enabled: bool,
    pub last_sequence: i64,
    pub last_refresh_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct SourceKeyRecord {
    pub source_id: String,
    pub key_id: String,
    pub public_key: String,
    pub label: String,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
    pub revoked_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct PublisherRecord {
    pub id: String,
    pub name: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct PublisherKeyRecord {
    pub publisher_id: String,
    pub key_id: String,
    pub public_key: String,
    pub label: String,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
    pub revoked_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct RegisteredKey {
    pub public_key: String,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
    pub revoked_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RemotePackageRecord {
    pub source_id: String,
    pub plugin_id: String,
    pub version: String,
    pub publisher_id: String,
    pub signer_key_id: String,
    pub package_sha256: String,
    pub package_path: String,
    pub review_status: String,
    pub index_sequence: i64,
    pub published_at: Option<String>,
    pub withdrawn_at: Option<String>,
    pub revocation_reason: Option<String>,
    pub provenance_json: Option<String>,
    pub oci_json: Option<String>,
}

#[derive(Clone)]
pub struct PostgresPluginStore {
    pool: PgPool,
    tenant_id: String,
}

impl PostgresPluginStore {
    pub fn new(pool: PgPool, tenant_id: impl Into<String>) -> Self {
        Self {
            pool,
            tenant_id: tenant_id.into(),
        }
    }

    async fn begin(&self) -> Result<sqlx::Transaction<'_, sqlx::Postgres>, PostgresPluginError> {
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

    pub async fn upsert_trusted_key(
        &self,
        key_id: &str,
        public_key: &str,
        label: &str,
        trust: &str,
        created_at: &str,
    ) -> Result<(), PostgresPluginError> {
        let mut transaction = self.begin().await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.plugin_trusted_keys
             (tenant_id, key_id, public_key, label, trust, created_at)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (tenant_id, key_id) DO UPDATE SET
                public_key = EXCLUDED.public_key,
                label = EXCLUDED.label,
                trust = EXCLUDED.trust",
        )
        .bind(&self.tenant_id)
        .bind(key_id)
        .bind(public_key)
        .bind(label)
        .bind(trust)
        .bind(parse_time(created_at)?)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn list_trusted_keys(&self) -> Result<Vec<TrustedKeyRecord>, PostgresPluginError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT key_id, public_key, label, trust, created_at
             FROM hpa_tenant.plugin_trusted_keys
             WHERE tenant_id = $1 ORDER BY created_at ASC",
        )
        .bind(&self.tenant_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.iter().map(trusted_key_from_row).collect()
    }

    pub async fn trusted_key_trust(
        &self,
        key_id: &str,
        public_key: &str,
    ) -> Result<Option<String>, PostgresPluginError> {
        let mut transaction = self.begin().await?;
        let trust: Option<String> = sqlx::query_scalar(
            "SELECT trust FROM hpa_tenant.plugin_trusted_keys
             WHERE tenant_id = $1 AND key_id = $2 AND public_key = $3",
        )
        .bind(&self.tenant_id)
        .bind(key_id)
        .bind(public_key)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(trust)
    }

    pub async fn package_content_hash(
        &self,
        plugin_id: &str,
        version: &str,
    ) -> Result<Option<String>, PostgresPluginError> {
        let mut transaction = self.begin().await?;
        let hash: Option<String> = sqlx::query_scalar(
            "SELECT content_hash FROM hpa_tenant.plugin_packages
             WHERE tenant_id = $1 AND plugin_id = $2 AND version = $3",
        )
        .bind(&self.tenant_id)
        .bind(plugin_id)
        .bind(version)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(hash)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_package(
        &self,
        plugin_id: &str,
        version: &str,
        name: &str,
        description: Option<&str>,
        license: &str,
        publisher: Option<&str>,
        manifest_json: &str,
        content_hash: &str,
        package_path: &str,
        signature_json: Option<&str>,
        signer_key_id: Option<&str>,
        signer_public_key: Option<&str>,
        trust: &str,
        source: &str,
        capabilities_json: &str,
        sbom_json: &str,
        created_at: &str,
    ) -> Result<(), PostgresPluginError> {
        let manifest: Value = serde_json::from_str(manifest_json)?;
        let signature: Option<Value> = signature_json.map(serde_json::from_str).transpose()?;
        let capabilities: Value = serde_json::from_str(capabilities_json)?;
        let sbom: Value = serde_json::from_str(sbom_json)?;
        let publisher = publisher.map(|value| Json(Value::String(value.to_string())));
        let mut transaction = self.begin().await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.plugin_packages
             (tenant_id, plugin_id, version, name, description, license, publisher,
              manifest_json, content_hash, package_path, signature_json, signer_key_id,
              signer_public_key, trust, source, capabilities_json, sbom_json, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)
             ON CONFLICT (tenant_id, plugin_id, version) DO UPDATE SET
                name = EXCLUDED.name,
                description = EXCLUDED.description,
                license = EXCLUDED.license,
                publisher = EXCLUDED.publisher,
                manifest_json = EXCLUDED.manifest_json,
                content_hash = EXCLUDED.content_hash,
                package_path = EXCLUDED.package_path,
                signature_json = EXCLUDED.signature_json,
                signer_key_id = EXCLUDED.signer_key_id,
                signer_public_key = EXCLUDED.signer_public_key,
                trust = EXCLUDED.trust,
                source = EXCLUDED.source,
                capabilities_json = EXCLUDED.capabilities_json,
                sbom_json = EXCLUDED.sbom_json",
        )
        .bind(&self.tenant_id)
        .bind(plugin_id)
        .bind(version)
        .bind(name)
        .bind(description)
        .bind(license)
        .bind(publisher)
        .bind(Json(&manifest))
        .bind(content_hash)
        .bind(package_path)
        .bind(signature.map(Json))
        .bind(signer_key_id)
        .bind(signer_public_key)
        .bind(trust)
        .bind(source)
        .bind(Json(&capabilities))
        .bind(Json(&sbom))
        .bind(parse_time(created_at)?)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn package(
        &self,
        plugin_id: &str,
        version: &str,
    ) -> Result<Option<PluginPackageRecord>, PostgresPluginError> {
        let mut transaction = self.begin().await?;
        let row = sqlx::query(
            "SELECT plugin_id, version, name, description, license, publisher, manifest_json,
                    content_hash, package_path, signature_json, signer_key_id, signer_public_key,
                    trust, source, capabilities_json, sbom_json, created_at
             FROM hpa_tenant.plugin_packages
             WHERE tenant_id = $1 AND plugin_id = $2 AND version = $3",
        )
        .bind(&self.tenant_id)
        .bind(plugin_id)
        .bind(version)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.as_ref().map(package_from_row).transpose()
    }

    pub async fn list_packages(
        &self,
        plugin_id: &str,
    ) -> Result<Vec<PluginPackageRecord>, PostgresPluginError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT plugin_id, version, name, description, license, publisher, manifest_json,
                    content_hash, package_path, signature_json, signer_key_id, signer_public_key,
                    trust, source, capabilities_json, sbom_json, created_at
             FROM hpa_tenant.plugin_packages
             WHERE tenant_id = $1 AND plugin_id = $2 ORDER BY created_at DESC",
        )
        .bind(&self.tenant_id)
        .bind(plugin_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.iter().map(package_from_row).collect()
    }

    pub async fn installed_packages(
        &self,
    ) -> Result<Vec<PluginPackageRecord>, PostgresPluginError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT p.plugin_id, p.version, p.name, p.description, p.license, p.publisher,
                    p.manifest_json, p.content_hash, p.package_path, p.signature_json,
                    p.signer_key_id, p.signer_public_key, p.trust, p.source,
                    p.capabilities_json, p.sbom_json, p.created_at
             FROM hpa_tenant.plugin_packages p
             JOIN hpa_tenant.plugins pl
               ON pl.tenant_id = p.tenant_id AND pl.id = p.plugin_id
             WHERE p.tenant_id = $1
               AND p.version = pl.installed_version
               AND pl.status != 'uninstalled'",
        )
        .bind(&self.tenant_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.iter().map(package_from_row).collect()
    }

    pub async fn plugin(&self, id: &str) -> Result<Option<PluginRecord>, PostgresPluginError> {
        let mut transaction = self.begin().await?;
        let row = sqlx::query(
            "SELECT id, name, latest_version, status, installed_version, previous_version,
                    trust, license, created_at, updated_at
             FROM hpa_tenant.plugins WHERE tenant_id = $1 AND id = $2",
        )
        .bind(&self.tenant_id)
        .bind(id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.as_ref().map(plugin_from_row).transpose()
    }

    pub async fn list_plugins(&self) -> Result<Vec<PluginRecord>, PostgresPluginError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT id, name, latest_version, status, installed_version, previous_version,
                    trust, license, created_at, updated_at
             FROM hpa_tenant.plugins WHERE tenant_id = $1 ORDER BY updated_at DESC",
        )
        .bind(&self.tenant_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.iter().map(plugin_from_row).collect()
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_plugin_summary(
        &self,
        id: &str,
        name: &str,
        latest_version: &str,
        initial_status: &str,
        trust: &str,
        license: &str,
        created_at: &str,
        updated_at: &str,
    ) -> Result<(), PostgresPluginError> {
        let mut transaction = self.begin().await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.plugins
             (tenant_id, id, name, latest_version, status, installed_version, previous_version,
              trust, license, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, NULL, NULL, $6, $7, $8, $9)
             ON CONFLICT (tenant_id, id) DO UPDATE SET
                name = EXCLUDED.name,
                latest_version = EXCLUDED.latest_version,
                trust = EXCLUDED.trust,
                license = EXCLUDED.license,
                updated_at = EXCLUDED.updated_at",
        )
        .bind(&self.tenant_id)
        .bind(id)
        .bind(name)
        .bind(latest_version)
        .bind(initial_status)
        .bind(trust)
        .bind(license)
        .bind(parse_time(created_at)?)
        .bind(parse_time(updated_at)?)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn set_plugin_installed(
        &self,
        id: &str,
        installed_version: &str,
        previous_version: Option<&str>,
        latest_version: &str,
        updated_at: &str,
    ) -> Result<(), PostgresPluginError> {
        let mut transaction = self.begin().await?;
        sqlx::query(
            "UPDATE hpa_tenant.plugins
             SET installed_version = $1, previous_version = $2, status = 'enabled',
                 latest_version = $3, updated_at = $4
             WHERE tenant_id = $5 AND id = $6",
        )
        .bind(installed_version)
        .bind(previous_version)
        .bind(latest_version)
        .bind(parse_time(updated_at)?)
        .bind(&self.tenant_id)
        .bind(id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn set_plugin_status(
        &self,
        id: &str,
        status: &str,
        updated_at: &str,
    ) -> Result<(), PostgresPluginError> {
        let mut transaction = self.begin().await?;
        sqlx::query(
            "UPDATE hpa_tenant.plugins SET status = $1, updated_at = $2
             WHERE tenant_id = $3 AND id = $4",
        )
        .bind(status)
        .bind(parse_time(updated_at)?)
        .bind(&self.tenant_id)
        .bind(id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn set_plugin_rollback(
        &self,
        id: &str,
        installed_version: &str,
        previous_version: &str,
        updated_at: &str,
    ) -> Result<(), PostgresPluginError> {
        let mut transaction = self.begin().await?;
        sqlx::query(
            "UPDATE hpa_tenant.plugins
             SET status = 'enabled', installed_version = $1, previous_version = $2, updated_at = $3
             WHERE tenant_id = $4 AND id = $5",
        )
        .bind(installed_version)
        .bind(previous_version)
        .bind(parse_time(updated_at)?)
        .bind(&self.tenant_id)
        .bind(id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn set_plugin_uninstalled(
        &self,
        id: &str,
        updated_at: &str,
    ) -> Result<(), PostgresPluginError> {
        let mut transaction = self.begin().await?;
        sqlx::query(
            "UPDATE hpa_tenant.plugins
             SET status = 'uninstalled', installed_version = NULL, previous_version = NULL,
                 updated_at = $1
             WHERE tenant_id = $2 AND id = $3",
        )
        .bind(parse_time(updated_at)?)
        .bind(&self.tenant_id)
        .bind(id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_capability(
        &self,
        plugin_id: &str,
        version: &str,
        kind: &str,
        capability_id: &str,
        capability_version: &str,
        path: &str,
        status: &str,
        config_json: &str,
        permissions_json: &str,
    ) -> Result<(), PostgresPluginError> {
        let config: Value = serde_json::from_str(config_json)?;
        let permissions: Value = serde_json::from_str(permissions_json)?;
        let mut transaction = self.begin().await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.plugin_capabilities
             (tenant_id, plugin_id, version, kind, capability_id, capability_version, path,
              status, config_json, permissions_json)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             ON CONFLICT (tenant_id, plugin_id, version, kind, capability_id) DO UPDATE SET
                capability_version = EXCLUDED.capability_version,
                path = EXCLUDED.path,
                status = EXCLUDED.status,
                config_json = EXCLUDED.config_json,
                permissions_json = EXCLUDED.permissions_json",
        )
        .bind(&self.tenant_id)
        .bind(plugin_id)
        .bind(version)
        .bind(kind)
        .bind(capability_id)
        .bind(capability_version)
        .bind(path)
        .bind(status)
        .bind(Json(&config))
        .bind(Json(&permissions))
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn set_capabilities_status(
        &self,
        plugin_id: &str,
        version: &str,
        status: &str,
    ) -> Result<(), PostgresPluginError> {
        let mut transaction = self.begin().await?;
        sqlx::query(
            "UPDATE hpa_tenant.plugin_capabilities SET status = $1
             WHERE tenant_id = $2 AND plugin_id = $3 AND version = $4",
        )
        .bind(status)
        .bind(&self.tenant_id)
        .bind(plugin_id)
        .bind(version)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn set_capabilities_uninstalled(
        &self,
        plugin_id: &str,
    ) -> Result<(), PostgresPluginError> {
        let mut transaction = self.begin().await?;
        sqlx::query(
            "UPDATE hpa_tenant.plugin_capabilities SET status = 'uninstalled'
             WHERE tenant_id = $1 AND plugin_id = $2",
        )
        .bind(&self.tenant_id)
        .bind(plugin_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn capabilities(
        &self,
        plugin_id: &str,
        version: &str,
    ) -> Result<Vec<PluginCapabilityRecord>, PostgresPluginError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT plugin_id, version, kind, capability_id, capability_version, path, status,
                    config_json, permissions_json
             FROM hpa_tenant.plugin_capabilities
             WHERE tenant_id = $1 AND plugin_id = $2 AND version = $3
             ORDER BY kind ASC, capability_id ASC",
        )
        .bind(&self.tenant_id)
        .bind(plugin_id)
        .bind(version)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.iter().map(capability_from_row).collect()
    }

    pub async fn listing(
        &self,
        plugin_id: &str,
        version: &str,
    ) -> Result<Option<MarketplaceListingRecord>, PostgresPluginError> {
        let mut transaction = self.begin().await?;
        let row = sqlx::query(
            "SELECT plugin_id, version, review_status, review_note, submitted_at, reviewed_at,
                    withdrawn_at, downloads
             FROM hpa_tenant.plugin_market_listings
             WHERE tenant_id = $1 AND plugin_id = $2 AND version = $3",
        )
        .bind(&self.tenant_id)
        .bind(plugin_id)
        .bind(version)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.as_ref().map(listing_from_row).transpose()
    }

    pub async fn list_listings(
        &self,
    ) -> Result<Vec<MarketplaceListingRecord>, PostgresPluginError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT plugin_id, version, review_status, review_note, submitted_at, reviewed_at,
                    withdrawn_at, downloads
             FROM hpa_tenant.plugin_market_listings
             WHERE tenant_id = $1 ORDER BY submitted_at DESC",
        )
        .bind(&self.tenant_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.iter().map(listing_from_row).collect()
    }

    pub async fn upsert_listing_pending(
        &self,
        plugin_id: &str,
        version: &str,
        submitted_at: &str,
    ) -> Result<(), PostgresPluginError> {
        let mut transaction = self.begin().await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.plugin_market_listings
             (tenant_id, plugin_id, version, review_status, review_note, submitted_at,
              reviewed_at, withdrawn_at, downloads)
             VALUES ($1, $2, $3, 'pending', NULL, $4, NULL, NULL, 0)
             ON CONFLICT (tenant_id, plugin_id, version) DO UPDATE SET
                review_status = 'pending',
                review_note = NULL,
                submitted_at = EXCLUDED.submitted_at,
                reviewed_at = NULL,
                withdrawn_at = NULL",
        )
        .bind(&self.tenant_id)
        .bind(plugin_id)
        .bind(version)
        .bind(parse_time(submitted_at)?)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn review_listing(
        &self,
        plugin_id: &str,
        version: &str,
        status: &str,
        note: Option<&str>,
        reviewed_at: &str,
    ) -> Result<u64, PostgresPluginError> {
        let mut transaction = self.begin().await?;
        let result = sqlx::query(
            "UPDATE hpa_tenant.plugin_market_listings
             SET review_status = $1, review_note = $2, reviewed_at = $3
             WHERE tenant_id = $4 AND plugin_id = $5 AND version = $6
               AND review_status = 'pending'",
        )
        .bind(status)
        .bind(note)
        .bind(parse_time(reviewed_at)?)
        .bind(&self.tenant_id)
        .bind(plugin_id)
        .bind(version)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(result.rows_affected())
    }

    pub async fn withdraw_listing(
        &self,
        plugin_id: &str,
        version: &str,
        withdrawn_at: &str,
    ) -> Result<u64, PostgresPluginError> {
        let mut transaction = self.begin().await?;
        let result = sqlx::query(
            "UPDATE hpa_tenant.plugin_market_listings
             SET review_status = 'withdrawn', withdrawn_at = $1
             WHERE tenant_id = $2 AND plugin_id = $3 AND version = $4
               AND review_status != 'withdrawn'",
        )
        .bind(parse_time(withdrawn_at)?)
        .bind(&self.tenant_id)
        .bind(plugin_id)
        .bind(version)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(result.rows_affected())
    }

    pub async fn increment_downloads(
        &self,
        plugin_id: &str,
        version: &str,
    ) -> Result<(), PostgresPluginError> {
        let mut transaction = self.begin().await?;
        sqlx::query(
            "UPDATE hpa_tenant.plugin_market_listings SET downloads = downloads + 1
             WHERE tenant_id = $1 AND plugin_id = $2 AND version = $3",
        )
        .bind(&self.tenant_id)
        .bind(plugin_id)
        .bind(version)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn listing_exists(
        &self,
        plugin_id: &str,
        version: &str,
    ) -> Result<bool, PostgresPluginError> {
        let mut transaction = self.begin().await?;
        let exists: Option<i32> = sqlx::query_scalar(
            "SELECT 1 FROM hpa_tenant.plugin_market_listings
             WHERE tenant_id = $1 AND plugin_id = $2 AND version = $3",
        )
        .bind(&self.tenant_id)
        .bind(plugin_id)
        .bind(version)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(exists.is_some())
    }

    pub async fn insert_review(
        &self,
        id: &str,
        plugin_id: &str,
        version: &str,
        rating: i64,
        comment: Option<&str>,
        created_at: &str,
    ) -> Result<(), PostgresPluginError> {
        let mut transaction = self.begin().await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.plugin_reviews
             (tenant_id, id, plugin_id, version, rating, comment, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&self.tenant_id)
        .bind(id)
        .bind(plugin_id)
        .bind(version)
        .bind(rating as i32)
        .bind(comment)
        .bind(parse_time(created_at)?)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn reviews(
        &self,
        plugin_id: &str,
        version: &str,
    ) -> Result<Vec<MarketplaceReviewRecord>, PostgresPluginError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT id, plugin_id, version, rating, comment, created_at
             FROM hpa_tenant.plugin_reviews
             WHERE tenant_id = $1 AND plugin_id = $2 AND version = $3
             ORDER BY created_at DESC",
        )
        .bind(&self.tenant_id)
        .bind(plugin_id)
        .bind(version)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.iter().map(review_from_row).collect()
    }

    pub async fn rating(
        &self,
        plugin_id: &str,
        version: &str,
    ) -> Result<(f64, i64), PostgresPluginError> {
        let mut transaction = self.begin().await?;
        let row: (f64, i64) = sqlx::query_as(
            "SELECT COALESCE(AVG(rating), 0)::float8, COUNT(*)::bigint
             FROM hpa_tenant.plugin_reviews
             WHERE tenant_id = $1 AND plugin_id = $2 AND version = $3",
        )
        .bind(&self.tenant_id)
        .bind(plugin_id)
        .bind(version)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(row)
    }

    pub async fn insert_event(
        &self,
        id: &str,
        at: &str,
        plugin_id: Option<&str>,
        version: Option<&str>,
        kind: &str,
        detail_json: &str,
    ) -> Result<(), PostgresPluginError> {
        let detail: Value = serde_json::from_str(detail_json)?;
        let mut transaction = self.begin().await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.plugin_events
             (tenant_id, id, at, plugin_id, version, kind, detail_json)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&self.tenant_id)
        .bind(id)
        .bind(parse_time(at)?)
        .bind(plugin_id)
        .bind(version)
        .bind(kind)
        .bind(Json(&detail))
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.marketplace_event_stream
             (tenant_id, id, at, plugin_id, version, kind, detail_json)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&self.tenant_id)
        .bind(id)
        .bind(parse_time(at)?)
        .bind(plugin_id)
        .bind(version)
        .bind(kind)
        .bind(Json(&detail))
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn events(
        &self,
        plugin_id: &str,
        limit: i64,
    ) -> Result<Vec<PluginEventRecord>, PostgresPluginError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT id, at, plugin_id, version, kind, detail_json
             FROM hpa_tenant.plugin_events
             WHERE tenant_id = $1 AND plugin_id = $2 ORDER BY at DESC LIMIT $3",
        )
        .bind(&self.tenant_id)
        .bind(plugin_id)
        .bind(limit.clamp(1, 200))
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.iter().map(event_from_row).collect()
    }

    pub async fn list_sources(&self) -> Result<Vec<MarketplaceSourceRecord>, PostgresPluginError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT id, label, index_uri, enabled, last_sequence, last_refresh_at,
                    created_at, updated_at
             FROM hpa_tenant.marketplace_sources
             WHERE tenant_id = $1 ORDER BY lower(id)",
        )
        .bind(&self.tenant_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.iter().map(source_from_row).collect()
    }

    pub async fn source_keys(
        &self,
        source_id: &str,
        active_only: bool,
    ) -> Result<Vec<SourceKeyRecord>, PostgresPluginError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT source_id, key_id, public_key, label, valid_from, valid_until, revoked_at,
                    created_at
             FROM hpa_tenant.marketplace_source_keys
             WHERE tenant_id = $1 AND source_id = $2
               AND ($3 = FALSE OR revoked_at IS NULL)
             ORDER BY created_at DESC",
        )
        .bind(&self.tenant_id)
        .bind(source_id)
        .bind(active_only)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.iter().map(source_key_from_row).collect()
    }

    pub async fn source_package_counts(
        &self,
        source_id: &str,
    ) -> Result<(i64, i64, i64), PostgresPluginError> {
        let mut transaction = self.begin().await?;
        let row: (i64, i64, i64) = sqlx::query_as(
            "SELECT count(*),
                    COALESCE(sum(CASE WHEN review_status = 'approved'
                                      AND withdrawn_at IS NULL THEN 1 ELSE 0 END), 0)::bigint,
                    COALESCE(sum(CASE WHEN withdrawn_at IS NOT NULL THEN 1 ELSE 0 END), 0)::bigint
             FROM hpa_tenant.marketplace_remote_packages
             WHERE tenant_id = $1 AND source_id = $2",
        )
        .bind(&self.tenant_id)
        .bind(source_id)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(row)
    }

    pub async fn list_publishers(&self) -> Result<Vec<PublisherRecord>, PostgresPluginError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT id, name, status, created_at, updated_at
             FROM hpa_tenant.plugin_publishers
             WHERE tenant_id = $1 ORDER BY lower(id)",
        )
        .bind(&self.tenant_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.iter().map(publisher_from_row).collect()
    }

    pub async fn publisher_keys(
        &self,
        publisher_id: &str,
    ) -> Result<Vec<PublisherKeyRecord>, PostgresPluginError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT publisher_id, key_id, public_key, label, valid_from, valid_until,
                    revoked_at, created_at
             FROM hpa_tenant.plugin_publisher_keys
             WHERE tenant_id = $1 AND publisher_id = $2
             ORDER BY created_at DESC",
        )
        .bind(&self.tenant_id)
        .bind(publisher_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.iter().map(publisher_key_from_row).collect()
    }

    pub async fn upsert_publisher(
        &self,
        publisher_id: &str,
        name: &str,
        now: &str,
    ) -> Result<(), PostgresPluginError> {
        let mut transaction = self.begin().await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.plugin_publishers
             (tenant_id, id, name, status, created_at, updated_at)
             VALUES ($1, $2, $3, 'active', $4, $5)
             ON CONFLICT (tenant_id, id) DO UPDATE SET
                name = EXCLUDED.name,
                updated_at = EXCLUDED.updated_at",
        )
        .bind(&self.tenant_id)
        .bind(publisher_id)
        .bind(name)
        .bind(parse_time(now)?)
        .bind(parse_time(now)?)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn publisher_active(&self, publisher_id: &str) -> Result<bool, PostgresPluginError> {
        let mut transaction = self.begin().await?;
        let exists: Option<i32> = sqlx::query_scalar(
            "SELECT 1 FROM hpa_tenant.plugin_publishers
             WHERE tenant_id = $1 AND id = $2 AND status = 'active'",
        )
        .bind(&self.tenant_id)
        .bind(publisher_id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(exists.is_some())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_publisher_key(
        &self,
        publisher_id: &str,
        key_id: &str,
        public_key: &str,
        label: &str,
        valid_from: Option<&str>,
        valid_until: Option<&str>,
        now: &str,
        trust_label: &str,
        upsert_trusted_key: bool,
    ) -> Result<(), PostgresPluginError> {
        let mut transaction = self.begin().await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.plugin_publisher_keys
             (tenant_id, publisher_id, key_id, public_key, label, valid_from, valid_until,
              revoked_at, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, NULL, $8)
             ON CONFLICT (tenant_id, publisher_id, key_id) DO UPDATE SET
                public_key = EXCLUDED.public_key,
                label = EXCLUDED.label,
                valid_from = EXCLUDED.valid_from,
                valid_until = EXCLUDED.valid_until,
                revoked_at = NULL",
        )
        .bind(&self.tenant_id)
        .bind(publisher_id)
        .bind(key_id)
        .bind(public_key)
        .bind(label)
        .bind(valid_from.map(parse_time).transpose()?)
        .bind(valid_until.map(parse_time).transpose()?)
        .bind(parse_time(now)?)
        .execute(&mut *transaction)
        .await?;
        if upsert_trusted_key {
            sqlx::query(
                "INSERT INTO hpa_tenant.plugin_trusted_keys
                 (tenant_id, key_id, public_key, label, trust, created_at)
                 VALUES ($1, $2, $3, $4, 'verified', $5)
                 ON CONFLICT (tenant_id, key_id) DO UPDATE SET
                    public_key = EXCLUDED.public_key,
                    label = EXCLUDED.label,
                    trust = 'verified'",
            )
            .bind(&self.tenant_id)
            .bind(key_id)
            .bind(public_key)
            .bind(trust_label)
            .bind(parse_time(now)?)
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn revoke_publisher_key(
        &self,
        publisher_id: &str,
        key_id: &str,
        now: &str,
    ) -> Result<bool, PostgresPluginError> {
        let mut transaction = self.begin().await?;
        let result = sqlx::query(
            "UPDATE hpa_tenant.plugin_publisher_keys SET revoked_at = $1
             WHERE tenant_id = $2 AND publisher_id = $3 AND key_id = $4
               AND revoked_at IS NULL",
        )
        .bind(parse_time(now)?)
        .bind(&self.tenant_id)
        .bind(publisher_id)
        .bind(key_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn publisher_key(
        &self,
        publisher_id: &str,
        key_id: &str,
    ) -> Result<Option<RegisteredKey>, PostgresPluginError> {
        let mut transaction = self.begin().await?;
        let row = sqlx::query(
            "SELECT public_key, valid_from, valid_until, revoked_at
             FROM hpa_tenant.plugin_publisher_keys
             WHERE tenant_id = $1 AND publisher_id = $2 AND key_id = $3",
        )
        .bind(&self.tenant_id)
        .bind(publisher_id)
        .bind(key_id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.map(|row| registered_key_from_row(&row)).transpose()
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_source_with_key(
        &self,
        source_id: &str,
        label: &str,
        index_uri: &str,
        key_id: &str,
        public_key: &str,
        key_label: &str,
        now: &str,
    ) -> Result<(), PostgresPluginError> {
        let mut transaction = self.begin().await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.marketplace_sources
             (tenant_id, id, label, index_uri, enabled, last_sequence, last_refresh_at,
              created_at, updated_at)
             VALUES ($1, $2, $3, $4, TRUE, 0, NULL, $5, $6)
             ON CONFLICT (tenant_id, id) DO UPDATE SET
                label = EXCLUDED.label,
                index_uri = EXCLUDED.index_uri,
                enabled = TRUE,
                updated_at = EXCLUDED.updated_at",
        )
        .bind(&self.tenant_id)
        .bind(source_id)
        .bind(label)
        .bind(index_uri)
        .bind(parse_time(now)?)
        .bind(parse_time(now)?)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.marketplace_source_keys
             (tenant_id, source_id, key_id, public_key, label, valid_from, valid_until,
              revoked_at, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, NULL, NULL, $7)
             ON CONFLICT (tenant_id, source_id, key_id) DO UPDATE SET
                public_key = EXCLUDED.public_key,
                label = EXCLUDED.label,
                revoked_at = NULL",
        )
        .bind(&self.tenant_id)
        .bind(source_id)
        .bind(key_id)
        .bind(public_key)
        .bind(key_label)
        .bind(parse_time(now)?)
        .bind(parse_time(now)?)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn rotate_source_key(
        &self,
        source_id: &str,
        key_id: &str,
        public_key: &str,
        label: &str,
        now: &str,
    ) -> Result<(), PostgresPluginError> {
        let mut transaction = self.begin().await?;
        sqlx::query(
            "UPDATE hpa_tenant.marketplace_source_keys SET revoked_at = $1
             WHERE tenant_id = $2 AND source_id = $3 AND revoked_at IS NULL",
        )
        .bind(parse_time(now)?)
        .bind(&self.tenant_id)
        .bind(source_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.marketplace_source_keys
             (tenant_id, source_id, key_id, public_key, label, valid_from, valid_until,
              revoked_at, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, NULL, NULL, $7)",
        )
        .bind(&self.tenant_id)
        .bind(source_id)
        .bind(key_id)
        .bind(public_key)
        .bind(label)
        .bind(parse_time(now)?)
        .bind(parse_time(now)?)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn enabled_source(
        &self,
        source_id: &str,
    ) -> Result<Option<MarketplaceSourceRecord>, PostgresPluginError> {
        let mut transaction = self.begin().await?;
        let row = sqlx::query(
            "SELECT id, label, index_uri, enabled, last_sequence, last_refresh_at,
                    created_at, updated_at
             FROM hpa_tenant.marketplace_sources
             WHERE tenant_id = $1 AND id = $2 AND enabled = TRUE",
        )
        .bind(&self.tenant_id)
        .bind(source_id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.as_ref().map(source_from_row).transpose()
    }

    pub async fn source_key(
        &self,
        source_id: &str,
        key_id: &str,
    ) -> Result<Option<RegisteredKey>, PostgresPluginError> {
        let mut transaction = self.begin().await?;
        let row = sqlx::query(
            "SELECT public_key, valid_from, valid_until, revoked_at
             FROM hpa_tenant.marketplace_source_keys
             WHERE tenant_id = $1 AND source_id = $2 AND key_id = $3",
        )
        .bind(&self.tenant_id)
        .bind(source_id)
        .bind(key_id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.map(|row| registered_key_from_row(&row)).transpose()
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_remote_package(
        &self,
        source_id: &str,
        plugin_id: &str,
        version: &str,
        publisher_id: &str,
        signer_key_id: &str,
        package_sha256: &str,
        package_path: &str,
        review_status: &str,
        index_sequence: i64,
        published_at: Option<&str>,
        provenance_json: Option<&str>,
        oci_json: Option<&str>,
    ) -> Result<(), PostgresPluginError> {
        let provenance: Option<Value> = provenance_json.map(serde_json::from_str).transpose()?;
        let oci: Option<Value> = oci_json.map(serde_json::from_str).transpose()?;
        let mut transaction = self.begin().await?;
        sqlx::query(
            "INSERT INTO hpa_tenant.marketplace_remote_packages
             (tenant_id, source_id, plugin_id, version, publisher_id, signer_key_id,
              package_sha256, package_path, review_status, index_sequence, published_at,
              withdrawn_at, revocation_reason, provenance_json, oci_json)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, NULL, NULL, $12, $13)
             ON CONFLICT (tenant_id, source_id, plugin_id, version) DO UPDATE SET
                publisher_id = EXCLUDED.publisher_id,
                signer_key_id = EXCLUDED.signer_key_id,
                package_sha256 = EXCLUDED.package_sha256,
                package_path = EXCLUDED.package_path,
                review_status = EXCLUDED.review_status,
                index_sequence = EXCLUDED.index_sequence,
                published_at = EXCLUDED.published_at,
                withdrawn_at = NULL,
                revocation_reason = NULL,
                provenance_json = EXCLUDED.provenance_json,
                oci_json = EXCLUDED.oci_json",
        )
        .bind(&self.tenant_id)
        .bind(source_id)
        .bind(plugin_id)
        .bind(version)
        .bind(publisher_id)
        .bind(signer_key_id)
        .bind(package_sha256)
        .bind(package_path)
        .bind(review_status)
        .bind(index_sequence)
        .bind(published_at.map(parse_time).transpose()?)
        .bind(provenance.map(Json))
        .bind(oci.map(Json))
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn withdraw_remote_package(
        &self,
        source_id: &str,
        plugin_id: &str,
        version: &str,
        at: &str,
        reason: &str,
    ) -> Result<(), PostgresPluginError> {
        let mut transaction = self.begin().await?;
        sqlx::query(
            "UPDATE hpa_tenant.marketplace_remote_packages
             SET withdrawn_at = $1, revocation_reason = $2
             WHERE tenant_id = $3 AND source_id = $4 AND plugin_id = $5 AND version = $6",
        )
        .bind(parse_time(at)?)
        .bind(reason)
        .bind(&self.tenant_id)
        .bind(source_id)
        .bind(plugin_id)
        .bind(version)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn plugin_installed_version(
        &self,
        plugin_id: &str,
    ) -> Result<Option<Option<String>>, PostgresPluginError> {
        let mut transaction = self.begin().await?;
        let row: Option<Option<String>> = sqlx::query_scalar(
            "SELECT installed_version FROM hpa_tenant.plugins
             WHERE tenant_id = $1 AND id = $2",
        )
        .bind(&self.tenant_id)
        .bind(plugin_id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(row)
    }

    pub async fn active_remote_packages(
        &self,
        source_id: &str,
    ) -> Result<Vec<(String, String)>, PostgresPluginError> {
        let mut transaction = self.begin().await?;
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT plugin_id, version FROM hpa_tenant.marketplace_remote_packages
             WHERE tenant_id = $1 AND source_id = $2 AND withdrawn_at IS NULL",
        )
        .bind(&self.tenant_id)
        .bind(source_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(rows)
    }

    pub async fn update_source_sequence(
        &self,
        source_id: &str,
        sequence: i64,
        now: &str,
    ) -> Result<(), PostgresPluginError> {
        let mut transaction = self.begin().await?;
        sqlx::query(
            "UPDATE hpa_tenant.marketplace_sources
             SET last_sequence = $1, last_refresh_at = $2, updated_at = $3
             WHERE tenant_id = $4 AND id = $5",
        )
        .bind(sequence)
        .bind(parse_time(now)?)
        .bind(parse_time(now)?)
        .bind(&self.tenant_id)
        .bind(source_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn remote_packages(
        &self,
        source_id: Option<&str>,
    ) -> Result<Vec<RemotePackageRecord>, PostgresPluginError> {
        let mut transaction = self.begin().await?;
        let rows = sqlx::query(
            "SELECT source_id, plugin_id, version, publisher_id, signer_key_id, package_sha256,
                    package_path, review_status, index_sequence, published_at, withdrawn_at,
                    revocation_reason, provenance_json, oci_json
             FROM hpa_tenant.marketplace_remote_packages
             WHERE tenant_id = $1 AND ($2::text IS NULL OR source_id = $2)
             ORDER BY source_id, plugin_id, version DESC",
        )
        .bind(&self.tenant_id)
        .bind(source_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.iter().map(remote_package_from_row).collect()
    }

    pub async fn remote_package(
        &self,
        source_id: &str,
        plugin_id: &str,
        version: &str,
    ) -> Result<Option<RemotePackageRecord>, PostgresPluginError> {
        let mut transaction = self.begin().await?;
        let row = sqlx::query(
            "SELECT source_id, plugin_id, version, publisher_id, signer_key_id, package_sha256,
                    package_path, review_status, index_sequence, published_at, withdrawn_at,
                    revocation_reason, provenance_json, oci_json
             FROM hpa_tenant.marketplace_remote_packages
             WHERE tenant_id = $1 AND source_id = $2 AND plugin_id = $3 AND version = $4",
        )
        .bind(&self.tenant_id)
        .bind(source_id)
        .bind(plugin_id)
        .bind(version)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.as_ref().map(remote_package_from_row).transpose()
    }
}

fn trusted_key_from_row(row: &PgRow) -> Result<TrustedKeyRecord, PostgresPluginError> {
    Ok(TrustedKeyRecord {
        key_id: row.try_get("key_id")?,
        public_key: row.try_get("public_key")?,
        label: row.try_get("label")?,
        trust: row.try_get("trust")?,
        created_at: format_time(row.try_get("created_at")?),
    })
}

fn package_from_row(row: &PgRow) -> Result<PluginPackageRecord, PostgresPluginError> {
    let manifest: Value = row.try_get("manifest_json")?;
    let publisher: Option<Value> = row.try_get("publisher")?;
    let signature: Option<Value> = row.try_get("signature_json")?;
    let capabilities: Value = row.try_get("capabilities_json")?;
    let sbom: Value = row.try_get("sbom_json")?;
    Ok(PluginPackageRecord {
        plugin_id: row.try_get("plugin_id")?,
        version: row.try_get("version")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        license: row.try_get("license")?,
        publisher: publisher.and_then(|value| value.as_str().map(str::to_string)),
        manifest_json: serde_json::to_string(&manifest)?,
        content_hash: row.try_get("content_hash")?,
        package_path: row.try_get("package_path")?,
        signature_json: signature
            .map(|value| serde_json::to_string(&value))
            .transpose()?,
        signer_key_id: row.try_get("signer_key_id")?,
        signer_public_key: row.try_get("signer_public_key")?,
        trust: row.try_get("trust")?,
        source: row.try_get("source")?,
        capabilities_json: serde_json::to_string(&capabilities)?,
        sbom_json: serde_json::to_string(&sbom)?,
        created_at: format_time(row.try_get("created_at")?),
    })
}

fn plugin_from_row(row: &PgRow) -> Result<PluginRecord, PostgresPluginError> {
    Ok(PluginRecord {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        latest_version: row.try_get("latest_version")?,
        status: row.try_get("status")?,
        installed_version: row.try_get("installed_version")?,
        previous_version: row.try_get("previous_version")?,
        trust: row.try_get("trust")?,
        license: row.try_get("license")?,
        created_at: format_time(row.try_get("created_at")?),
        updated_at: format_time(row.try_get("updated_at")?),
    })
}

fn capability_from_row(row: &PgRow) -> Result<PluginCapabilityRecord, PostgresPluginError> {
    let config: Value = row.try_get("config_json")?;
    let permissions: Value = row.try_get("permissions_json")?;
    Ok(PluginCapabilityRecord {
        plugin_id: row.try_get("plugin_id")?,
        version: row.try_get("version")?,
        kind: row.try_get("kind")?,
        capability_id: row.try_get("capability_id")?,
        capability_version: row.try_get("capability_version")?,
        path: row.try_get("path")?,
        status: row.try_get("status")?,
        config_json: serde_json::to_string(&config)?,
        permissions_json: serde_json::to_string(&permissions)?,
    })
}

fn listing_from_row(row: &PgRow) -> Result<MarketplaceListingRecord, PostgresPluginError> {
    Ok(MarketplaceListingRecord {
        plugin_id: row.try_get("plugin_id")?,
        version: row.try_get("version")?,
        review_status: row.try_get("review_status")?,
        review_note: row.try_get("review_note")?,
        submitted_at: format_time(row.try_get("submitted_at")?),
        reviewed_at: row
            .try_get::<Option<OffsetDateTime>, _>("reviewed_at")?
            .map(format_time),
        withdrawn_at: row
            .try_get::<Option<OffsetDateTime>, _>("withdrawn_at")?
            .map(format_time),
        downloads: row.try_get("downloads")?,
    })
}

fn review_from_row(row: &PgRow) -> Result<MarketplaceReviewRecord, PostgresPluginError> {
    Ok(MarketplaceReviewRecord {
        id: row.try_get("id")?,
        plugin_id: row.try_get("plugin_id")?,
        version: row.try_get("version")?,
        rating: i64::from(row.try_get::<i32, _>("rating")?),
        comment: row.try_get("comment")?,
        created_at: format_time(row.try_get("created_at")?),
    })
}

fn event_from_row(row: &PgRow) -> Result<PluginEventRecord, PostgresPluginError> {
    let detail: Value = row.try_get("detail_json")?;
    Ok(PluginEventRecord {
        id: row.try_get("id")?,
        at: format_time(row.try_get("at")?),
        plugin_id: row.try_get("plugin_id")?,
        version: row.try_get("version")?,
        kind: row.try_get("kind")?,
        detail_json: serde_json::to_string(&detail)?,
    })
}

fn source_from_row(row: &PgRow) -> Result<MarketplaceSourceRecord, PostgresPluginError> {
    Ok(MarketplaceSourceRecord {
        id: row.try_get("id")?,
        label: row.try_get("label")?,
        index_uri: row.try_get("index_uri")?,
        enabled: row.try_get("enabled")?,
        last_sequence: row.try_get("last_sequence")?,
        last_refresh_at: row
            .try_get::<Option<OffsetDateTime>, _>("last_refresh_at")?
            .map(format_time),
        created_at: format_time(row.try_get("created_at")?),
        updated_at: format_time(row.try_get("updated_at")?),
    })
}

fn source_key_from_row(row: &PgRow) -> Result<SourceKeyRecord, PostgresPluginError> {
    Ok(SourceKeyRecord {
        source_id: row.try_get("source_id")?,
        key_id: row.try_get("key_id")?,
        public_key: row.try_get("public_key")?,
        label: row.try_get("label")?,
        valid_from: row
            .try_get::<Option<OffsetDateTime>, _>("valid_from")?
            .map(format_time),
        valid_until: row
            .try_get::<Option<OffsetDateTime>, _>("valid_until")?
            .map(format_time),
        revoked_at: row
            .try_get::<Option<OffsetDateTime>, _>("revoked_at")?
            .map(format_time),
        created_at: format_time(row.try_get("created_at")?),
    })
}

fn publisher_from_row(row: &PgRow) -> Result<PublisherRecord, PostgresPluginError> {
    Ok(PublisherRecord {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        status: row.try_get("status")?,
        created_at: format_time(row.try_get("created_at")?),
        updated_at: format_time(row.try_get("updated_at")?),
    })
}

fn publisher_key_from_row(row: &PgRow) -> Result<PublisherKeyRecord, PostgresPluginError> {
    Ok(PublisherKeyRecord {
        publisher_id: row.try_get("publisher_id")?,
        key_id: row.try_get("key_id")?,
        public_key: row.try_get("public_key")?,
        label: row.try_get("label")?,
        valid_from: row
            .try_get::<Option<OffsetDateTime>, _>("valid_from")?
            .map(format_time),
        valid_until: row
            .try_get::<Option<OffsetDateTime>, _>("valid_until")?
            .map(format_time),
        revoked_at: row
            .try_get::<Option<OffsetDateTime>, _>("revoked_at")?
            .map(format_time),
        created_at: format_time(row.try_get("created_at")?),
    })
}

fn registered_key_from_row(row: &PgRow) -> Result<RegisteredKey, PostgresPluginError> {
    Ok(RegisteredKey {
        public_key: row.try_get("public_key")?,
        valid_from: row
            .try_get::<Option<OffsetDateTime>, _>("valid_from")?
            .map(format_time),
        valid_until: row
            .try_get::<Option<OffsetDateTime>, _>("valid_until")?
            .map(format_time),
        revoked_at: row
            .try_get::<Option<OffsetDateTime>, _>("revoked_at")?
            .map(format_time),
    })
}

fn remote_package_from_row(row: &PgRow) -> Result<RemotePackageRecord, PostgresPluginError> {
    let provenance: Option<Value> = row.try_get("provenance_json")?;
    let oci: Option<Value> = row.try_get("oci_json")?;
    Ok(RemotePackageRecord {
        source_id: row.try_get("source_id")?,
        plugin_id: row.try_get("plugin_id")?,
        version: row.try_get("version")?,
        publisher_id: row.try_get("publisher_id")?,
        signer_key_id: row.try_get("signer_key_id")?,
        package_sha256: row.try_get("package_sha256")?,
        package_path: row.try_get("package_path")?,
        review_status: row.try_get("review_status")?,
        index_sequence: row.try_get("index_sequence")?,
        published_at: row
            .try_get::<Option<OffsetDateTime>, _>("published_at")?
            .map(format_time),
        withdrawn_at: row
            .try_get::<Option<OffsetDateTime>, _>("withdrawn_at")?
            .map(format_time),
        revocation_reason: row.try_get("revocation_reason")?,
        provenance_json: provenance
            .map(|value| serde_json::to_string(&value))
            .transpose()?,
        oci_json: oci.map(|value| serde_json::to_string(&value)).transpose()?,
    })
}

fn parse_time(raw: &str) -> Result<OffsetDateTime, PostgresPluginError> {
    OffsetDateTime::parse(raw, &Rfc3339).map_err(|error| {
        PostgresPluginError::Invalid(format!("invalid timestamp `{raw}`: {error}"))
    })
}

fn format_time(value: OffsetDateTime) -> String {
    value.format(&Rfc3339).unwrap_or_default()
}
