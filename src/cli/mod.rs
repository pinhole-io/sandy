mod commands;

use clap::{Parser, Subcommand};

/// Sandy — Python and JavaScript sandbox (local + Docker)
#[derive(Parser)]
#[command(name = "sandy")]
#[command(
    about = "Run Python and JavaScript in a controlled way (uv / Node.js / Docker)",
    long_about = None
)]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Start an interactive shell
    Shell(commands::shell::ShellArgs),
    /// Execute a Python script with uv inline packages
    Py03(commands::py03::Py03Args),
    /// Execute Python code from a string
    ExecCode(commands::exec_code::ExecCodeArgs),
    /// Execute a JavaScript file with Node.js
    Js(commands::js::JsArgs),
    /// Execute JavaScript code from a string
    ExecJs(commands::exec_js::ExecJsArgs),
    /// Execute Python code in a Docker container
    #[cfg(feature = "docker")]
    DockerExec(commands::docker_exec::DockerExecArgs),
    /// Execute JavaScript code in a Docker container
    #[cfg(feature = "docker")]
    DockerExecJs(commands::docker_exec_js::DockerExecJsArgs),
    /// Execute Python code in a Firecracker microVM
    #[cfg(feature = "firecracker")]
    #[command(name = "fc-exec")]
    FirecrackerExec(commands::firecracker_exec_code::FirecrackerExecArgs),
    /// Execute JavaScript code in a Firecracker microVM
    #[cfg(feature = "firecracker")]
    #[command(name = "fc-exec-js")]
    FirecrackerExecJs(commands::firecracker_exec_js::FirecrackerExecJsArgs),
    /// Guest-only Firecracker agent
    #[cfg(feature = "firecracker")]
    #[command(name = "fc-agent", hide = true)]
    FirecrackerAgent,
    /// Manage persistent Firecracker microVMs
    #[cfg(feature = "firecracker")]
    #[command(name = "microvm")]
    MicroVm(commands::microvm::MicroVmArgs),
}

#[cfg(feature = "docker")]
pub use commands::docker_exec::run_docker_exec;
#[cfg(feature = "docker")]
pub use commands::docker_exec_js::run_docker_exec_js;
pub use commands::exec_code::run_exec_code;
pub use commands::exec_js::run_exec_js;
#[cfg(feature = "firecracker")]
pub use commands::firecracker_agent::run_firecracker_agent;
#[cfg(feature = "firecracker")]
pub use commands::firecracker_exec_code::run_firecracker_exec;
#[cfg(feature = "firecracker")]
pub use commands::firecracker_exec_js::run_firecracker_exec_js;
pub use commands::js::run_js;
#[cfg(feature = "firecracker")]
pub use commands::microvm::run_microvm;
pub use commands::py03::run_py03;
pub use commands::shell::run_shell;
