use crate::service::docker::config::SandboxConfig;
use crate::service::ExecutionResult;
use serde::Deserialize;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use tempfile::{NamedTempFile, TempDir};
use thiserror::Error;
use tracing::debug;
use wait_timeout::ChildExt;

#[derive(Error, Debug)]
pub enum JsExecutionError {
    #[error("node is not installed or not in PATH. Please install Node.js first.")]
    NodeNotFound,

    #[error(
        "npm is not installed or not in PATH. Inline `// /// script` dependencies require npm."
    )]
    NpmNotFound,

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

    #[error("Invalid `// /// script` metadata: {0}")]
    InvalidScriptMetadata(String),
}

#[derive(Debug, Deserialize, Default)]
struct ScriptMetadata {
    #[serde(default)]
    dependencies: Vec<String>,
}

pub struct JsExecutionService;

impl JsExecutionService {
    pub fn new() -> Self {
        Self
    }

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

    fn path_for_local_node_child() -> String {
        let base = Self::sandbox_path_for_local_exec();
        match std::env::var("PATH") {
            Ok(p) if !p.is_empty() => format!("{base}:{p}"),
            _ => base,
        }
    }

    fn tool_available(name: &str) -> bool {
        Command::new(name)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .env_clear()
            .env("PATH", Self::path_for_local_node_child())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn node_available() -> bool {
        Self::tool_available("node")
    }

    fn npm_available() -> bool {
        Self::tool_available("npm")
    }

    fn apply_sandbox_command(cmd: &mut Command, _limits: &SandboxConfig) {
        let in_image = std::env::var_os("SANDY_IN_DOCKER").is_some()
            || std::env::var_os("SANDY_IN_FIRECRACKER").is_some();
        cmd.stdin(Stdio::null());
        cmd.env_clear();
        cmd.env("PATH", Self::path_for_local_node_child());
        cmd.env("LANG", "C.UTF-8");
        cmd.env("LC_ALL", "C.UTF-8");
        cmd.env("npm_config_update_notifier", "false");
        cmd.env("npm_config_fund", "false");
        cmd.env("npm_config_audit", "false");
        cmd.env("npm_config_loglevel", "error");
        if in_image {
            cmd.env("HOME", "/workspace");
            cmd.env("XDG_CACHE_HOME", "/workspace/.cache");
            cmd.env("npm_config_cache", "/workspace/.cache/npm");
        } else {
            cmd.env("HOME", "/tmp");
            cmd.env("XDG_CACHE_HOME", "/tmp/cache");
            cmd.env("npm_config_cache", "/tmp/cache/npm");
        }
    }

    fn read_lines_capped(
        reader: impl BufRead,
        max_bytes: usize,
    ) -> Result<Vec<String>, JsExecutionError> {
        let mut lines = Vec::new();
        let mut total = 0usize;
        for line in reader.lines() {
            let line =
                line.map_err(|e| JsExecutionError::IoError(format!("Failed to read line: {}", e)))?;
            let sep = if lines.is_empty() { 0 } else { 1 };
            if total.saturating_add(sep).saturating_add(line.len()) > max_bytes {
                return Err(JsExecutionError::OutputTruncated(max_bytes));
            }
            total = total.saturating_add(sep).saturating_add(line.len());
            lines.push(line);
        }
        Ok(lines)
    }

    fn spawn_and_collect(
        mut cmd: Command,
        limits: &SandboxConfig,
        mut on_output: impl FnMut(&str) -> Result<(), Box<dyn std::error::Error>>,
        stream_stdout: bool,
    ) -> Result<ExecutionResult, JsExecutionError> {
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| {
            JsExecutionError::ExecutionFailed(format!("Failed to spawn process: {}", e))
        })?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| JsExecutionError::IoError("Failed to capture stdout".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| JsExecutionError::IoError("Failed to capture stderr".to_string()))?;

        let stdout_lines = if stream_stdout {
            let stdout_reader = BufReader::new(stdout);
            let mut stdout_lines = Vec::new();
            let mut stdout_bytes = 0usize;
            for line in stdout_reader.lines() {
                let line = line.map_err(|e| {
                    JsExecutionError::IoError(format!("Failed to read stdout: {}", e))
                })?;
                let sep = if stdout_lines.is_empty() { 0 } else { 1 };
                if stdout_bytes.saturating_add(sep).saturating_add(line.len())
                    > limits.max_output_bytes
                {
                    let _ = child.kill();
                    let _ = child.wait_timeout(Duration::from_secs(1));
                    return Err(JsExecutionError::OutputTruncated(limits.max_output_bytes));
                }
                stdout_bytes = stdout_bytes.saturating_add(sep).saturating_add(line.len());
                on_output(&line)
                    .map_err(|e| JsExecutionError::IoError(format!("Callback error: {}", e)))?;
                stdout_lines.push(line);
            }
            stdout_lines
        } else {
            match Self::read_lines_capped(BufReader::new(stdout), limits.max_output_bytes) {
                Ok(l) => l,
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait_timeout(Duration::from_secs(1));
                    return Err(e);
                }
            }
        };

        let stderr_lines =
            match Self::read_lines_capped(BufReader::new(stderr), limits.max_output_bytes) {
                Ok(l) => {
                    for line in &l {
                        debug!(target: "sandy::js_stderr", "{}", line);
                    }
                    l
                }
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
                return Err(JsExecutionError::ExecutionTimedOut(limits.timeout_secs));
            }
            Ok(Some(s)) => s,
            Err(e) => {
                let _ = child.kill();
                return Err(JsExecutionError::ExecutionFailed(format!(
                    "Failed to wait for process: {}",
                    e
                )));
            }
        };

        Ok(ExecutionResult {
            stdout: stdout_lines.join("\n"),
            stderr: stderr_lines.join("\n"),
            exit_code: status.code().unwrap_or(-1),
        })
    }

    fn run_node(
        &self,
        script_path: &Path,
        cwd: Option<&Path>,
        limits: &SandboxConfig,
        on_output: impl FnMut(&str) -> Result<(), Box<dyn std::error::Error>>,
        stream_stdout: bool,
    ) -> Result<ExecutionResult, JsExecutionError> {
        let mut cmd = Command::new("node");
        cmd.arg(script_path);
        if let Some(cwd) = cwd {
            cmd.current_dir(cwd);
        }
        Self::apply_sandbox_command(&mut cmd, limits);
        Self::spawn_and_collect(cmd, limits, on_output, stream_stdout)
    }

    fn npm_install(
        &self,
        workspace: &Path,
        dependencies: &[String],
        limits: &SandboxConfig,
    ) -> Result<(), JsExecutionError> {
        let mut cmd = Command::new("npm");
        cmd.arg("install");
        cmd.arg("--ignore-scripts");
        cmd.arg("--omit=dev");
        cmd.arg("--no-audit");
        cmd.arg("--no-fund");
        cmd.args(dependencies);
        cmd.current_dir(workspace);
        Self::apply_sandbox_command(&mut cmd, limits);

        let result = Self::spawn_and_collect(cmd, limits, |_| Ok(()), false)?;
        if result.exit_code != 0 {
            let detail = if result.stderr.is_empty() {
                result.stdout
            } else {
                result.stderr
            };
            return Err(JsExecutionError::ExecutionFailed(format!(
                "npm install failed (exit {}): {}",
                result.exit_code, detail
            )));
        }
        Ok(())
    }

    fn prepare_workspace(source: &str) -> Result<(TempDir, PathBuf), JsExecutionError> {
        let dir = TempDir::new().map_err(|e| {
            JsExecutionError::IoError(format!("Failed to create temporary directory: {}", e))
        })?;
        let package_json = dir.path().join("package.json");
        fs::write(
            &package_json,
            r#"{"name":"sandy-js","private":true,"type":"module"}"#,
        )
        .map_err(|e| JsExecutionError::IoError(format!("Failed to write package.json: {}", e)))?;
        let script_path = dir.path().join("script.mjs");
        fs::write(&script_path, source).map_err(|e| {
            JsExecutionError::IoError(format!("Failed to write temporary script: {}", e))
        })?;
        Ok((dir, script_path))
    }

    fn execute_source(
        &self,
        source: &str,
        original_path: Option<&Path>,
        limits: &SandboxConfig,
        on_output: impl FnMut(&str) -> Result<(), Box<dyn std::error::Error>>,
        stream_stdout: bool,
    ) -> Result<ExecutionResult, JsExecutionError> {
        if !Self::node_available() {
            return Err(JsExecutionError::NodeNotFound);
        }

        let dependencies = parse_js_script_dependencies(source)?;
        if dependencies.is_empty() {
            if let Some(path) = original_path {
                let abs = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    std::env::current_dir()
                        .map(|d| d.join(path))
                        .unwrap_or_else(|_| path.to_path_buf())
                };
                let cwd = abs.parent().map(|p| p.to_path_buf());
                return self.run_node(&abs, cwd.as_deref(), limits, on_output, stream_stdout);
            }
            let mut temp_file = NamedTempFile::with_suffix(".mjs").map_err(|e| {
                JsExecutionError::IoError(format!("Failed to create temporary file: {}", e))
            })?;
            temp_file.write_all(source.as_bytes()).map_err(|e| {
                JsExecutionError::IoError(format!("Failed to write code to temporary file: {}", e))
            })?;
            temp_file.flush().map_err(|e| {
                JsExecutionError::IoError(format!("Failed to flush temporary file: {}", e))
            })?;
            let temp_path = temp_file.into_temp_path();
            return self.run_node(&temp_path, None, limits, on_output, stream_stdout);
        }

        if !Self::npm_available() {
            return Err(JsExecutionError::NpmNotFound);
        }

        let (workspace, script_path) = Self::prepare_workspace(source)?;
        self.npm_install(workspace.path(), &dependencies, limits)?;
        self.run_node(
            &script_path,
            Some(workspace.path()),
            limits,
            on_output,
            stream_stdout,
        )
    }

    pub fn execute_script(&self, script_path: &Path) -> Result<ExecutionResult, JsExecutionError> {
        self.execute_script_with_limits(script_path, &SandboxConfig::default())
    }

    pub fn execute_script_with_limits(
        &self,
        script_path: &Path,
        limits: &SandboxConfig,
    ) -> Result<ExecutionResult, JsExecutionError> {
        if !script_path.exists() {
            return Err(JsExecutionError::ScriptNotFound(
                script_path.to_string_lossy().to_string(),
            ));
        }
        let source = fs::read_to_string(script_path)
            .map_err(|e| JsExecutionError::IoError(format!("Failed to read script: {}", e)))?;
        self.execute_source(&source, Some(script_path), limits, |_| Ok(()), false)
    }

    pub fn execute_code(&self, code: &str) -> Result<ExecutionResult, JsExecutionError> {
        self.execute_code_with_limits(code, &SandboxConfig::default())
    }

    pub fn execute_code_with_limits(
        &self,
        code: &str,
        limits: &SandboxConfig,
    ) -> Result<ExecutionResult, JsExecutionError> {
        self.execute_source(code, None, limits, |_| Ok(()), false)
    }

    pub fn execute_code_streaming(
        &self,
        code: &str,
        on_output: impl FnMut(&str) -> Result<(), Box<dyn std::error::Error>>,
    ) -> Result<ExecutionResult, JsExecutionError> {
        self.execute_code_streaming_with_limits(code, on_output, &SandboxConfig::default())
    }

    pub fn execute_code_streaming_with_limits(
        &self,
        code: &str,
        on_output: impl FnMut(&str) -> Result<(), Box<dyn std::error::Error>>,
        limits: &SandboxConfig,
    ) -> Result<ExecutionResult, JsExecutionError> {
        self.execute_source(code, None, limits, on_output, true)
    }

    pub fn execute_script_streaming(
        &self,
        script_path: &Path,
        on_output: impl FnMut(&str) -> Result<(), Box<dyn std::error::Error>>,
    ) -> Result<ExecutionResult, JsExecutionError> {
        self.execute_script_streaming_with_limits(script_path, on_output, &SandboxConfig::default())
    }

    pub fn execute_script_streaming_with_limits(
        &self,
        script_path: &Path,
        on_output: impl FnMut(&str) -> Result<(), Box<dyn std::error::Error>>,
        limits: &SandboxConfig,
    ) -> Result<ExecutionResult, JsExecutionError> {
        if !script_path.exists() {
            return Err(JsExecutionError::ScriptNotFound(
                script_path.to_string_lossy().to_string(),
            ));
        }
        let source = fs::read_to_string(script_path)
            .map_err(|e| JsExecutionError::IoError(format!("Failed to read script: {}", e)))?;
        self.execute_source(&source, Some(script_path), limits, on_output, true)
    }
}

impl Default for JsExecutionService {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse `dependencies` from a leading `// /// script` block (PEP 723-style, JS comments).
pub fn parse_js_script_dependencies(source: &str) -> Result<Vec<String>, JsExecutionError> {
    let Some(toml_src) = js_script_block_toml(source) else {
        return Ok(Vec::new());
    };
    if toml_src.trim().is_empty() {
        return Ok(Vec::new());
    }
    let meta: ScriptMetadata = toml::from_str(&toml_src).map_err(|e| {
        JsExecutionError::InvalidScriptMetadata(format!("invalid TOML in script block: {e}"))
    })?;
    Ok(meta.dependencies)
}

fn js_script_block_toml(source: &str) -> Option<String> {
    let mut lines = source.lines().peekable();
    if let Some(first) = lines.peek() {
        if first.starts_with("#!") {
            lines.next();
        }
    }
    while let Some(line) = lines.peek() {
        if line.trim().is_empty() {
            lines.next();
        } else {
            break;
        }
    }
    let first = lines.next()?;
    let first_trim = first.trim();
    if first_trim != "// /// script" && first_trim != "///// script" {
        return None;
    }
    let mut inner = Vec::new();
    for line in lines {
        let trimmed = line.trim();
        if trimmed == "// ///" || trimmed == "/////" {
            return Some(inner.join("\n"));
        }
        inner.push(strip_js_comment_prefix(line));
    }
    None
}

fn strip_js_comment_prefix(line: &str) -> String {
    let raw = line.trim_start();
    if let Some(rest) = raw.strip_prefix("// ") {
        rest.to_string()
    } else if let Some(rest) = raw.strip_prefix("//") {
        rest.to_string()
    } else {
        raw.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_no_block() {
        assert!(parse_js_script_dependencies("console.log(1)\n")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn parse_empty_deps() {
        let src = r#"// /// script
// dependencies = []
// ///
console.log(1)
"#;
        assert!(parse_js_script_dependencies(src).unwrap().is_empty());
    }

    #[test]
    fn parse_deps_and_shebang() {
        let src = r#"#!/usr/bin/env node
// /// script
// dependencies = ["lodash", "zod@3"]
// ///
import _ from "lodash";
"#;
        assert_eq!(
            parse_js_script_dependencies(src).unwrap(),
            vec!["lodash".to_string(), "zod@3".to_string()]
        );
    }

    #[test]
    fn parse_multiline_deps() {
        let src = r#"// /// script
// dependencies = [
//     "ms",
// ]
// ///
console.log(1)
"#;
        assert_eq!(
            parse_js_script_dependencies(src).unwrap(),
            vec!["ms".to_string()]
        );
    }

    #[test]
    fn execute_console_log_when_node_available() {
        if !JsExecutionService::node_available() {
            return;
        }
        let svc = JsExecutionService::new();
        let result = svc
            .execute_code("console.log('hello-sandy-js')")
            .expect("node exec");
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("hello-sandy-js"));
    }

    #[test]
    fn execute_relative_script_path_when_node_available() {
        if !JsExecutionService::node_available() {
            return;
        }
        let rel = PathBuf::from("target/sandy-js-rel-test.js");
        std::fs::create_dir_all("target").unwrap();
        std::fs::write(&rel, "console.log('from-file')\n").unwrap();
        let svc = JsExecutionService::new();
        let result = svc.execute_script(&rel).expect("node script");
        let _ = std::fs::remove_file(&rel);
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("from-file"));
    }
}
