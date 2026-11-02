use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

#[derive(Debug, Error)]
pub enum AiWorkerError {
    #[error("AI worker process is not running")]
    Closed,
    #[error("AI worker I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("AI worker protocol error: {0}")]
    Protocol(String),
    #[error("AI worker request timed out after {0:?}")]
    Timeout(Duration),
}

pub struct AiWorkerProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    request_token: String,
}

pub struct AiWorkerConfig {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub request_token: String,
    pub environment: BTreeMap<String, String>,
}

pub struct AiWorkerStart<'a> {
    pub session_id: &'a str,
    pub tenant_id: &'a str,
    pub agent_version_id: &'a str,
    pub input: Value,
    pub instructions: Option<&'a str>,
    pub tools: &'a [Value],
    pub budget: Value,
    pub tool_token: Option<&'a str>,
    pub model: Option<&'a str>,
}

pub fn checkpoint_schema_name(tenant_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(tenant_id.as_bytes());
    format!("hpa_agent_cp_{}", &format!("{:x}", hasher.finalize())[..20])
}

impl AiWorkerProcess {
    pub async fn spawn(config: AiWorkerConfig) -> Result<Self, AiWorkerError> {
        if config.request_token.trim().is_empty() {
            return Err(AiWorkerError::Protocol(
                "AI worker request token is empty".to_string(),
            ));
        }
        let mut command = Command::new(&config.executable);
        command
            .args(&config.args)
            .env("HPA_AI_WORKER_TOKEN", &config.request_token)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        for (key, value) in config.environment {
            command.env(key, value);
        }
        let mut child = command.spawn()?;
        let stdin = child.stdin.take().ok_or(AiWorkerError::Closed)?;
        let stdout = child.stdout.take().ok_or(AiWorkerError::Closed)?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            request_token: config.request_token,
        })
    }

    pub async fn start(&mut self, start: AiWorkerStart<'_>) -> Result<(), AiWorkerError> {
        let mut request = json!({
            "type": "start",
            "sessionId": start.session_id,
            "requestToken": self.request_token,
            "tenantId": start.tenant_id,
            "agentVersionId": start.agent_version_id,
            "input": start.input,
            "tools": start.tools,
            "budget": start.budget,
        });
        if let Some(instructions) = start.instructions {
            request["instructions"] = Value::String(instructions.to_string());
        }
        if let Some(tool_token) = start.tool_token {
            request["toolToken"] = Value::String(tool_token.to_string());
        }
        if let Some(model) = start.model {
            request["model"] = Value::String(model.to_string());
        }
        if let Some(traceparent) = current_traceparent() {
            request["traceparent"] = Value::String(traceparent);
        }
        self.write(&request).await
    }

    pub async fn resume(
        &mut self,
        session_id: &str,
        approval: Value,
        budget: Option<Value>,
        tool_token: Option<&str>,
    ) -> Result<(), AiWorkerError> {
        let mut request = json!({
            "type": "resume",
            "sessionId": session_id,
            "requestToken": self.request_token,
            "approval": approval,
        });
        if let Some(budget) = budget {
            request["budget"] = budget;
        }
        if let Some(tool_token) = tool_token {
            request["toolToken"] = Value::String(tool_token.to_string());
        }
        if let Some(traceparent) = current_traceparent() {
            request["traceparent"] = Value::String(traceparent);
        }
        self.write(&request).await
    }

    #[allow(dead_code)]
    pub async fn cancel(&mut self, session_id: &str, reason: &str) -> Result<(), AiWorkerError> {
        self.write(&json!({
            "type": "cancel",
            "sessionId": session_id,
            "requestToken": self.request_token,
            "reason": reason,
        }))
        .await
    }

    pub async fn next_event(&mut self, timeout: Duration) -> Result<Value, AiWorkerError> {
        let mut line = String::new();
        let read = tokio::time::timeout(timeout, self.stdout.read_line(&mut line))
            .await
            .map_err(|_| AiWorkerError::Timeout(timeout))??;
        if read == 0 {
            return Err(AiWorkerError::Closed);
        }
        let event: Value = serde_json::from_str(&line)
            .map_err(|error| AiWorkerError::Protocol(error.to_string()))?;
        if event.get("type").and_then(Value::as_str).is_none() {
            return Err(AiWorkerError::Protocol(
                "AI worker event has no type".to_string(),
            ));
        }
        Ok(event)
    }

    pub async fn shutdown(mut self) -> Result<(), AiWorkerError> {
        self.stdin.shutdown().await?;
        drop(self.stdin);
        let status = tokio::time::timeout(Duration::from_secs(5), self.child.wait())
            .await
            .map_err(|_| AiWorkerError::Timeout(Duration::from_secs(5)))??;
        if !status.success() {
            return Err(AiWorkerError::Protocol(format!(
                "AI worker exited with {status}"
            )));
        }
        Ok(())
    }

    async fn write(&mut self, request: &Value) -> Result<(), AiWorkerError> {
        let mut encoded = serde_json::to_vec(request)
            .map_err(|error| AiWorkerError::Protocol(error.to_string()))?;
        encoded.push(b'\n');
        self.stdin.write_all(&encoded).await?;
        self.stdin.flush().await?;
        Ok(())
    }
}

fn current_traceparent() -> Option<String> {
    let mut headers = axum::http::HeaderMap::new();
    crate::telemetry::inject_current_context(&mut headers);
    headers
        .get("traceparent")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}
