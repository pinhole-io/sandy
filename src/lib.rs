pub mod cli;
pub mod exec_sandbox;
pub mod service;

#[cfg(feature = "python-bindings")]
mod python_bindings;

#[cfg(feature = "docker")]
pub use cli::run_docker_exec;
#[cfg(feature = "docker")]
pub use cli::run_docker_exec_js;
pub use cli::{run_exec_code, run_exec_js, run_js, run_py03, run_shell, Cli, Commands};
#[cfg(feature = "firecracker")]
pub use cli::{run_firecracker_agent, run_firecracker_exec, run_firecracker_exec_js, run_microvm};
#[cfg(feature = "firecracker")]
pub use exec_sandbox::FirecrackerShellSandbox;
pub use exec_sandbox::{
    bwrap_apparmor_profile, bwrap_unavailable_reason, emit_result_output, needs_bwrap_apparmor,
    none_sandbox_fallback_count, reset_bwrap_probe, reset_none_sandbox_fallback_count,
    select_backend, BubblewrapSandbox, CommandSandbox, EnvPolicy, ExecBackend, ExecPolicy,
    ExecResult, NetworkPolicy, NoneSandbox, OutputSink, OutputStream, SandboxError,
    SandboxSelection,
};
#[cfg(feature = "docker")]
pub use service::docker::{DockerConfig, DockerError, DockerService};
pub use service::docker::{SandboxConfig, DOCKER_UV_PYTHON_INSTALL_DIR};
#[cfg(feature = "firecracker")]
pub use service::firecracker::{
    FirecrackerConfig, FirecrackerError, FirecrackerExecutionResult, FirecrackerService,
    FirecrackerVmInfo,
};
pub use service::{
    ExecutionResult, JsExecutionError, JsExecutionService, PythonExecutionError,
    PythonExecutionService,
};

#[cfg(feature = "python-bindings")]
use pyo3::prelude::*;

/// Native extension `sandy._native` (imported by the `sandy` Python package).
#[cfg(feature = "python-bindings")]
#[pymodule]
#[pyo3(name = "_native")]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    python_bindings::init_module(m)
}
