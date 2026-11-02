#![allow(dead_code)]

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use hpa_core::compiler::compile_workflow;
use hpa_core::ir::Workflow;
use hpa_core::runtime::{
    ExecutionEvent, ExecutionState, ExecutionStatus, StepStatus, cancel_execution, start_event,
    step,
};
use opentelemetry::trace::FutureExt as _;
use reqwest::redirect::Policy;
use reqwest::{Client, StatusCode, Url};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;
use tracing::{Instrument, debug, error, info, warn};

use crate::worker_outbox::WorkerOutbox;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(1);
const MAX_REPORT_RETRIES: usize = 8;
const MAX_RETRY_DELAY_SECS: u64 = 30;
const MAX_RETRY_SLEEP_MS: u64 = 60_000;

type CancelMap = Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>;

#[derive(Clone)]
pub struct WorkerDaemonConfig {
    pub control_url: String,
    pub token: String,
    pub max_concurrency: usize,
    pub capabilities: Vec<String>,
    pub heartbeat_interval: Duration,
    pub poll_interval: Duration,
    pub outbox_path: PathBuf,
}

impl Default for WorkerDaemonConfig {
    fn default() -> Self {
        Self {
            control_url: String::new(),
            token: String::new(),
            max_concurrency: 1,
            capabilities: vec!["workflow".to_string()],
            heartbeat_interval: DEFAULT_HEARTBEAT_INTERVAL,
            poll_interval: DEFAULT_POLL_INTERVAL,
            outbox_path: PathBuf::new(),
        }
    }
}

#[derive(Debug)]
struct WorkerError(String);

impl fmt::Display for WorkerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for WorkerError {}

#[derive(Debug)]
enum ApiError {
    Retryable { message: String },
    LeaseLost { message: String },
    Unauthorized { message: String },
    Rejected { message: String },
    Storage { message: String },
}

impl ApiError {
    fn is_retryable(&self) -> bool {
        matches!(self, Self::Retryable { .. })
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Retryable { message }
            | Self::LeaseLost { message }
            | Self::Unauthorized { message }
            | Self::Rejected { message }
            | Self::Storage { message } => formatter.write_str(message),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HeartbeatRequest {
    version: String,
    capabilities: Vec<String>,
    max_concurrency: usize,
    active_jobs: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HeartbeatResponse {
    #[serde(default)]
    cancel_jobs: Option<Vec<String>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LeaseRequest {
    slots: usize,
    version: String,
    capabilities: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LeaseResponse {
    #[serde(default)]
    jobs: Option<Vec<Value>>,
    #[serde(default)]
    cancel_jobs: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkerJob {
    job_id: String,
    execution_id: String,
    lease_id: String,
    #[serde(default)]
    attempt: Option<u32>,
    #[serde(default)]
    resumed: bool,
    #[serde(default)]
    runtime_version: String,
    #[serde(default)]
    workflow: Value,
    #[serde(default)]
    state: Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JobReport {
    lease_id: String,
    state: Value,
    events: Vec<ExecutionEvent>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JobCompleteReport {
    lease_id: String,
    status: &'static str,
    state: Value,
    events: Vec<ExecutionEvent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Deserialize)]
struct AcceptedResponse {
    #[serde(default = "default_true")]
    accepted: bool,
}

fn default_true() -> bool {
    true
}

struct WorkerClient {
    http: Client,
    base: Url,
    token: String,
    version: String,
    capabilities: Vec<String>,
    max_concurrency: usize,
}

#[derive(Clone)]
struct DurableReporter {
    client: Arc<WorkerClient>,
    outbox: Arc<WorkerOutbox>,
    gate: Arc<Mutex<()>>,
}

impl DurableReporter {
    fn new(client: Arc<WorkerClient>, outbox: Arc<WorkerOutbox>) -> Self {
        Self {
            client,
            outbox,
            gate: Arc::new(Mutex::new(())),
        }
    }

    async fn report<B>(&self, job_id: &str, action: &str, report: &B) -> Result<(), ApiError>
    where
        B: Serialize,
    {
        let value = serde_json::to_value(report).map_err(|error| ApiError::Storage {
            message: format!("cannot serialize {action} report: {error}"),
        })?;
        let lease_id = value
            .get("leaseId")
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::Storage {
                message: format!("{action} report has no leaseId"),
            })?;
        self.outbox
            .enqueue(job_id, lease_id, action, &value)
            .await
            .map_err(|message| ApiError::Storage { message })?;
        self.flush_pending().await
    }

    async fn flush_pending(&self) -> Result<(), ApiError> {
        let _gate = self.gate.lock().await;
        self.flush_locked().await
    }

    async fn flush_locked(&self) -> Result<(), ApiError> {
        loop {
            let entry = self
                .outbox
                .next()
                .await
                .map_err(|message| ApiError::Storage { message })?;
            let Some(entry) = entry else {
                return Ok(());
            };
            if entry.attempts > 0 {
                debug!(
                    job_id = %entry.job_id,
                    action = %entry.action,
                    attempts = entry.attempts,
                    "retrying durable worker report"
                );
            }
            let payload: Value = match serde_json::from_str(&entry.payload_json) {
                Ok(payload) => payload,
                Err(error) => {
                    warn!(
                        job_id = %entry.job_id,
                        action = %entry.action,
                        error = %error,
                        "discarding corrupt durable worker report"
                    );
                    self.outbox
                        .remove(entry.id)
                        .await
                        .map_err(|message| ApiError::Storage { message })?;
                    continue;
                }
            };
            match self
                .client
                .job_report(&entry.job_id, &entry.action, &payload)
                .await
            {
                Ok(()) => {
                    self.outbox
                        .remove(entry.id)
                        .await
                        .map_err(|message| ApiError::Storage { message })?;
                }
                Err(ApiError::Retryable { message }) => {
                    self.outbox
                        .record_failure(entry.id, &message)
                        .await
                        .map_err(|message| ApiError::Storage { message })?;
                    return Err(ApiError::Retryable { message });
                }
                Err(error @ ApiError::LeaseLost { .. })
                | Err(error @ ApiError::Unauthorized { .. })
                | Err(error @ ApiError::Rejected { .. }) => {
                    let discarded = self
                        .outbox
                        .discard_lease(&entry.lease_id)
                        .await
                        .map_err(|message| ApiError::Storage { message })?;
                    warn!(
                        job_id = %entry.job_id,
                        lease_id = %entry.lease_id,
                        action = %entry.action,
                        discarded,
                        error = %error,
                        "discarding worker reports for a rejected lease"
                    );
                    return Err(error);
                }
                Err(error) => return Err(error),
            }
        }
    }
}

impl WorkerClient {
    fn endpoint(&self, segments: &[&str]) -> Url {
        let mut url = self.base.clone();
        if let Ok(mut path) = url.path_segments_mut() {
            path.pop_if_empty();
            for segment in segments {
                path.push(segment);
            }
        }
        url
    }

    async fn post_once<B, R>(&self, url: Url, body: &B) -> Result<R, ApiError>
    where
        B: Serialize + ?Sized,
        R: DeserializeOwned,
    {
        let parent = hpa_local::telemetry::current_context();
        let url_attribute = url.to_string();
        let response = async move {
            let span = tracing::info_span!(
                "http.client",
                otel.kind = "client",
                http.request.method = "POST",
                url.full = %url_attribute,
                http.response.status_code = tracing::field::Empty
            );
            let response_span = span.clone();
            async move {
                let mut trace_headers = axum::http::HeaderMap::new();
                hpa_local::telemetry::inject_current_context(&mut trace_headers);
                let mut request = self.http.post(url).bearer_auth(&self.token).json(body);
                for (name, value) in &trace_headers {
                    request = request.header(name, value);
                }
                let response = request.send().await.map_err(|error| ApiError::Retryable {
                    message: format!("request failed: {error}"),
                })?;
                response_span.record("http.response.status_code", response.status().as_u16());
                Ok::<_, ApiError>(response)
            }
            .instrument(span.clone())
            .await
        }
        .with_context(parent)
        .await?;
        let status = response.status();
        let body = response.text().await.map_err(|error| ApiError::Retryable {
            message: format!("cannot read response body: {error}"),
        })?;
        if status == StatusCode::CONFLICT {
            return Err(ApiError::LeaseLost {
                message: format!("HTTP 409: {}", body_excerpt(&body)),
            });
        }
        if status == StatusCode::UNAUTHORIZED {
            return Err(ApiError::Unauthorized {
                message: format!("HTTP 401: {}", body_excerpt(&body)),
            });
        }
        if !status.is_success() {
            let message = format!("HTTP {}: {}", status.as_u16(), body_excerpt(&body));
            if status.is_server_error()
                || status == StatusCode::REQUEST_TIMEOUT
                || status == StatusCode::TOO_MANY_REQUESTS
            {
                return Err(ApiError::Retryable { message });
            }
            return Err(ApiError::Rejected { message });
        }
        serde_json::from_str::<R>(&body).map_err(|error| ApiError::Retryable {
            message: format!("invalid JSON response: {error}"),
        })
    }

    async fn post_retry<B, R>(&self, url: Url, body: &B, operation: &str) -> Result<R, ApiError>
    where
        B: Serialize + ?Sized,
        R: DeserializeOwned,
    {
        let mut retries = 0usize;
        loop {
            match self.post_once::<B, R>(url.clone(), body).await {
                Ok(value) => return Ok(value),
                Err(error) => {
                    if !error.is_retryable() || retries >= MAX_REPORT_RETRIES {
                        return Err(error);
                    }
                    retries += 1;
                    let delay = retry_delay(retries);
                    warn!(
                        operation = operation,
                        retry = retries,
                        delay_ms = delay.as_millis(),
                        error = %error,
                        "worker request failed; retrying with backoff"
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    async fn heartbeat(&self, active_jobs: Vec<String>) -> Result<HeartbeatResponse, ApiError> {
        let request = HeartbeatRequest {
            version: self.version.clone(),
            capabilities: self.capabilities.clone(),
            max_concurrency: self.max_concurrency,
            active_jobs,
        };
        self.post_once(self.endpoint(&["api", "worker", "heartbeat"]), &request)
            .await
    }

    async fn lease(&self, slots: usize) -> Result<LeaseResponse, ApiError> {
        let request = LeaseRequest {
            slots,
            version: self.version.clone(),
            capabilities: self.capabilities.clone(),
        };
        self.post_once(self.endpoint(&["api", "worker", "lease"]), &request)
            .await
    }

    async fn job_report<B>(&self, job_id: &str, action: &str, report: &B) -> Result<(), ApiError>
    where
        B: Serialize + ?Sized,
    {
        let url = self.endpoint(&["api", "worker", "jobs", job_id, action]);
        let response: AcceptedResponse = self.post_retry(url, report, action).await?;
        if response.accepted {
            Ok(())
        } else {
            Err(ApiError::Rejected {
                message: format!("{action} report was not accepted"),
            })
        }
    }
}

pub async fn run_worker(config: WorkerDaemonConfig) -> Result<(), Box<dyn std::error::Error>> {
    let base = validate_control_url(&config.control_url).map_err(WorkerError)?;
    if config.outbox_path.as_os_str().is_empty() {
        return Err(WorkerError("worker outbox path is required".to_string()).into());
    }
    let http = Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|error| WorkerError(format!("cannot create worker HTTP client: {error}")))?;
    let heartbeat_interval = non_zero(config.heartbeat_interval);
    let poll_interval = non_zero(config.poll_interval);
    let max_concurrency = config.max_concurrency;
    let client = Arc::new(WorkerClient {
        http,
        base: base.clone(),
        token: config.token,
        version: env!("CARGO_PKG_VERSION").to_string(),
        capabilities: config.capabilities,
        max_concurrency,
    });
    let outbox = Arc::new(
        WorkerOutbox::open(&config.outbox_path)
            .await
            .map_err(WorkerError)?,
    );
    let reporter = DurableReporter::new(client.clone(), outbox.clone());
    let active: CancelMap = Arc::new(Mutex::new(HashMap::new()));
    let pending_reports = outbox.pending_count().await.map_err(WorkerError)?;

    info!(
        control_url = %base,
        max_concurrency = max_concurrency,
        outbox = %outbox.path().display(),
        pending_reports,
        "worker daemon starting"
    );

    match reporter.flush_pending().await {
        Ok(()) => {}
        Err(error) => {
            warn!(error = %error, "pending worker reports could not be drained; leasing is paused");
        }
    }

    if let Err(error) = send_heartbeat(&client, &active).await {
        warn!(error = %error, "worker initial heartbeat failed; continuing with backoff");
    }

    let heartbeat_client = client.clone();
    let heartbeat_active = active.clone();
    tokio::spawn(async move {
        let mut retry = 0usize;
        loop {
            let delay = if retry == 0 {
                heartbeat_interval
            } else {
                retry_delay(retry)
            };
            tokio::time::sleep(delay).await;
            match send_heartbeat(&heartbeat_client, &heartbeat_active).await {
                Ok(()) => {
                    retry = 0;
                    debug!("worker heartbeat accepted");
                }
                Err(error) => {
                    retry = retry.saturating_add(1);
                    warn!(retry = retry, error = %error, "worker heartbeat failed; backing off");
                }
            }
        }
    });

    let mut lease_retry = 0usize;
    loop {
        match reporter.flush_pending().await {
            Ok(()) => {}
            Err(error) => {
                let delay = retry_delay(lease_retry.saturating_add(1));
                lease_retry = lease_retry.saturating_add(1);
                warn!(
                    retry = lease_retry,
                    delay_ms = delay.as_millis(),
                    error = %error,
                    "worker outbox is not empty; leasing is paused"
                );
                tokio::time::sleep(delay.min(poll_interval.saturating_mul(5))).await;
                continue;
            }
        }
        let active_jobs = active_job_ids(&active).await.len();
        let slots = max_concurrency.saturating_sub(active_jobs);
        if slots > 0 {
            match client.lease(slots).await {
                Ok(response) => {
                    lease_retry = 0;
                    let cancel_jobs = response.cancel_jobs;
                    let jobs = response.jobs.unwrap_or_default();
                    debug!(count = jobs.len(), slots = slots, "worker lease result");
                    let mut leased = Vec::new();
                    for value in jobs {
                        let job = match serde_json::from_value::<WorkerJob>(value) {
                            Ok(job) => job,
                            Err(error) => {
                                warn!(error = %error, "discarding malformed worker job");
                                continue;
                            }
                        };
                        let job_id = job.job_id.clone();
                        let execution_id = job.execution_id.clone();
                        let flag = {
                            let mut guard = active.lock().await;
                            if guard.len() >= max_concurrency {
                                warn!(
                                    job_id = %job_id,
                                    execution_id = %execution_id,
                                    "worker is at capacity; leaving leased job to expire"
                                );
                                break;
                            }
                            if guard.contains_key(&job_id) {
                                warn!(
                                    job_id = %job_id,
                                    execution_id = %execution_id,
                                    "duplicate leased job ignored"
                                );
                                continue;
                            }
                            let flag = Arc::new(AtomicBool::new(false));
                            guard.insert(job_id.clone(), flag.clone());
                            flag
                        };
                        leased.push((job, flag));
                    }
                    apply_cancel_jobs(&active, cancel_jobs).await;
                    for (job, flag) in leased {
                        info!(
                            job_id = %job.job_id,
                            execution_id = %job.execution_id,
                            attempt = ?job.attempt,
                            "worker leased job"
                        );
                        let job_reporter = reporter.clone();
                        let job_active = active.clone();
                        tokio::spawn(async move {
                            run_job_task(job_reporter, job_active, job, flag).await;
                        });
                    }
                }
                Err(error) => {
                    lease_retry = lease_retry.saturating_add(1);
                    let delay = retry_delay(lease_retry);
                    warn!(
                        retry = lease_retry,
                        delay_ms = delay.as_millis(),
                        error = %error,
                        "worker lease failed; backing off"
                    );
                    tokio::time::sleep(delay).await;
                    continue;
                }
            }
        }
        tokio::time::sleep(poll_interval).await;
    }
}

fn non_zero(duration: Duration) -> Duration {
    if duration.is_zero() {
        Duration::from_millis(1)
    } else {
        duration
    }
}

pub(crate) fn validate_control_url(raw: &str) -> Result<Url, String> {
    let url = Url::parse(raw).map_err(|error| format!("invalid control URL: {error}"))?;
    if !url.username().is_empty() || url.password().is_some() || raw_has_userinfo(raw) {
        return Err("control URL must not contain userinfo".to_string());
    }
    match url.scheme() {
        "http" => {
            let host = url
                .host_str()
                .ok_or_else(|| "control URL must include a host".to_string())?;
            if !is_loopback_host(host) {
                return Err(format!(
                    "plaintext control URL requires a loopback host, got `{host}`"
                ));
            }
        }
        "https" => {
            if url.host_str().is_none() {
                return Err("control URL must include a host".to_string());
            }
        }
        scheme => {
            return Err(format!(
                "unsupported control URL scheme `{scheme}`; use loopback http:// or https://"
            ));
        }
    }
    Ok(url)
}

fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .map(|address| address.is_loopback())
            .unwrap_or(false)
}

fn raw_has_userinfo(raw: &str) -> bool {
    let after_scheme = raw
        .trim()
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(raw);
    after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .contains('@')
}

pub(crate) fn runtime_version_matches(version: &str) -> bool {
    version == env!("CARGO_PKG_VERSION")
}

pub(crate) fn complete_status(status: ExecutionStatus) -> &'static str {
    match status {
        ExecutionStatus::WaitingApproval => "waiting_approval",
        ExecutionStatus::Succeeded => "succeeded",
        ExecutionStatus::Failed => "failed",
        ExecutionStatus::Cancelled => "cancelled",
        ExecutionStatus::Pending | ExecutionStatus::Running => "failed",
    }
}

pub(crate) fn retry_delay(retry: usize) -> Duration {
    let exponent = retry.saturating_sub(1).min(63) as u32;
    let seconds = 1u64
        .checked_shl(exponent)
        .unwrap_or(u64::MAX)
        .min(MAX_RETRY_DELAY_SECS);
    Duration::from_secs(seconds)
}

pub(crate) async fn request_cancel(
    active: &Mutex<HashMap<String, Arc<AtomicBool>>>,
    job_id: &str,
) -> bool {
    let guard = active.lock().await;
    match guard.get(job_id) {
        Some(flag) => {
            flag.store(true, Ordering::SeqCst);
            true
        }
        None => false,
    }
}

async fn send_heartbeat(client: &WorkerClient, active: &CancelMap) -> Result<(), ApiError> {
    let active_jobs = active_job_ids(active).await;
    let response = client.heartbeat(active_jobs).await?;
    apply_cancel_jobs(active, response.cancel_jobs).await;
    Ok(())
}

async fn active_job_ids(active: &CancelMap) -> Vec<String> {
    active.lock().await.keys().cloned().collect()
}

async fn apply_cancel_jobs(active: &CancelMap, jobs: Option<Vec<String>>) {
    for job_id in jobs.unwrap_or_default() {
        if request_cancel(active, &job_id).await {
            info!(job_id = %job_id, "worker cancellation requested");
        }
    }
}

async fn run_job_task(
    reporter: DurableReporter,
    active: CancelMap,
    job: WorkerJob,
    cancel: Arc<AtomicBool>,
) {
    let job_id = job.job_id.clone();
    let execution_id = job.execution_id.clone();
    execute_job(&reporter, &job, &cancel).await;
    active.lock().await.remove(&job_id);
    debug!(
        job_id = %job_id,
        execution_id = %execution_id,
        "worker job task finished"
    );
}

async fn execute_job(reporter: &DurableReporter, job: &WorkerJob, cancel: &AtomicBool) {
    let job_id = job.job_id.as_str();
    let execution_id = job.execution_id.as_str();

    if !runtime_version_matches(&job.runtime_version) {
        error!(
            job_id = %job_id,
            execution_id = %execution_id,
            runtime_version = %job.runtime_version,
            expected = %env!("CARGO_PKG_VERSION"),
            "worker job runtime version does not match"
        );
        complete_failed(
            reporter,
            job,
            job.state.clone(),
            "runtime-version-mismatch".to_string(),
        )
        .await;
        return;
    }

    let workflow = match Workflow::from_value(&job.workflow) {
        Ok(workflow) => workflow,
        Err(error) => {
            let message = format!("workflow-parse-error: {error}");
            error!(
                job_id = %job_id,
                execution_id = %execution_id,
                error = %message,
                "worker job workflow could not be parsed"
            );
            complete_failed(reporter, job, job.state.clone(), message).await;
            return;
        }
    };

    let mut state: ExecutionState = match serde_json::from_value(job.state.clone()) {
        Ok(state) => state,
        Err(error) => {
            let message = format!("state-parse-error: {error}");
            error!(
                job_id = %job_id,
                execution_id = %execution_id,
                error = %message,
                "worker job state could not be parsed"
            );
            complete_failed(reporter, job, job.state.clone(), message).await;
            return;
        }
    };

    if state.id != job.execution_id {
        let message = format!(
            "state-execution-id-mismatch: state id `{}` does not match job execution `{}`",
            state.id, job.execution_id
        );
        error!(
            job_id = %job_id,
            execution_id = %execution_id,
            error = %message,
            "worker job state id does not match execution"
        );
        complete_failed(reporter, job, serialize_state(&state, &job.state), message).await;
        return;
    }

    let plan = match compile_workflow(&workflow) {
        Ok(compilation) => compilation.plan,
        Err(issues) => {
            let message = format!(
                "workflow-invalid: {}",
                issues
                    .iter()
                    .map(|issue| format!("{} {}: {}", issue.code, issue.path, issue.message))
                    .collect::<Vec<_>>()
                    .join("; ")
            );
            error!(
                job_id = %job_id,
                execution_id = %execution_id,
                error = %message,
                "worker job workflow is invalid"
            );
            complete_failed(reporter, job, serialize_state(&state, &job.state), message).await;
            return;
        }
    };

    let mut pending_events = Vec::new();
    if !job.resumed {
        pending_events.push(start_event(&mut state));
    }
    let start_report = JobReport {
        lease_id: job.lease_id.clone(),
        state: serialize_state(&state, &job.state),
        events: pending_events.clone(),
    };
    if let Err(error) = reporter.report(&job.job_id, "start", &start_report).await {
        log_report_failure(job, "start", None, &error);
        return;
    }
    pending_events.clear();
    info!(
        job_id = %job_id,
        execution_id = %execution_id,
        attempt = ?job.attempt,
        resumed = job.resumed,
        "worker job started"
    );

    let mut last_status = state.status;
    loop {
        if cancel.load(Ordering::SeqCst) {
            cancel_state(&mut state, &mut pending_events);
            break;
        }
        if state.status.is_terminal() || state.status == ExecutionStatus::WaitingApproval {
            break;
        }

        let result = step(&workflow, &plan, &mut state).await;
        pending_events.extend(result.events);
        let status_changed = state.status != last_status;
        let final_step =
            state.status.is_terminal() || state.status == ExecutionStatus::WaitingApproval;
        if !final_step && (!pending_events.is_empty() || status_changed) {
            let report = JobReport {
                lease_id: job.lease_id.clone(),
                state: serialize_state(&state, &job.state),
                events: std::mem::take(&mut pending_events),
            };
            if let Err(error) = reporter.report(&job.job_id, "events", &report).await {
                log_report_failure(job, "events", Some(state.status), &error);
                return;
            }
        }
        last_status = state.status;

        if cancel.load(Ordering::SeqCst) {
            cancel_state(&mut state, &mut pending_events);
            break;
        }

        match result.status {
            StepStatus::Continue => {}
            StepStatus::Retry { delay_ms } => {
                let delay = Duration::from_millis(delay_ms.min(MAX_RETRY_SLEEP_MS));
                if cancellable_sleep(delay, cancel).await {
                    continue;
                }
            }
            StepStatus::WaitingApproval => break,
            StepStatus::Succeeded | StepStatus::Failed => break,
        }
    }

    let status = complete_status(state.status);
    let report = JobCompleteReport {
        lease_id: job.lease_id.clone(),
        status,
        state: serialize_state(&state, &job.state),
        events: pending_events,
        error: state.error.as_ref().map(|error| error.message.clone()),
    };
    match reporter.report(&job.job_id, "complete", &report).await {
        Ok(()) => info!(
            job_id = %job_id,
            execution_id = %execution_id,
            status = status,
            "worker job completed"
        ),
        Err(error) => log_report_failure(job, "complete", Some(state.status), &error),
    }
}

fn cancel_state(state: &mut ExecutionState, events: &mut Vec<ExecutionEvent>) {
    if state.status.is_terminal() {
        return;
    }
    match cancel_execution(state) {
        Ok(cancelled) => events.extend(cancelled),
        Err(error) => warn!(error = %error, "worker job cancellation failed"),
    }
}

async fn cancellable_sleep(delay: Duration, cancel: &AtomicBool) -> bool {
    let deadline = tokio::time::Instant::now() + delay;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return true;
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return false;
        }
        let remaining = deadline.saturating_duration_since(now);
        tokio::time::sleep(remaining.min(Duration::from_millis(200))).await;
    }
}

async fn complete_failed(
    reporter: &DurableReporter,
    job: &WorkerJob,
    state: Value,
    message: String,
) {
    let report = JobCompleteReport {
        lease_id: job.lease_id.clone(),
        status: "failed",
        state,
        events: Vec::new(),
        error: Some(message.clone()),
    };
    match reporter.report(&job.job_id, "complete", &report).await {
        Ok(()) => warn!(
            job_id = %job.job_id,
            execution_id = %job.execution_id,
            status = "failed",
            error = %message,
            "worker job failed before execution"
        ),
        Err(error) => log_report_failure(job, "complete", Some(ExecutionStatus::Failed), &error),
    }
}

fn log_report_failure(
    job: &WorkerJob,
    action: &str,
    status: Option<ExecutionStatus>,
    error: &ApiError,
) {
    let status = status.map(complete_status);
    match error {
        ApiError::LeaseLost { .. } => warn!(
            job_id = %job.job_id,
            execution_id = %job.execution_id,
            action = action,
            status = ?status,
            error = %error,
            "worker job lease is no longer valid; abandoning job"
        ),
        ApiError::Unauthorized { .. } => error!(
            job_id = %job.job_id,
            execution_id = %job.execution_id,
            action = action,
            status = ?status,
            error = %error,
            "worker job report unauthorized; stopping job"
        ),
        _ => error!(
            job_id = %job.job_id,
            execution_id = %job.execution_id,
            action = action,
            status = ?status,
            error = %error,
            "worker job report failed; stopping job"
        ),
    }
}

fn serialize_state(state: &ExecutionState, fallback: &Value) -> Value {
    serde_json::to_value(state).unwrap_or_else(|_| fallback.clone())
}

fn body_excerpt(body: &str) -> String {
    let trimmed = body.trim();
    let excerpt: String = trimmed.chars().take(240).collect();
    if trimmed.chars().count() > 240 {
        format!("{excerpt}...")
    } else {
        excerpt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_url_accepts_loopback_http_and_https() {
        for url in [
            "http://127.0.0.1:8787",
            "http://localhost:8787",
            "http://[::1]:8787",
            "https://127.0.0.1:9443",
            "https://worker.example.com",
        ] {
            assert!(validate_control_url(url).is_ok(), "expected valid: {url}");
        }
    }

    #[test]
    fn control_url_rejects_public_http_userinfo_and_other_schemes() {
        for url in [
            "http://worker.example.com:8787",
            "http://0.0.0.0:8787",
            "http://user:secret@127.0.0.1:8787",
            "http://@127.0.0.1:8787",
            "https://user@worker.example.com",
            "ftp://127.0.0.1:21",
            "file:///tmp/hpa",
            "ws://127.0.0.1:8787",
        ] {
            assert!(
                validate_control_url(url).is_err(),
                "expected invalid: {url}"
            );
        }
    }

    #[test]
    fn runtime_version_matches_package_version() {
        assert!(runtime_version_matches(env!("CARGO_PKG_VERSION")));
        assert!(!runtime_version_matches("99.99.99"));
    }

    #[test]
    fn status_maps_to_wire_values() {
        assert_eq!(
            complete_status(ExecutionStatus::WaitingApproval),
            "waiting_approval"
        );
        assert_eq!(complete_status(ExecutionStatus::Succeeded), "succeeded");
        assert_eq!(complete_status(ExecutionStatus::Failed), "failed");
        assert_eq!(complete_status(ExecutionStatus::Cancelled), "cancelled");
        assert_eq!(complete_status(ExecutionStatus::Running), "failed");
    }

    #[test]
    fn retry_delays_grow_exponentially_and_cap() {
        let delays: Vec<u64> = (1..=8).map(|retry| retry_delay(retry).as_secs()).collect();
        assert_eq!(delays, vec![1, 2, 4, 8, 16, 30, 30, 30]);
        assert_eq!(retry_delay(0), Duration::from_secs(1));
        assert_eq!(retry_delay(100), Duration::from_secs(30));
    }

    #[tokio::test]
    async fn cancellation_flags_only_apply_to_active_jobs() {
        let active: Mutex<HashMap<String, Arc<AtomicBool>>> = Mutex::new(HashMap::new());
        let flag = Arc::new(AtomicBool::new(false));
        active
            .lock()
            .await
            .insert("job_1".to_string(), flag.clone());

        assert!(request_cancel(&active, "job_1").await);
        assert!(flag.load(Ordering::SeqCst));
        assert!(!request_cancel(&active, "job_missing").await);
    }
}
