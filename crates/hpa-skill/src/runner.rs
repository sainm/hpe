use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use uuid::Uuid;

use crate::error::{SkillError, SkillResult};
use crate::loader::{hash_bytes, safe_relative_path};
use crate::manifest::{
    EntryKind, SkillPermissions, SkillScript, SkillSnapshot, allowed_license, denied_license,
};

const STDERR_LIMIT: u64 = 256 * 1024;
const MAX_INPUT_BYTES: usize = 1024 * 1024;
const SAFE_ENV: &[&str] = &["LANG", "LC_ALL", "TZ", "NO_COLOR"];

#[derive(Debug, Clone)]
pub struct RunnerConfig {
    pub work_root: PathBuf,
    pub artifact_store: PathBuf,
    pub audit_log: Option<PathBuf>,
}

#[derive(Debug)]
pub struct SkillRunRequest<'a> {
    pub snapshot: &'a SkillSnapshot,
    pub entry: EntryKind,
    pub permissions: &'a SkillPermissions,
    pub scripts: &'a [SkillScript],
    pub script_id: String,
    pub input: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Succeeded,
    Failed,
    TimedOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxKind {
    MacosSeatbelt,
    LinuxBubblewrap,
    Process,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunFailure {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRecord {
    pub id: String,
    pub name: String,
    pub sha256: String,
    pub size: u64,
    pub mime_type: String,
    #[serde(skip_serializing)]
    pub stored_path: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunOutcome {
    pub id: String,
    pub skill_id: String,
    pub version: String,
    pub script_id: String,
    pub status: RunStatus,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub output: Option<Value>,
    pub error: Option<RunFailure>,
    pub artifacts: Vec<ArtifactRecord>,
    pub duration_ms: u64,
    pub sandbox: SandboxKind,
    pub started_at: String,
    pub finished_at: String,
}

impl RunOutcome {
    pub fn succeeded(&self) -> bool {
        self.status == RunStatus::Succeeded
    }
}

struct PreparedCommand {
    program: PathBuf,
    args: Vec<String>,
    runtime: PathBuf,
    sandbox: SandboxKind,
}

pub async fn run_script(
    config: &RunnerConfig,
    request: SkillRunRequest<'_>,
) -> SkillResult<RunOutcome> {
    if !request.entry.deterministic() {
        return Err(SkillError::Permission(
            "skill entry kind is `instructions`; deterministic execution requires `script` or `both`"
                .to_string(),
        ));
    }
    let script = request
        .scripts
        .iter()
        .find(|script| script.id == request.script_id)
        .cloned()
        .ok_or_else(|| SkillError::UnknownScript(request.script_id.clone()))?;
    validate_policy(&script, request.permissions)?;
    if let Some(schema) = &script.input_schema {
        validate_schema_input(schema, &request.input)?;
    }
    let input_bytes = serde_json::to_vec_pretty(&request.input)?;
    if input_bytes.len() > MAX_INPUT_BYTES {
        return Err(SkillError::invalid(
            "SKILL_INPUT_TOO_LARGE",
            format!(
                "input is {} bytes; the limit is {MAX_INPUT_BYTES}",
                input_bytes.len()
            ),
        ));
    }
    let runtime = resolve_runtime(script.runtime())?;
    let resources = request.permissions.resources();

    let run_id = format!("skillrun_{}", Uuid::now_v7());
    let started = OffsetDateTime::now_utc();
    let started_clock = Instant::now();
    let workdir = config.work_root.join(&run_id);
    prepare_workdir(&workdir, request.snapshot)?;
    let input_file = workdir.join("input/input.json");
    std::fs::write(&input_file, &input_bytes)?;
    set_read_only(&input_file)?;

    let outcome = run_inner(
        config,
        &request,
        &script,
        &run_id,
        &workdir,
        &runtime,
        &input_file,
        &input_bytes,
        resources.output_bytes(),
        resources.cpu_seconds(),
        resources.memory_mb(),
        resources.artifact_bytes(),
        started,
        started_clock,
    )
    .await;

    let _ = tokio::fs::remove_dir_all(&workdir).await;
    let mut outcome = outcome?;
    outcome.finished_at = now_rfc3339();
    outcome.duration_ms = started_clock.elapsed().as_millis() as u64;
    if let Some(audit) = &config.audit_log {
        let _ = append_audit(audit, &outcome).await;
    }
    Ok(outcome)
}

#[allow(clippy::too_many_arguments)]
async fn run_inner(
    config: &RunnerConfig,
    request: &SkillRunRequest<'_>,
    script: &SkillScript,
    run_id: &str,
    workdir: &Path,
    runtime: &Path,
    input_file: &Path,
    input_bytes: &[u8],
    output_bytes: u64,
    cpu_seconds: u64,
    memory_mb: u64,
    artifact_bytes: u64,
    started: OffsetDateTime,
    started_clock: Instant,
) -> SkillResult<RunOutcome> {
    let skill_dir = workdir.join("skill");
    let artifact_dir = workdir.join("artifacts");
    let temp_dir = workdir.join("tmp");
    let home_dir = workdir.join("home");
    let script_file = skill_dir.join(&script.path);
    let prepared = prepare_command(
        runtime,
        script,
        workdir,
        &skill_dir,
        &artifact_dir,
        input_file,
        &script_file,
    )?;
    let mut command = Command::from(build_std_command(
        &prepared,
        script,
        request.permissions,
        workdir,
        &skill_dir,
        &artifact_dir,
        &temp_dir,
        &home_dir,
        input_file,
        cpu_seconds,
        memory_mb,
        artifact_bytes,
    )?);
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.kill_on_drop(true);

    let mut child = command.spawn().map_err(|error| {
        SkillError::SandboxUnavailable(format!(
            "cannot start sandboxed runtime `{}`: {error}",
            runtime.display()
        ))
    })?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = tokio::time::timeout(Duration::from_secs(5), async {
            let _ = stdin.write_all(input_bytes).await;
            let _ = stdin.shutdown().await;
        })
        .await;
    }
    let stdout = child.stdout.take().ok_or_else(|| {
        SkillError::SandboxUnavailable("sandboxed process has no stdout pipe".to_string())
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        SkillError::SandboxUnavailable("sandboxed process has no stderr pipe".to_string())
    })?;
    let stdout_task = tokio::spawn(read_capped(stdout, output_bytes));
    let stderr_task = tokio::spawn(read_capped(stderr, STDERR_LIMIT));
    let timeout = Duration::from_millis(script.timeout_ms());
    let wait = tokio::time::timeout(timeout, child.wait()).await;
    let (status, timed_out) = match wait {
        Ok(result) => (Some(result?), false),
        Err(_) => {
            kill_process_group(&mut child);
            let _ = child.wait().await;
            (None, true)
        }
    };
    let (stdout_bytes, stdout_exceeded) = stdout_task.await.unwrap_or_default();
    let (stderr_bytes, _stderr_exceeded) = stderr_task.await.unwrap_or_default();
    let stdout = String::from_utf8_lossy(&stdout_bytes).to_string();
    let stderr = String::from_utf8_lossy(&stderr_bytes).to_string();
    let exit_code = status.as_ref().and_then(std::process::ExitStatus::code);
    let output = serde_json::from_str::<Value>(stdout.trim()).ok();

    let mut failure = None;
    let run_status = if timed_out {
        failure = Some(RunFailure {
            code: "SCRIPT_TIMEOUT".to_string(),
            message: format!("script exceeded {} ms", script.timeout_ms()),
        });
        RunStatus::TimedOut
    } else if stdout_exceeded {
        failure = Some(RunFailure {
            code: "SCRIPT_OUTPUT_LIMIT".to_string(),
            message: format!("stdout exceeded {output_bytes} bytes"),
        });
        RunStatus::Failed
    } else if exit_code != Some(0) {
        failure = Some(RunFailure {
            code: "SCRIPT_EXIT".to_string(),
            message: format!(
                "script exited with {}{}",
                exit_code
                    .map(|code| format!("code {code}"))
                    .unwrap_or_else(|| "a signal".to_string()),
                if stderr.trim().is_empty() {
                    String::new()
                } else {
                    format!(": {}", first_lines(&stderr, 4))
                }
            ),
        });
        RunStatus::Failed
    } else if let Some(schema) = &script.output_schema {
        match &output {
            None => {
                failure = Some(RunFailure {
                    code: "SCRIPT_OUTPUT_INVALID".to_string(),
                    message: "stdout is not valid JSON".to_string(),
                });
                RunStatus::Failed
            }
            Some(output) => match jsonschema::validator_for(schema) {
                Ok(validator) if validator.is_valid(output) => RunStatus::Succeeded,
                Ok(_) => {
                    failure = Some(RunFailure {
                        code: "SCRIPT_OUTPUT_INVALID".to_string(),
                        message: "stdout does not match outputSchema".to_string(),
                    });
                    RunStatus::Failed
                }
                Err(error) => {
                    return Err(SkillError::invalid(
                        "SKILL_SCHEMA",
                        format!("outputSchema is invalid: {error}"),
                    ));
                }
            },
        }
    } else {
        RunStatus::Succeeded
    };

    let artifacts = collect_artifacts(&artifact_dir, &config.artifact_store, artifact_bytes)?;
    let finished = OffsetDateTime::now_utc();
    Ok(RunOutcome {
        id: run_id.to_string(),
        skill_id: request.snapshot.id.clone(),
        version: request.snapshot.version.clone(),
        script_id: script.id.clone(),
        status: run_status,
        exit_code,
        stdout,
        stderr,
        output,
        error: failure,
        artifacts,
        duration_ms: started_clock.elapsed().as_millis() as u64,
        sandbox: prepared.sandbox,
        started_at: started.format(&Rfc3339).unwrap_or_default(),
        finished_at: finished.format(&Rfc3339).unwrap_or_default(),
    })
}

#[allow(clippy::too_many_arguments)]
fn build_std_command(
    prepared: &PreparedCommand,
    script: &SkillScript,
    permissions: &SkillPermissions,
    workdir: &Path,
    skill_dir: &Path,
    artifact_dir: &Path,
    temp_dir: &Path,
    home_dir: &Path,
    input_file: &Path,
    cpu_seconds: u64,
    memory_mb: u64,
    artifact_bytes: u64,
) -> SkillResult<std::process::Command> {
    let mut command = std::process::Command::new(&prepared.program);
    command.args(&prepared.args);
    command.env_clear();
    let runtime_dir = prepared
        .runtime
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("/usr/bin"));
    let path = format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", runtime_dir.display());
    command.env("PATH", path);
    command.env("HOME", home_dir);
    command.env("TMPDIR", temp_dir);
    command.env("LANG", "C.UTF-8");
    command.env("LC_ALL", "C.UTF-8");
    command.env("HPA_WORK_DIR", workdir);
    command.env("HPA_SKILL_DIR", skill_dir);
    command.env("HPA_INPUT_FILE", input_file);
    command.env("HPA_INPUT_DIR", workdir.join("input"));
    command.env("HPA_ARTIFACT_DIR", artifact_dir);
    for (key, value) in &script.env {
        if !SAFE_ENV.contains(&key.as_str()) || !permissions.env.iter().any(|name| name == key) {
            return Err(SkillError::Permission(format!(
                "environment variable `{key}` is not declared in permissions.env"
            )));
        }
        command.env(key, value);
    }
    for key in SAFE_ENV {
        if let Ok(value) = std::env::var(key)
            && value.len() <= 1024
            && !value.contains('\0')
            && permissions.env.iter().any(|name| name == key)
        {
            command.env(key, value);
        }
    }
    command.current_dir(workdir);
    command.stdin(Stdio::null());
    command.stdout(Stdio::null());
    command.stderr(Stdio::null());
    command.process_group(0);

    #[cfg(not(target_os = "linux"))]
    let _ = memory_mb;
    #[cfg(unix)]
    unsafe {
        command.pre_exec(move || {
            let cpu = libc::rlimit {
                rlim_cur: cpu_seconds,
                rlim_max: cpu_seconds,
            };
            libc::setrlimit(libc::RLIMIT_CPU, &cpu);
            let file_size = libc::rlimit {
                rlim_cur: artifact_bytes,
                rlim_max: artifact_bytes,
            };
            libc::setrlimit(libc::RLIMIT_FSIZE, &file_size);
            let files = libc::rlimit {
                rlim_cur: 128,
                rlim_max: 128,
            };
            libc::setrlimit(libc::RLIMIT_NOFILE, &files);
            #[cfg(target_os = "linux")]
            {
                let memory = libc::rlimit {
                    rlim_cur: memory_mb * 1024 * 1024,
                    rlim_max: memory_mb * 1024 * 1024,
                };
                libc::setrlimit(libc::RLIMIT_AS, &memory);
            }
            Ok(())
        });
    }
    Ok(command)
}

fn prepare_command(
    runtime: &Path,
    script: &SkillScript,
    workdir: &Path,
    skill_dir: &Path,
    artifact_dir: &Path,
    input_file: &Path,
    script_file: &Path,
) -> SkillResult<PreparedCommand> {
    if !script_file.is_file() {
        return Err(SkillError::invalid(
            "SKILL_SCRIPT_MISSING",
            format!("script file `{}` is missing", script.path),
        ));
    }
    let mut args = Vec::new();
    for arg in &script.args {
        args.push(substitute_arg(
            arg,
            workdir,
            skill_dir,
            artifact_dir,
            input_file,
        )?);
    }
    let mut base = vec![script_file.display().to_string()];
    base.extend(args);
    sandbox_command(runtime, base, workdir)
}

fn substitute_arg(
    arg: &str,
    workdir: &Path,
    skill_dir: &Path,
    artifact_dir: &Path,
    input_file: &Path,
) -> SkillResult<String> {
    let mut value = arg.to_string();
    let replacements = [
        ("${workDir}", workdir.display().to_string()),
        ("${skillDir}", skill_dir.display().to_string()),
        ("${artifactDir}", artifact_dir.display().to_string()),
        ("${inputFile}", input_file.display().to_string()),
        (
            "${inputDir}",
            input_file
                .parent()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
        ),
    ];
    for (placeholder, replacement) in replacements {
        value = value.replace(placeholder, &replacement);
    }
    if value.contains("${") {
        return Err(SkillError::Permission(format!(
            "unsupported placeholder in script argument `{arg}`"
        )));
    }
    Ok(value)
}

/// Explicit escape hatch for environments that cannot nest a sandbox (containers
/// without `bwrap`, CI images where `sandbox-exec` is denied). Isolation is then
/// reported as `process` in the run result so audits can see the degraded mode.
fn sandbox_disabled_by_env() -> bool {
    static WARNED: std::sync::Once = std::sync::Once::new();
    let disabled = std::env::var("HPA_SKILL_ALLOW_UNSANDBOXED")
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes"))
        .unwrap_or(false);
    if disabled {
        WARNED.call_once(|| {
            tracing::warn!(
                "HPA_SKILL_ALLOW_UNSANDBOXED is set: skill scripts run without a sandbox"
            );
        });
    }
    disabled
}

fn sandbox_command(
    runtime: &Path,
    mut args: Vec<String>,
    workdir: &Path,
) -> SkillResult<PreparedCommand> {
    if sandbox_disabled_by_env() {
        let mut full = vec![runtime.display().to_string()];
        full.append(&mut args);
        return Ok(PreparedCommand {
            program: runtime.to_path_buf(),
            args: full.into_iter().skip(1).collect(),
            runtime: runtime.to_path_buf(),
            sandbox: SandboxKind::Process,
        });
    }
    #[cfg(target_os = "macos")]
    {
        let sandbox = Path::new("/usr/bin/sandbox-exec");
        if sandbox.is_file() {
            let profile = seatbelt_profile(workdir);
            let mut sandbox_args = vec!["-p".to_string(), profile, runtime.display().to_string()];
            sandbox_args.append(&mut args);
            return Ok(PreparedCommand {
                program: sandbox.to_path_buf(),
                args: sandbox_args,
                runtime: runtime.to_path_buf(),
                sandbox: SandboxKind::MacosSeatbelt,
            });
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(bwrap) = find_executable("bwrap") {
            let mut sandbox_args = vec![
                "--unshare-all".to_string(),
                "--die-with-parent".to_string(),
                "--ro-bind".to_string(),
                "/".to_string(),
                "/".to_string(),
                "--bind".to_string(),
                workdir.display().to_string(),
                workdir.display().to_string(),
                "--chdir".to_string(),
                workdir.display().to_string(),
                "--dev".to_string(),
                "/dev".to_string(),
                "--proc".to_string(),
                "/proc".to_string(),
                runtime.display().to_string(),
            ];
            sandbox_args.append(&mut args);
            return Ok(PreparedCommand {
                program: bwrap,
                args: sandbox_args,
                runtime: runtime.to_path_buf(),
                sandbox: SandboxKind::LinuxBubblewrap,
            });
        }
    }
    let mut full = vec![runtime.display().to_string()];
    full.append(&mut args);
    Ok(PreparedCommand {
        program: runtime.to_path_buf(),
        args: full.into_iter().skip(1).collect(),
        runtime: runtime.to_path_buf(),
        sandbox: SandboxKind::Process,
    })
}

#[cfg(target_os = "macos")]
fn seatbelt_profile(workdir: &Path) -> String {
    let workdir = workdir
        .canonicalize()
        .unwrap_or_else(|_| workdir.to_path_buf());
    let workdir = escape_sandbox_path(&workdir.display().to_string());
    format!(
        "(version 1)\n(allow default)\n(deny network*)\n(deny file-write*)\n(allow file-write* (subpath \"{workdir}\"))\n(deny file-read* (subpath \"/Users\") (subpath \"/Volumes\") (subpath \"/private/etc/ssh\") (subpath \"/private/var/root\"))\n(allow file-read* (subpath \"{workdir}\"))\n"
    )
}

#[cfg(target_os = "macos")]
fn escape_sandbox_path(path: &str) -> String {
    path.replace('\\', "\\\\").replace('"', "\\\"")
}

fn validate_policy(script: &SkillScript, permissions: &SkillPermissions) -> SkillResult<()> {
    if script.network() != "none" {
        return Err(SkillError::Permission(
            "network access is disabled for local skill scripts".to_string(),
        ));
    }
    if !permissions.network_rules().is_empty() {
        return Err(SkillError::Permission(
            "network permissions are not supported by the local sandbox".to_string(),
        ));
    }
    if !permissions.commands.is_empty() {
        let runtime = script.runtime();
        let allowed = permissions
            .commands
            .iter()
            .any(|command| command == runtime || (runtime == "python3" && command == "python"));
        if !allowed {
            return Err(SkillError::Permission(format!(
                "runtime `{runtime}` is not listed in permissions.commands"
            )));
        }
    }
    Ok(())
}

fn validate_schema_input(schema: &Value, input: &Value) -> SkillResult<()> {
    let validator = jsonschema::validator_for(schema)
        .map_err(|error| SkillError::invalid("SKILL_SCHEMA", format!("inputSchema: {error}")))?;
    if validator.is_valid(input) {
        return Ok(());
    }
    let details = validator
        .iter_errors(input)
        .take(3)
        .map(|error| format!("{}: {error}", error.instance_path()))
        .collect::<Vec<_>>()
        .join("; ");
    Err(SkillError::invalid(
        "SKILL_INPUT_INVALID",
        format!("input does not match inputSchema: {details}"),
    ))
}

fn resolve_runtime(runtime: &str) -> SkillResult<PathBuf> {
    let candidates: &[&str] = match runtime {
        "python3" => &[
            "/usr/bin/python3",
            "/opt/homebrew/bin/python3",
            "/usr/local/bin/python3",
        ],
        "python" => &[
            "/usr/bin/python3",
            "/opt/homebrew/bin/python3",
            "/usr/bin/python",
        ],
        "node" => &[
            "/opt/homebrew/bin/node",
            "/usr/local/bin/node",
            "/usr/bin/node",
        ],
        "bash" => &["/bin/bash"],
        "sh" => &["/bin/sh"],
        _ => return Err(SkillError::RuntimeMissing(runtime.to_string())),
    };
    for candidate in candidates {
        let path = Path::new(candidate);
        if is_executable(path) {
            return Ok(path.to_path_buf());
        }
    }
    if let Some(path) = find_executable(runtime) {
        return Ok(path);
    }
    Err(SkillError::RuntimeMissing(runtime.to_string()))
}

fn find_executable(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        let path = PathBuf::from(name);
        return is_executable(&path).then_some(path);
    }
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(name);
        if is_executable(&candidate) {
            return candidate.canonicalize().ok();
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

fn prepare_workdir(workdir: &Path, snapshot: &SkillSnapshot) -> SkillResult<()> {
    std::fs::create_dir_all(workdir.join("skill"))?;
    std::fs::create_dir_all(workdir.join("input"))?;
    std::fs::create_dir_all(workdir.join("artifacts"))?;
    std::fs::create_dir_all(workdir.join("tmp"))?;
    std::fs::create_dir_all(workdir.join("home"))?;
    for file in &snapshot.files {
        if !safe_relative_path(&file.path) {
            return Err(SkillError::invalid(
                "SKILL_PATH",
                format!("unsafe snapshot path `{}`", file.path),
            ));
        }
        let destination = workdir.join("skill").join(&file.path);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&destination, &file.content)?;
    }
    Ok(())
}

fn set_read_only(path: &Path) -> SkillResult<()> {
    let metadata = std::fs::metadata(path)?;
    let mut permissions = metadata.permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

async fn read_capped<R: AsyncRead + Unpin>(mut reader: R, limit: u64) -> (Vec<u8>, bool) {
    let mut output = Vec::new();
    let mut buffer = [0u8; 16 * 1024];
    let mut exceeded = false;
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(read) => {
                let remaining = limit.saturating_sub(output.len() as u64);
                if remaining == 0 {
                    exceeded = true;
                    break;
                }
                let take = read.min(remaining as usize);
                output.extend_from_slice(&buffer[..take]);
                if take < read {
                    exceeded = true;
                    break;
                }
            }
            Err(_) => break,
        }
    }
    (output, exceeded)
}

fn kill_process_group(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.start_kill();
    }
}

fn collect_artifacts(
    directory: &Path,
    store: &Path,
    limit_bytes: u64,
) -> SkillResult<Vec<ArtifactRecord>> {
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    std::fs::create_dir_all(store)?;
    let mut files = Vec::new();
    collect_artifact_paths(directory, "", 0, &mut files)?;
    files.sort();
    let mut records = Vec::new();
    let mut total = 0u64;
    for relative in files {
        if records.len() >= crate::manifest::MAX_ARTIFACT_FILES {
            return Err(SkillError::invalid(
                "SKILL_ARTIFACT_LIMIT",
                "artifact count exceeds the local limit",
            ));
        }
        let path = directory.join(&relative);
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            continue;
        }
        let size = metadata.len();
        total = total.saturating_add(size);
        if total > limit_bytes {
            return Err(SkillError::invalid(
                "SKILL_ARTIFACT_LIMIT",
                format!("artifacts exceed {limit_bytes} bytes"),
            ));
        }
        let content = std::fs::read(&path)?;
        let sha256 = hash_bytes(&content);
        let stored_path = store.join(&sha256);
        if !stored_path.exists() {
            std::fs::write(&stored_path, &content)?;
        }
        records.push(ArtifactRecord {
            id: format!("artifact_{}", Uuid::now_v7()),
            name: relative.clone(),
            sha256,
            size,
            mime_type: mime_type(&relative).to_string(),
            stored_path,
        });
    }
    Ok(records)
}

fn collect_artifact_paths(
    root: &Path,
    prefix: &str,
    depth: usize,
    files: &mut Vec<String>,
) -> SkillResult<()> {
    if depth > 8 {
        return Ok(());
    }
    let mut entries = std::fs::read_dir(root)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == ".DS_Store" {
            continue;
        }
        let relative = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let metadata = std::fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            collect_artifact_paths(&entry.path(), &relative, depth + 1, files)?;
        } else if metadata.is_file() && safe_relative_path(&relative) {
            files.push(relative);
        }
    }
    Ok(())
}

fn mime_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or_default() {
        "json" => "application/json",
        "md" | "txt" => "text/plain",
        "csv" => "text/csv",
        "html" => "text/html",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

fn first_lines(text: &str, count: usize) -> String {
    text.lines().take(count).collect::<Vec<_>>().join(" ")
}

fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default()
}

async fn append_audit(path: &Path, outcome: &RunOutcome) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let entry = serde_json::json!({
        "at": outcome.finished_at,
        "action": "skill.run",
        "runId": outcome.id,
        "skillId": outcome.skill_id,
        "version": outcome.version,
        "script": outcome.script_id,
        "status": outcome.status,
        "exitCode": outcome.exit_code,
        "sandbox": outcome.sandbox,
        "artifactCount": outcome.artifacts.len(),
        "error": outcome.error.as_ref().map(|error| error.code.clone()),
    });
    let mut line = serde_json::to_string(&entry)?;
    line.push('\n');
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    file.write_all(line.as_bytes()).await
}

pub fn export_license_allowed(license: Option<&str>) -> bool {
    license.is_some_and(|license| allowed_license(license) && !denied_license(license))
}

pub fn detected_sandbox() -> SandboxKind {
    #[cfg(target_os = "macos")]
    if Path::new("/usr/bin/sandbox-exec").is_file() {
        return SandboxKind::MacosSeatbelt;
    }
    #[cfg(target_os = "linux")]
    if find_executable("bwrap").is_some() {
        return SandboxKind::LinuxBubblewrap;
    }
    SandboxKind::Process
}

pub fn runtime_versions() -> BTreeMap<String, Option<String>> {
    let mut runtimes = BTreeMap::new();
    for runtime in ["python3", "node", "bash"] {
        let version = resolve_runtime(runtime).ok().and_then(|path| {
            std::process::Command::new(path)
                .arg("--version")
                .output()
                .ok()
                .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        });
        runtimes.insert(runtime.to_string(), version);
    }
    runtimes
}
