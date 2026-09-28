use clap::Args;
use std::io::{self, Read};
use std::path::PathBuf;

use crate::service::firecracker::FirecrackerConfig;

use super::firecracker_exec::{execute, FirecrackerCodeKind};

#[derive(Args, Debug)]
#[command(about = "Execute Python code in a Firecracker microVM")]
pub struct FirecrackerExecArgs {
    #[arg(short, long)]
    pub code: Option<String>,

    #[arg(long)]
    pub kernel: Option<PathBuf>,

    #[arg(long)]
    pub rootfs: Option<PathBuf>,

    #[arg(long)]
    pub allow_network: bool,
}

pub async fn run_firecracker_exec(
    args: FirecrackerExecArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let code = read_code(args.code)?;
    let mut config = FirecrackerConfig::default();
    if let Some(kernel) = args.kernel {
        config.kernel_path = kernel;
    }
    if let Some(rootfs) = args.rootfs {
        config.rootfs_path = rootfs;
    }
    if args.allow_network {
        config.tap_device = std::env::var("SANDY_FC_TAP").ok();
        if config.tap_device.is_none() {
            return Err("network access requires SANDY_FC_TAP".into());
        }
    }
    execute(FirecrackerCodeKind::Python, code, config).await
}

fn read_code(code: Option<String>) -> Result<String, io::Error> {
    match code {
        Some(code) => Ok(code),
        None => {
            let mut buffer = String::new();
            io::stdin().read_to_string(&mut buffer)?;
            Ok(buffer)
        }
    }
}
