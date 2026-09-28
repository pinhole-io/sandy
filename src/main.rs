use clap::Parser;
use sandy::{Cli, Commands};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Shell(args) => sandy::run_shell(args)?,
        Commands::Py03(args) => sandy::run_py03(args)?,
        Commands::ExecCode(args) => sandy::run_exec_code(args)?,
        Commands::Js(args) => sandy::run_js(args)?,
        Commands::ExecJs(args) => sandy::run_exec_js(args)?,
        #[cfg(feature = "docker")]
        Commands::DockerExec(args) => sandy::run_docker_exec(args).await?,
        #[cfg(feature = "docker")]
        Commands::DockerExecJs(args) => sandy::run_docker_exec_js(args).await?,
        #[cfg(feature = "firecracker")]
        Commands::FirecrackerExec(args) => sandy::run_firecracker_exec(args).await?,
        #[cfg(feature = "firecracker")]
        Commands::FirecrackerExecJs(args) => sandy::run_firecracker_exec_js(args).await?,
        #[cfg(feature = "firecracker")]
        Commands::FirecrackerAgent => sandy::run_firecracker_agent()?,
        #[cfg(feature = "firecracker")]
        Commands::MicroVm(args) => sandy::run_microvm(args).await?,
    }

    Ok(())
}
