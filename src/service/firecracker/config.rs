use std::env;
use std::path::PathBuf;

use super::error::FirecrackerError;

const DEFAULT_VCPU_COUNT: u8 = 1;
const DEFAULT_MEMORY_MIB: u32 = 1024;
const DEFAULT_TIMEOUT_SECS: u64 = 300;
const DEFAULT_GUEST_CID: u32 = 3;
const DEFAULT_AGENT_PORT: u32 = 52;

#[derive(Debug, Clone)]
pub struct FirecrackerConfig {
    pub binary: PathBuf,
    pub kernel_path: PathBuf,
    pub rootfs_path: PathBuf,
    pub vcpu_count: u8,
    pub memory_mib: u32,
    pub guest_cid: u32,
    pub agent_port: u32,
    pub timeout_secs: u64,
    pub tap_device: Option<String>,
    pub state_dir: PathBuf,
}

impl Default for FirecrackerConfig {
    fn default() -> Self {
        Self {
            binary: env::var_os("SANDY_FIRECRACKER")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("firecracker")),
            kernel_path: env::var_os("SANDY_FC_KERNEL")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(".firecracker/out/vmlinux")),
            rootfs_path: env::var_os("SANDY_FC_ROOTFS")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(".firecracker/out/rootfs.ext4")),
            state_dir: default_state_dir(),
            vcpu_count: DEFAULT_VCPU_COUNT,
            memory_mib: DEFAULT_MEMORY_MIB,
            guest_cid: DEFAULT_GUEST_CID,
            agent_port: DEFAULT_AGENT_PORT,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            tap_device: None,
        }
    }
}

impl FirecrackerConfig {
    pub fn validate(&self) -> Result<(), FirecrackerError> {
        if self.vcpu_count == 0 {
            return Err(FirecrackerError::InvalidConfig(
                "vcpu_count must be positive".into(),
            ));
        }
        if self.memory_mib == 0 {
            return Err(FirecrackerError::InvalidConfig(
                "memory_mib must be positive".into(),
            ));
        }
        if self.guest_cid < 3 {
            return Err(FirecrackerError::InvalidConfig(
                "guest_cid must be at least 3".into(),
            ));
        }
        if self.agent_port == 0 {
            return Err(FirecrackerError::InvalidConfig(
                "agent_port must be positive".into(),
            ));
        }
        if self.timeout_secs == 0 {
            return Err(FirecrackerError::InvalidConfig(
                "timeout_secs must be positive".into(),
            ));
        }
        if self.tap_device.as_deref().is_some_and(str::is_empty) {
            return Err(FirecrackerError::InvalidConfig(
                "tap_device must not be empty".into(),
            ));
        }
        Ok(())
    }
}

pub(super) const API_SOCKET_NAME: &str = "firecracker.socket";
pub(super) const VSOCK_SOCKET_NAME: &str = "agent.vsock";

fn default_state_dir() -> PathBuf {
    if let Some(path) = env::var_os("SANDY_FC_STATE_DIR") {
        return PathBuf::from(path);
    }
    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".cache")
            .join("sandy")
            .join("firecracker")
            .join("vms");
    }
    PathBuf::from("/tmp/sandy-firecracker/vms")
}
