pub mod docker;
#[cfg(feature = "firecracker")]
pub mod firecracker;
pub mod js;

pub use js::{JsExecutionError, JsExecutionService};

use crate::service::docker::config::{SandboxConfig, DOCKER_UV_PYTHON_INSTALL_DIR};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;
use tempfile::NamedTempFile;
use thiserror::Error;
use tracing::debug;
use wait_timeout::ChildExt;

#[derive(Debug, Clone)]
pub struct ExecutionResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

#[derive(Error, Debug)]
pub enum PythonExecutionError {
    #[error("uv is not installed or not in PATH. Please install uv first.")]
    UvNotFound,

    #[error("Script not found: {0}")]
    ScriptNotFound(String),

    #[error("Execution failed: {0}")]
    ExecutionFailed(String),

    #[error("IO error: {0}")]
    IoError(String),

    #[error("Execution timed out after {0} seconds")]
    ExecutionTimedOut(u64),

    #[error("Output exceeded maximum size of {0} bytes")]
    OutputTruncated(usize),
}

pub struct PythonExecutionService;

impl PythonExecutionService {
    pub fn new() -> Self {
        Self
    }

    /// Minimal PATH for the child: system dirs plus common install locations for `uv`
    /// (`~/.local/bin`, `~/.cargo/bin`). Uses the **current** process `HOME` only to build
    /// these paths (child still gets `HOME=/tmp` and does not inherit host env).
    fn sandbox_path_for_local_exec() -> String {
        let mut parts = vec![
            "/usr/local/bin".to_string(),
            "/usr/bin".to_string(),
            "/bin".to_string(),
        ];
        if let Ok(home) = std::env::var("HOME") {
            if !home.is_empty() {
                parts.push(format!("{}/.local/bin", home));
                parts.push(format!("{}/.cargo/bin", home));
            }
        }
        #[cfg(target_os = "macos")]
        parts.push("/opt/homebrew/bin".to_string());
        parts.join(":")
    }

    /// PATH for the child: [`sandbox_path_for_local_exec`] first, then the **parent** process
    /// `PATH`. This keeps `env_clear()` for secrets while still finding `uv` when it lives only
    /// in nix/asdf/custom profile paths (not under `~/.local/bin` etc.).
    fn path_for_local_uv_child() -> String {
        let base = Self::sandbox_path_for_local_exec();
        match std::env::var("PATH") {
            Ok(p) if !p.is_empty() => format!("{base}:{p}"),
            _ => base,
        }
    }

    /// Whether `uv` resolves using the same PATH we pass to the real `uv run` spawn.
    fn uv_available_under_sandbox_path() -> bool {
        Command::new("uv")
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .env_clear()
            .env("PATH", Self::path_for_local_uv_child())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn apply_sandbox_command(cmd: &mut Command, limits: &SandboxConfig) {
        let in_image = std::env::var_os("SANDY_IN_DOCKER").is_some()
            || std::env::var_os("SANDY_IN_FIRECRACKER").is_some();
        cmd.stdin(Stdio::null());
        cmd.env_clear();
        cmd.env("PATH", Self::path_for_local_uv_child());
        cmd.env("LANG", "C.UTF-8");
        cmd.env("LC_ALL", "C.UTF-8");
        cmd.env("UV_PYTHON", limits.python_version.as_str());
        if in_image {
            // Writable tmpfs mounts: /workspace (HOME) and /tmp — not noexec.
            // Match Dockerfile `UV_PYTHON_INSTALL_DIR` so `uv` uses the image Python, not a fresh download.
            cmd.env("UV_PYTHON_INSTALL_DIR", DOCKER_UV_PYTHON_INSTALL_DIR);
            cmd.env("HOME", "/workspace");
            cmd.env("XDG_CACHE_HOME", "/workspace/.cache");
            cmd.env("UV_CACHE_DIR", "/workspace/.cache/uv");
        } else {
            cmd.env("HOME", "/tmp");
            cmd.env("XDG_CACHE_HOME", "/tmp/cache");
            cmd.env("UV_CACHE_DIR", "/tmp/cache/uv");
        }
    }

    fn read_lines_capped(
        reader: impl BufRead,
        max_bytes: usize,
    ) -> Result<Vec<String>, PythonExecutionError> {
        let mut lines = Vec::new();
        let mut total = 0usize;
        for line in reader.lines() {
            let line = line.map_err(|e| {
                PythonExecutionError::IoError(format!("Failed to read line: {}", e))
            })?;
            let sep = if lines.is_empty() { 0 } else { 1 };
            if total.saturating_add(sep).saturating_add(line.len()) > max_bytes {
                return Err(PythonExecutionError::OutputTruncated(max_bytes));
            }
            total = total.saturating_add(sep).saturating_add(line.len());
            lines.push(line);
        }
        Ok(lines)
    }

    /// Execute a Python script using uv run
    /// The script can declare dependencies with [PEP 723 inline script metadata](https://docs.astral.sh/uv/guides/scripts/#declaring-script-dependencies) at the top (`# /// script` … `# ///`), e.g. `# dependencies = ["httpx"]` inside that block—not the old `# uv:` comment form.
    pub fn execute_script(
        &self,
        script_path: &Path,
    ) -> Result<ExecutionResult, PythonExecutionError> {
        self.execute_script_with_limits(script_path, &SandboxConfig::default())
    }

    pub fn execute_script_with_limits(
        &self,
        script_path: &Path,
        limits: &SandboxConfig,
    ) -> Result<ExecutionResult, PythonExecutionError> {
        if !script_path.exists() {
            return Err(PythonExecutionError::ScriptNotFound(
                script_path.to_string_lossy().to_string(),
            ));
        }

        if !Self::uv_available_under_sandbox_path() {
            return Err(PythonExecutionError::UvNotFound);
        }

        let mut cmd = Command::new("uv");
        cmd.arg("run");
        cmd.arg("--quiet");
        cmd.arg(script_path);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        Self::apply_sandbox_command(&mut cmd, limits);

        let mut child = cmd.spawn().map_err(|e| {
            PythonExecutionError::ExecutionFailed(format!("Failed to spawn uv process: {}", e))
        })?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| PythonExecutionError::IoError("Failed to capture stdout".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| PythonExecutionError::IoError("Failed to capture stderr".to_string()))?;

        let stdout_reader = BufReader::new(stdout);
        let stderr_reader = BufReader::new(stderr);

        let stdout_lines = match Self::read_lines_capped(stdout_reader, limits.max_output_bytes) {
            Ok(l) => l,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait_timeout(Duration::from_secs(1));
                return Err(e);
            }
        };
        let stderr_lines = match Self::read_lines_capped(stderr_reader, limits.max_output_bytes) {
            Ok(l) => l,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait_timeout(Duration::from_secs(1));
                return Err(e);
            }
        };

        let status = match child.wait_timeout(Duration::from_secs(limits.timeout_secs)) {
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait_timeout(Duration::from_secs(1));
                return Err(PythonExecutionError::ExecutionTimedOut(limits.timeout_secs));
            }
            Ok(Some(s)) => s,
            Err(e) => {
                let _ = child.kill();
                return Err(PythonExecutionError::ExecutionFailed(format!(
                    "Failed to wait for process: {}",
                    e
                )));
            }
        };

        let exit_code = status.code().unwrap_or(-1);

        Ok(ExecutionResult {
            stdout: stdout_lines.join("\n"),
            stderr: stderr_lines.join("\n"),
            exit_code,
        })
    }

    /// Execute Python code from a string using uv run
    /// The code can include [PEP 723 inline script metadata](https://docs.astral.sh/uv/guides/scripts/#declaring-script-dependencies) (`# /// script` … `# ///`) so `uv run` installs packages before executing the temp `.py` file.
    /// A temporary file is created, executed, and automatically cleaned up
    pub fn execute_code(&self, code: &str) -> Result<ExecutionResult, PythonExecutionError> {
        self.execute_code_with_limits(code, &SandboxConfig::default())
    }

    pub fn execute_code_with_limits(
        &self,
        code: &str,
        limits: &SandboxConfig,
    ) -> Result<ExecutionResult, PythonExecutionError> {
        if !Self::uv_available_under_sandbox_path() {
            return Err(PythonExecutionError::UvNotFound);
        }

        let mut temp_file = NamedTempFile::with_suffix(".py").map_err(|e| {
            PythonExecutionError::IoError(format!("Failed to create temporary file: {}", e))
        })?;

        temp_file.write_all(code.as_bytes()).map_err(|e| {
            PythonExecutionError::IoError(format!("Failed to write code to temporary file: {}", e))
        })?;

        temp_file.flush().map_err(|e| {
            PythonExecutionError::IoError(format!("Failed to flush temporary file: {}", e))
        })?;

        let temp_path = temp_file.into_temp_path();
        let script_path = temp_path.as_ref();

        let result = self.execute_script_with_limits(script_path, limits)?;

        Ok(result)
    }

    /// Execute Python code from a string with streaming output
    /// Calls the callback for each line of output as it's produced
    /// A temporary file is created, executed, and automatically cleaned up
    pub fn execute_code_streaming(
        &self,
        code: &str,
        on_output: impl FnMut(&str) -> Result<(), Box<dyn std::error::Error>>,
    ) -> Result<ExecutionResult, PythonExecutionError> {
        self.execute_code_streaming_with_limits(code, on_output, &SandboxConfig::default())
    }

    pub fn execute_code_streaming_with_limits(
        &self,
        code: &str,
        on_output: impl FnMut(&str) -> Result<(), Box<dyn std::error::Error>>,
        limits: &SandboxConfig,
    ) -> Result<ExecutionResult, PythonExecutionError> {
        if !Self::uv_available_under_sandbox_path() {
            return Err(PythonExecutionError::UvNotFound);
        }

        let mut temp_file = NamedTempFile::with_suffix(".py").map_err(|e| {
            PythonExecutionError::IoError(format!("Failed to create temporary file: {}", e))
        })?;

        temp_file.write_all(code.as_bytes()).map_err(|e| {
            PythonExecutionError::IoError(format!("Failed to write code to temporary file: {}", e))
        })?;

        temp_file.flush().map_err(|e| {
            PythonExecutionError::IoError(format!("Failed to flush temporary file: {}", e))
        })?;

        let temp_path = temp_file.into_temp_path();
        let script_path = temp_path.as_ref();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(script_path)
                .map_err(|e| {
                    PythonExecutionError::IoError(format!("Failed to get file metadata: {}", e))
                })?
                .permissions();
            perms.set_mode(0o755);
            fs::set_permissions(script_path, perms).map_err(|e| {
                PythonExecutionError::IoError(format!("Failed to set file permissions: {}", e))
            })?;
        }

        let result = self.execute_script_streaming_with_limits(script_path, on_output, limits)?;

        Ok(result)
    }

    /// Execute a Python script with streaming output
    /// Calls the callback for each line of output as it's produced
    pub fn execute_script_streaming(
        &self,
        script_path: &Path,
        on_output: impl FnMut(&str) -> Result<(), Box<dyn std::error::Error>>,
    ) -> Result<ExecutionResult, PythonExecutionError> {
        self.execute_script_streaming_with_limits(script_path, on_output, &SandboxConfig::default())
    }

    pub fn execute_script_streaming_with_limits(
        &self,
        script_path: &Path,
        mut on_output: impl FnMut(&str) -> Result<(), Box<dyn std::error::Error>>,
        limits: &SandboxConfig,
    ) -> Result<ExecutionResult, PythonExecutionError> {
        if !script_path.exists() {
            return Err(PythonExecutionError::ScriptNotFound(
                script_path.to_string_lossy().to_string(),
            ));
        }

        if !Self::uv_available_under_sandbox_path() {
            return Err(PythonExecutionError::UvNotFound);
        }

        let mut cmd = Command::new("uv");
        cmd.arg("run");
        cmd.arg("--quiet");
        cmd.arg(script_path);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        Self::apply_sandbox_command(&mut cmd, limits);

        let mut child = cmd.spawn().map_err(|e| {
            PythonExecutionError::ExecutionFailed(format!("Failed to spawn uv process: {}", e))
        })?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| PythonExecutionError::IoError("Failed to capture stdout".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| PythonExecutionError::IoError("Failed to capture stderr".to_string()))?;

        let stdout_reader = BufReader::new(stdout);
        let mut stdout_lines = Vec::new();
        let mut stdout_bytes = 0usize;
        for line in stdout_reader.lines() {
            let line = line.map_err(|e| {
                PythonExecutionError::IoError(format!("Failed to read stdout: {}", e))
            })?;
            let sep = if stdout_lines.is_empty() { 0 } else { 1 };
            if stdout_bytes.saturating_add(sep).saturating_add(line.len()) > limits.max_output_bytes
            {
                let _ = child.kill();
                let _ = child.wait_timeout(Duration::from_secs(1));
                return Err(PythonExecutionError::OutputTruncated(
                    limits.max_output_bytes,
                ));
            }
            stdout_bytes = stdout_bytes.saturating_add(sep).saturating_add(line.len());
            on_output(&line)
                .map_err(|e| PythonExecutionError::IoError(format!("Callback error: {}", e)))?;
            stdout_lines.push(line);
        }

        let stderr_reader = BufReader::new(stderr);
        let mut stderr_lines = Vec::new();
        let mut stderr_bytes = 0usize;
        for line in stderr_reader.lines() {
            let line = line.map_err(|e| {
                PythonExecutionError::IoError(format!("Failed to read stderr: {}", e))
            })?;
            let sep = if stderr_lines.is_empty() { 0 } else { 1 };
            if stderr_bytes.saturating_add(sep).saturating_add(line.len()) > limits.max_output_bytes
            {
                let _ = child.kill();
                let _ = child.wait_timeout(Duration::from_secs(1));
                return Err(PythonExecutionError::OutputTruncated(
                    limits.max_output_bytes,
                ));
            }
            stderr_bytes = stderr_bytes.saturating_add(sep).saturating_add(line.len());
            // `uv` and Python often use stderr for normal progress; not necessarily errors.
            debug!(target: "sandy::python_stderr", "{}", line);
            stderr_lines.push(line);
        }

        let status = match child.wait_timeout(Duration::from_secs(limits.timeout_secs)) {
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait_timeout(Duration::from_secs(1));
                return Err(PythonExecutionError::ExecutionTimedOut(limits.timeout_secs));
            }
            Ok(Some(s)) => s,
            Err(e) => {
                let _ = child.kill();
                return Err(PythonExecutionError::ExecutionFailed(format!(
                    "Failed to wait for process: {}",
                    e
                )));
            }
        };

        let exit_code = status.code().unwrap_or(-1);

        Ok(ExecutionResult {
            stdout: stdout_lines.join("\n"),
            stderr: stderr_lines.join("\n"),
            exit_code,
        })
    }
}

impl Default for PythonExecutionService {
    fn default() -> Self {
        Self::new()
    }
}
