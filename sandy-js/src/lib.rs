use std::path::PathBuf;
use std::time::Duration;

use std::sync::Arc;

use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;
use sandy::{
    bwrap_apparmor_profile as sandy_bwrap_apparmor_profile,
    bwrap_unavailable_reason as sandy_bwrap_unavailable_reason,
    needs_bwrap_apparmor as sandy_needs_bwrap_apparmor,
    reset_bwrap_probe as sandy_reset_bwrap_probe, select_backend, EnvPolicy, ExecBackend,
    ExecPolicy, NetworkPolicy, NoneSandbox, OutputSink, OutputStream, SandboxError,
    SandboxSelection,
};

const MAX_TIMEOUT_SECS: u32 = 600;
const DEFAULT_MAX_OUTPUT_BYTES: usize = 1024 * 1024;
const MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_COMMAND_BYTES: usize = 1024 * 1024;
const MAX_ENV_KEYS: usize = 64;
const MAX_ENV_KEY_BYTES: usize = 256;

#[napi(object)]
pub struct RunShellOptions {
    pub command: String,
    pub workspace: String,
    pub backend: Option<String>,
    pub allow_network: Option<bool>,
    pub env_allowlist: Option<Vec<String>>,
    pub timeout_secs: Option<u32>,
    pub max_output_bytes: Option<u32>,
}

#[napi(object)]
pub struct ShellResult {
    pub backend: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

#[napi(object)]
pub struct BackendInfo {
    pub id: String,
    pub available: bool,
}

fn parse_backend(value: Option<String>) -> Result<ExecBackend> {
    match value
        .as_deref()
        .unwrap_or("auto")
        .to_ascii_lowercase()
        .as_str()
    {
        "auto" => Ok(ExecBackend::Auto),
        "bubblewrap" | "bwrap" => Ok(ExecBackend::Bubblewrap),
        "docker" => Ok(ExecBackend::Docker),
        "firecracker" => {
            #[cfg(feature = "firecracker")]
            {
                Ok(ExecBackend::Firecracker)
            }
            #[cfg(not(feature = "firecracker"))]
            {
                Err(Error::from_reason(
                    "the firecracker backend is not included in this Sandy build",
                ))
            }
        }
        "none" => Ok(ExecBackend::None),
        other => Err(Error::from_reason(format!(
            "unknown sandbox backend `{other}`"
        ))),
    }
}

fn bounded(value: String, limit: usize) -> (String, bool) {
    if value.len() <= limit {
        return (value, false);
    }

    let mut end = limit;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    (value[..end].to_string(), true)
}

fn build_policy(options: &RunShellOptions) -> Result<ExecPolicy> {
    let workspace = PathBuf::from(options.workspace.trim());
    if !workspace.is_absolute() {
        return Err(Error::from_reason("workspace must be an absolute path"));
    }
    let workspace = workspace
        .canonicalize()
        .map_err(|_| Error::from_reason("workspace must be an existing directory"))?;
    if !workspace.is_dir() {
        return Err(Error::from_reason("workspace must be a directory"));
    }

    let env = match options.env_allowlist.clone() {
        Some(allowlist) => EnvPolicy::Clean { allowlist },
        None => EnvPolicy::Clean {
            allowlist: Vec::new(),
        },
    };
    let max_output_bytes = options
        .max_output_bytes
        .map(|value| value as usize)
        .unwrap_or(DEFAULT_MAX_OUTPUT_BYTES);
    Ok(ExecPolicy::workspace(workspace)
        .with_allow_network(options.allow_network.unwrap_or(false))
        .with_env_policy(env)
        .with_max_output_bytes(max_output_bytes))
}

#[napi]
pub fn backend_info() -> Vec<BackendInfo> {
    let backends = [
        ("auto", ExecBackend::Auto),
        ("bubblewrap", ExecBackend::Bubblewrap),
        ("docker", ExecBackend::Docker),
        ("none", ExecBackend::None),
    ];
    backends
        .into_iter()
        .map(|(id, backend)| {
            let selection = SandboxSelection {
                enabled: true,
                backend,
                network: NetworkPolicy::Deny,
                fail_closed: false,
            };
            let selected = select_backend(&selection).unwrap_or_else(|_| Arc::new(NoneSandbox));
            BackendInfo {
                id: id.to_string(),
                available: id == "none"
                    || selected.id() == id
                    || (id == "auto" && selected.id() != "none"),
            }
        })
        .collect()
}

#[napi]
pub fn bwrap_unavailable_reason() -> Option<String> {
    sandy_bwrap_unavailable_reason()
}

#[napi]
pub fn needs_bwrap_apparmor() -> bool {
    sandy_needs_bwrap_apparmor()
}

#[napi]
pub fn reset_bwrap_probe() {
    sandy_reset_bwrap_probe()
}

#[napi]
pub fn bwrap_apparmor_profile() -> String {
    sandy_bwrap_apparmor_profile().to_string()
}

#[napi]
pub async fn run_shell(
    options: RunShellOptions,
    #[napi(ts_arg_type = "((stream: string, chunk: string) => void) | undefined | null")]
    on_output: Option<ThreadsafeFunction<(String, String)>>,
) -> Result<ShellResult> {
    validate_options(&options)?;
    if options.command.trim().is_empty() {
        return Err(Error::from_reason("command must not be empty"));
    }

    let backend = parse_backend(options.backend.clone())?;
    if backend == ExecBackend::None {
        return Err(Error::from_reason(
            "the unsandboxed backend is not available through sandy-js",
        ));
    }

    let timeout_secs = options.timeout_secs.unwrap_or(120);
    if !(1..=MAX_TIMEOUT_SECS).contains(&timeout_secs) {
        return Err(Error::from_reason(format!(
            "timeout_secs must be between 1 and {MAX_TIMEOUT_SECS}"
        )));
    }
    let max_output_bytes = options
        .max_output_bytes
        .map(|value| value as usize)
        .unwrap_or(DEFAULT_MAX_OUTPUT_BYTES);
    if max_output_bytes == 0 {
        return Err(Error::from_reason("max_output_bytes must be positive"));
    }
    if max_output_bytes > MAX_OUTPUT_BYTES {
        return Err(Error::from_reason(format!(
            "max_output_bytes must not exceed {MAX_OUTPUT_BYTES}"
        )));
    }

    let policy = build_policy(&options)?;
    let selected = match select_backend(&SandboxSelection {
        enabled: true,
        backend,
        network: if options.allow_network.unwrap_or(false) {
            NetworkPolicy::Allow
        } else {
            NetworkPolicy::Deny
        },
        fail_closed: true,
    }) {
        Ok(selected) => selected,
        Err(err) => {
            return Err(Error::from_reason(err.to_string()));
        }
    };
    if selected.id() == "none" {
        let backend_name = options.backend.as_deref().unwrap_or("auto");
        let reason = sandy_bwrap_unavailable_reason()
            .unwrap_or_else(|| "no sandbox backend is available".to_string());
        return Err(Error::from_reason(format!(
            "sandbox backend `{backend_name}` is unavailable: {reason}"
        )));
    }

    let backend_id = selected.id().to_string();
    let sink: Option<OutputSink> = on_output.map(|callback| {
        Arc::new(move |stream: OutputStream, bytes: &[u8]| {
            let name = match stream {
                OutputStream::Stdout => "stdout",
                OutputStream::Stderr => "stderr",
            };
            let chunk = String::from_utf8_lossy(bytes).into_owned();
            let _ = callback.call(
                Ok((name.to_string(), chunk)),
                ThreadsafeFunctionCallMode::NonBlocking,
            );
        }) as OutputSink
    });
    let result = selected
        .run_shell_streaming(
            &policy,
            &options.command,
            Duration::from_secs(timeout_secs as u64),
            sink,
        )
        .await
        .map_err(|error| Error::from_reason(safe_error(&error)))?;
    let (stdout, stdout_truncated) = bounded(result.stdout, max_output_bytes);
    let (stderr, stderr_truncated) = bounded(result.stderr, max_output_bytes);

    Ok(ShellResult {
        backend: backend_id,
        stdout,
        stderr,
        exit_code: result.exit_code,
        stdout_truncated,
        stderr_truncated,
    })
}

fn validate_options(options: &RunShellOptions) -> Result<()> {
    if options.command.len() > MAX_COMMAND_BYTES {
        return Err(Error::from_reason(format!(
            "command must not exceed {MAX_COMMAND_BYTES} bytes"
        )));
    }
    if options.workspace.len() > MAX_COMMAND_BYTES {
        return Err(Error::from_reason("workspace path is too long"));
    }
    if let Some(backend) = &options.backend {
        if backend.len() > 32 {
            return Err(Error::from_reason("sandbox backend name is too long"));
        }
    }
    if let Some(allowlist) = &options.env_allowlist {
        if allowlist.len() > MAX_ENV_KEYS {
            return Err(Error::from_reason(format!(
                "env_allowlist must not contain more than {MAX_ENV_KEYS} keys"
            )));
        }
        for key in allowlist {
            if key.is_empty()
                || key.len() > MAX_ENV_KEY_BYTES
                || key.contains('=')
                || key.contains('\0')
            {
                return Err(Error::from_reason("env_allowlist contains an invalid key"));
            }
        }
    }
    if let Some(max_output_bytes) = options.max_output_bytes {
        if max_output_bytes as usize > MAX_OUTPUT_BYTES {
            return Err(Error::from_reason(format!(
                "max_output_bytes must not exceed {MAX_OUTPUT_BYTES}"
            )));
        }
    }
    Ok(())
}

fn safe_error(error: &SandboxError) -> &'static str {
    match error {
        SandboxError::Unavailable(_) => "sandbox backend unavailable",
        SandboxError::SpawnFailed(_) => "sandbox command could not start",
        SandboxError::Io(_) => "sandbox I/O error",
        SandboxError::TimedOut(_) => "sandbox command timed out",
        SandboxError::InvalidPolicy(_) => "invalid sandbox policy",
        SandboxError::OutputLimitExceeded(_) => "sandbox command output exceeded its limit",
    }
}
