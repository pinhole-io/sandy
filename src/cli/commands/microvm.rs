use std::path::PathBuf;

use clap::{Args, Subcommand};

use crate::service::firecracker::{FirecrackerConfig, FirecrackerService, FirecrackerVmInfo};

#[derive(Args, Debug)]
pub struct MicroVmArgs {
    #[command(subcommand)]
    pub action: MicroVmAction,
}

#[derive(Subcommand, Debug)]
pub enum MicroVmAction {
    /// Start a named Firecracker microVM.
    Start(MicroVmStartArgs),
    /// Stop a named Firecracker microVM.
    Stop(MicroVmStopArgs),
    /// Show the state of one or all Firecracker microVMs.
    Status(MicroVmStatusArgs),
}

#[derive(Args, Debug)]
pub struct MicroVmStartArgs {
    /// Persistent name used by stop and status.
    #[arg(long, default_value = "default")]
    pub name: String,

    /// Firecracker binary path.
    #[arg(long)]
    pub binary: Option<PathBuf>,

    /// Firecracker kernel image.
    #[arg(long)]
    pub kernel: Option<PathBuf>,

    /// Guest ext4 rootfs.
    #[arg(long)]
    pub rootfs: Option<PathBuf>,

    /// Number of virtual CPUs.
    #[arg(long, default_value_t = 1)]
    pub vcpu_count: u8,

    /// Guest memory in MiB.
    #[arg(long, default_value_t = 1024)]
    pub memory_mib: u32,

    /// Attach this pre-created TAP device.
    #[arg(long)]
    pub tap_device: Option<String>,

    /// Directory that stores VM state and sockets.
    #[arg(long)]
    pub state_dir: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct MicroVmStopArgs {
    #[arg(long, default_value = "default")]
    pub name: String,

    /// Send SIGKILL instead of SIGTERM.
    #[arg(long)]
    pub force: bool,

    #[arg(long)]
    pub state_dir: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct MicroVmStatusArgs {
    /// Show one VM. Omit this flag to show all VMs.
    #[arg(long)]
    pub name: Option<String>,

    /// Print machine-readable JSON.
    #[arg(long)]
    pub json: bool,

    #[arg(long)]
    pub state_dir: Option<PathBuf>,
}

pub async fn run_microvm(args: MicroVmArgs) -> Result<(), Box<dyn std::error::Error>> {
    match args.action {
        MicroVmAction::Start(args) => start(args).await,
        MicroVmAction::Stop(args) => stop(args),
        MicroVmAction::Status(args) => status(args),
    }
}

async fn start(args: MicroVmStartArgs) -> Result<(), Box<dyn std::error::Error>> {
    let mut config = FirecrackerConfig::default();
    if let Some(binary) = args.binary {
        config.binary = binary;
    }
    if let Some(kernel) = args.kernel {
        config.kernel_path = kernel;
    }
    if let Some(rootfs) = args.rootfs {
        config.rootfs_path = rootfs;
    }
    if args.vcpu_count != 0 {
        config.vcpu_count = args.vcpu_count;
    }
    if args.memory_mib != 0 {
        config.memory_mib = args.memory_mib;
    }
    if let Some(tap_device) = args.tap_device {
        config.tap_device = Some(tap_device);
    }
    if let Some(state_dir) = args.state_dir {
        config.state_dir = state_dir;
    }

    let service = FirecrackerService::new(config)?;
    let info = service.start_vm(&args.name).await?;
    println!(
        "started microVM '{}' (pid {})\napi socket: {}\nvsock socket: {}",
        info.name,
        info.pid,
        info.api_socket.display(),
        info.vsock_socket.display()
    );
    Ok(())
}

fn stop(args: MicroVmStopArgs) -> Result<(), Box<dyn std::error::Error>> {
    let mut config = FirecrackerConfig::default();
    if let Some(state_dir) = args.state_dir {
        config.state_dir = state_dir;
    }
    let service = FirecrackerService::new(config)?;
    match service.stop_vm(&args.name, args.force)? {
        Some(info) => println!("stopped microVM '{}' (pid {})", info.name, info.pid),
        None => println!("microVM '{}' is not registered", args.name),
    }
    Ok(())
}

fn status(args: MicroVmStatusArgs) -> Result<(), Box<dyn std::error::Error>> {
    let mut config = FirecrackerConfig::default();
    if let Some(state_dir) = args.state_dir {
        config.state_dir = state_dir;
    }
    let service = FirecrackerService::new(config)?;
    let infos = service.status_vms(args.name.as_deref())?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&infos)?);
    } else {
        print_text_status(&infos);
    }
    Ok(())
}

fn print_text_status(infos: &[FirecrackerVmInfo]) {
    if infos.is_empty() {
        println!("no registered microVMs");
        return;
    }
    for info in infos {
        let state = if info.running { "running" } else { "stopped" };
        println!(
            "{}\t{}\tpid={}\tapi={}",
            info.name,
            state,
            info.pid,
            info.api_socket.display()
        );
    }
}
