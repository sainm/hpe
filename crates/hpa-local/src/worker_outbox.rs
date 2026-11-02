use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{FromRow, SqlitePool};

#[derive(Clone)]
pub struct WorkerOutbox {
    pool: SqlitePool,
    path: PathBuf,
}

#[derive(Debug, Clone, FromRow)]
pub struct WorkerOutboxEntry {
    pub id: i64,
    pub job_id: String,
    pub lease_id: String,
    pub action: String,
    pub payload_json: String,
    pub attempts: i64,
}

impl WorkerOutbox {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "cannot create worker outbox directory `{}`: {error}",
                    parent.display()
                )
            })?;
        }
        let options = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await
            .map_err(|error| format!("cannot open worker outbox `{}`: {error}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .map_err(|error| format!("cannot secure worker outbox: {error}"))?;
        }
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS worker_outbox (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                job_id TEXT NOT NULL,
                lease_id TEXT NOT NULL,
                action TEXT NOT NULL,
                payload_hash TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                attempts INTEGER NOT NULL DEFAULT 0,
                last_error TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                UNIQUE (job_id, lease_id, action, payload_hash)
            )",
        )
        .execute(&pool)
        .await
        .map_err(|error| format!("cannot initialize worker outbox: {error}"))?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS worker_outbox_pending
             ON worker_outbox (id)",
        )
        .execute(&pool)
        .await
        .map_err(|error| format!("cannot initialize worker outbox index: {error}"))?;
        Ok(Self { pool, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn enqueue(
        &self,
        job_id: &str,
        lease_id: &str,
        action: &str,
        payload: &Value,
    ) -> Result<(), String> {
        let payload_json = serde_json::to_string(payload)
            .map_err(|error| format!("invalid report payload: {error}"))?;
        let payload_hash = format!("{:x}", Sha256::digest(payload_json.as_bytes()));
        let now = now_ms();
        sqlx::query(
            "INSERT INTO worker_outbox
             (job_id, lease_id, action, payload_hash, payload_json, attempts, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, 0, ?, ?)
             ON CONFLICT(job_id, lease_id, action, payload_hash) DO NOTHING",
        )
        .bind(job_id)
        .bind(lease_id)
        .bind(action)
        .bind(payload_hash)
        .bind(payload_json)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|error| format!("cannot persist worker report: {error}"))?;
        Ok(())
    }

    pub async fn next(&self) -> Result<Option<WorkerOutboxEntry>, String> {
        sqlx::query_as::<_, WorkerOutboxEntry>(
            "SELECT id, job_id, lease_id, action, payload_json, attempts
             FROM worker_outbox ORDER BY id ASC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| format!("cannot read worker outbox: {error}"))
    }

    pub async fn remove(&self, id: i64) -> Result<(), String> {
        sqlx::query("DELETE FROM worker_outbox WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|error| format!("cannot acknowledge worker report: {error}"))?;
        Ok(())
    }

    pub async fn record_failure(&self, id: i64, error: &str) -> Result<(), String> {
        let excerpt: String = error.chars().take(500).collect();
        sqlx::query(
            "UPDATE worker_outbox
             SET attempts = attempts + 1, last_error = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(excerpt)
        .bind(now_ms())
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|error| format!("cannot record worker report failure: {error}"))?;
        Ok(())
    }

    pub async fn discard_lease(&self, lease_id: &str) -> Result<u64, String> {
        let result = sqlx::query("DELETE FROM worker_outbox WHERE lease_id = ?")
            .bind(lease_id)
            .execute(&self.pool)
            .await
            .map_err(|error| format!("cannot discard stale worker reports: {error}"))?;
        Ok(result.rows_affected())
    }

    pub async fn pending_count(&self) -> Result<i64, String> {
        sqlx::query_scalar("SELECT COUNT(*) FROM worker_outbox")
            .fetch_one(&self.pool)
            .await
            .map_err(|error| format!("cannot count worker outbox: {error}"))
    }
}

fn now_ms() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp_nanos() as i64 / 1_000_000
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use uuid::Uuid;

    use super::*;

    async fn temp_outbox() -> WorkerOutbox {
        let path = std::env::temp_dir()
            .join(format!("hpa-worker-outbox-{}", Uuid::now_v7()))
            .join("outbox.db");
        WorkerOutbox::open(path).await.expect("outbox opens")
    }

    #[tokio::test]
    async fn reports_are_ordered_deduplicated_and_reopenable() {
        let path = std::env::temp_dir()
            .join(format!("hpa-worker-outbox-{}", Uuid::now_v7()))
            .join("outbox.db");
        {
            let outbox = WorkerOutbox::open(&path).await.expect("outbox opens");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    std::fs::metadata(outbox.path())
                        .expect("outbox metadata")
                        .permissions()
                        .mode()
                        & 0o777,
                    0o600
                );
            }
            outbox
                .enqueue("job_1", "lease_1", "start", &json!({"accepted": true}))
                .await
                .expect("start");
            outbox
                .enqueue("job_1", "lease_1", "start", &json!({"accepted": true}))
                .await
                .expect("deduplicated start");
            outbox
                .enqueue("job_1", "lease_1", "events", &json!({"accepted": true}))
                .await
                .expect("events");
            assert_eq!(outbox.pending_count().await.unwrap(), 2);
        }

        let reopened = WorkerOutbox::open(&path).await.expect("reopen");
        let first = reopened.next().await.unwrap().expect("first");
        assert_eq!(first.action, "start");
        reopened.remove(first.id).await.unwrap();
        let second = reopened.next().await.unwrap().expect("second");
        assert_eq!(second.action, "events");
        reopened.discard_lease("lease_1").await.unwrap();
        assert_eq!(reopened.pending_count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn failures_increment_attempts_without_losing_the_report() {
        let outbox = temp_outbox().await;
        outbox
            .enqueue(
                "job_1",
                "lease_1",
                "complete",
                &json!({"status": "succeeded"}),
            )
            .await
            .unwrap();
        let entry = outbox.next().await.unwrap().expect("entry");
        outbox
            .record_failure(entry.id, "temporary network failure")
            .await
            .unwrap();
        let retried = outbox.next().await.unwrap().expect("retry");
        assert_eq!(retried.attempts, 1);
        assert_eq!(retried.id, entry.id);
    }
}
