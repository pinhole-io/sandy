use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use tokio::process::{Child, Command};
use tokio::time::{sleep, timeout};

use super::config::{FirecrackerConfig, API_SOCKET_NAME, VSOCK_SOCKET_NAME};
use super::error::FirecrackerError;
use super::protocol::{GuestRequest, GuestResponse};
use super::vsock::{connect_to_guest, receive_json_line, send_json_line, set_stream_timeout};

#[derive(Debug, Clone)]
pub struct FirecrackerExecutionResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirecrackerVmInfo {
    pub name: String,
    pub pid: u32,
    pub api_socket: PathBuf,
    pub vsock_socket: PathBuf,
    #[serde(default)]
    pub log_path: PathBuf,
    pub guest_cid: u32,
    pub agent_port: u32,
    pub kernel_path: PathBuf,
    pub rootfs_path: PathBuf,
    pub started_at: String,
    pub running: bool,
}

pub struct FirecrackerService {
    config: FirecrackerConfig,
}

impl FirecrackerService {
    pub fn new(config: FirecrackerConfig) -> Result<Self, FirecrackerError> {
        config.validate()?;
        Ok(Self { config })
    }

    pub fn with_defaults() -> Result<Self, FirecrackerError> {
        Self::new(FirecrackerConfig::default())
    }

    pub fn config(&self) -> &FirecrackerConfig {
        &self.config
    }

    pub fn is_available(&self) -> bool {
        cfg!(target_os = "linux")
            && Path::new("/dev/kvm").exists()
            && command_available(&self.config.binary)
            && self.config.kernel_path.is_file()
            && self.config.rootfs_path.is_file()
    }

    pub async fn start_vm(&self, name: &str) -> Result<FirecrackerVmInfo, FirecrackerError> {
        validate_vm_name(name)?;
        self.validate_artifacts()?;
        std::fs::create_dir_all(&self.config.state_dir)?;

        let state_path = self.state_path(name)?;
        if state_path.exists() {
            let existing = read_vm_info(&state_path)?;
            if process_alive(existing.pid) {
                return Err(FirecrackerError::Unavailable(format!(
                    "VM '{name}' is already running with PID {}",
                    existing.pid
                )));
            }
            remove_vm_files(&existing, &state_path);
        }

        let api_socket = self.config.state_dir.join(format!("{name}.socket"));
        let vsock_socket = self.config.state_dir.join(format!("{name}.vsock"));
        let log_path = self.config.state_dir.join(format!("{name}.log"));
        let _ = std::fs::remove_file(&api_socket);
        let _ = std::fs::remove_file(&vsock_socket);
        let _ = std::fs::remove_file(&log_path);
        let mut child = spawn_firecracker(&self.config, &api_socket, Some(&log_path))?;
        let boot_result = self.configure_vm(&api_socket, &vsock_socket).await;
        if let Err(error) = boot_result {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(error);
        }
        if let Some(status) = child.try_wait()? {
            return Err(FirecrackerError::Unavailable(format!(
                "Firecracker exited during boot with {status}; see {}",
                log_path.display()
            )));
        }

        let pid = child
            .id()
            .ok_or_else(|| FirecrackerError::Unavailable("Firecracker has no process ID".into()))?;
        let info = FirecrackerVmInfo {
            name: name.to_string(),
            pid,
            api_socket,
            vsock_socket,
            log_path,
            guest_cid: self.config.guest_cid,
            agent_port: self.config.agent_port,
            kernel_path: self.config.kernel_path.clone(),
            rootfs_path: self.config.rootfs_path.clone(),
            started_at: chrono::Utc::now().to_rfc3339(),
            running: true,
        };
        write_vm_info(&state_path, &info)?;
        Ok(info)
    }

    pub fn status_vms(
        &self,
        name: Option<&str>,
    ) -> Result<Vec<FirecrackerVmInfo>, FirecrackerError> {
        if let Some(name) = name {
            validate_vm_name(name)?;
            let state_path = self.state_path(name)?;
            if !state_path.is_file() {
                return Ok(Vec::new());
            }
            return Ok(vec![refresh_vm_info(read_vm_info(&state_path)?)]);
        }
        if !self.config.state_dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut infos = Vec::new();
        for entry in std::fs::read_dir(&self.config.state_dir)? {
            let path = entry?.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            infos.push(refresh_vm_info(read_vm_info(&path)?));
        }
        infos.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(infos)
    }

    pub fn stop_vm(
        &self,
        name: &str,
        force: bool,
    ) -> Result<Option<FirecrackerVmInfo>, FirecrackerError> {
        validate_vm_name(name)?;
        let state_path = self.state_path(name)?;
        if !state_path.is_file() {
            return Ok(None);
        }
        let info = read_vm_info(&state_path)?;
        if process_alive(info.pid) {
            terminate_process(info.pid, force)?;
        }
        remove_vm_files(&info, &state_path);
        Ok(Some(info))
    }

    pub async fn execute_python_code(
        &self,
        code: &str,
    ) -> Result<FirecrackerExecutionResult, FirecrackerError> {
        self.execute(GuestRequest::Python {
            code: code.to_string(),
        })
        .await
    }

    pub async fn execute_javascript_code(
        &self,
        code: &str,
    ) -> Result<FirecrackerExecutionResult, FirecrackerError> {
        self.execute(GuestRequest::Javascript {
            code: code.to_string(),
        })
        .await
    }

    pub async fn execute_shell(
        &self,
        command: &str,
        workspace_hex: String,
        env: Vec<(String, String)>,
    ) -> Result<(FirecrackerExecutionResult, Option<String>), FirecrackerError> {
        let response = self
            .run_guest(GuestRequest::Shell {
                command: command.to_string(),
                workspace_hex,
                env,
            })
            .await?;
        Ok((
            FirecrackerExecutionResult {
                stdout: response.stdout,
                stderr: response.stderr,
                exit_code: response.exit_code,
            },
            response.workspace_hex,
        ))
    }

    async fn execute(
        &self,
        request: GuestRequest,
    ) -> Result<FirecrackerExecutionResult, FirecrackerError> {
        let response = self.run_guest(request).await?;
        Ok(FirecrackerExecutionResult {
            stdout: response.stdout,
            stderr: response.stderr,
            exit_code: response.exit_code,
        })
    }

    async fn run_guest(&self, request: GuestRequest) -> Result<GuestResponse, FirecrackerError> {
        self.validate_artifacts()?;

        let temp = TempDir::new()?;
        let api_socket = temp.path().join(API_SOCKET_NAME);
        let vsock_socket = temp.path().join(VSOCK_SOCKET_NAME);
        let mut child = spawn_firecracker(&self.config, &api_socket, None)?;

        let result = timeout(
            Duration::from_secs(self.config.timeout_secs),
            self.configure_and_execute(&api_socket, &vsock_socket, request, &mut child),
        )
        .await
        .map_err(|_| FirecrackerError::TimedOut(self.config.timeout_secs))?;

        let _ = child.kill().await;
        let _ = child.wait().await;
        result
    }

    fn validate_artifacts(&self) -> Result<(), FirecrackerError> {
        if !cfg!(target_os = "linux") {
            return Err(FirecrackerError::UnsupportedPlatform);
        }
        if !self.config.kernel_path.is_file() {
            return Err(FirecrackerError::Unavailable(format!(
                "kernel image does not exist: {} (run `make firecracker-kernel`)",
                self.config.kernel_path.display(),
            )));
        }
        if !self.config.rootfs_path.is_file() {
            return Err(FirecrackerError::Unavailable(format!(
                "rootfs image does not exist: {} (run `make firecracker-rootfs`)",
                self.config.rootfs_path.display(),
            )));
        }
        if let Err(error) = std::fs::OpenOptions::new()
            .write(true)
            .open(&self.config.rootfs_path)
        {
            return Err(FirecrackerError::Unavailable(format!(
                "rootfs image is not writable by the current user: {} ({error})",
                self.config.rootfs_path.display(),
            )));
        }
        if !Path::new("/dev/kvm").exists() {
            return Err(FirecrackerError::Unavailable(
                "/dev/kvm is not available".into(),
            ));
        }
        if !command_available(&self.config.binary) {
            return Err(FirecrackerError::Unavailable(format!(
                "Firecracker binary is not available: {}",
                self.config.binary.display()
            )));
        }
        Ok(())
    }

    fn state_path(&self, name: &str) -> Result<PathBuf, FirecrackerError> {
        validate_vm_name(name)?;
        Ok(self.config.state_dir.join(format!("{name}.json")))
    }

    async fn configure_and_execute(
        &self,
        api_socket: &Path,
        vsock_socket: &Path,
        request: GuestRequest,
        child: &mut Child,
    ) -> Result<GuestResponse, FirecrackerError> {
        self.configure_vm(api_socket, vsock_socket).await?;

        let socket_path = vsock_socket.to_path_buf();
        let port = self.config.agent_port;
        tokio::task::spawn_blocking(move || {
            let mut last_error = None;
            for _ in 0..100 {
                match connect_to_guest(&socket_path, port) {
                    Ok(mut stream) => {
                        set_stream_timeout(&stream, Duration::from_secs(30))?;
                        match send_json_line(&mut stream, &request)
                            .and_then(|()| receive_json_line(&mut stream))
                        {
                            Ok(response) => return Ok(response),
                            Err(error) => last_error = Some(error),
                        }
                    }
                    Err(error) => last_error = Some(error),
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(last_error.unwrap_or_else(|| {
                FirecrackerError::Guest("guest agent did not accept a connection".into())
            }))
        })
        .await
        .map_err(|e| FirecrackerError::Guest(format!("vsock task failed: {e}")))?
        .and_then(|response: GuestResponse| {
            if let Some(error) = response.error.clone() {
                Err(FirecrackerError::Guest(error))
            } else {
                Ok(response)
            }
        })
        .or_else(|error| {
            if child.try_wait()?.is_some() {
                Err(FirecrackerError::Guest(format!(
                    "{error}; Firecracker exited before the guest responded"
                )))
            } else {
                Err(error)
            }
        })
    }

    async fn configure_vm(
        &self,
        api_socket: &Path,
        vsock_socket: &Path,
    ) -> Result<(), FirecrackerError> {
        wait_for_socket(api_socket, self.config.timeout_secs).await?;

        api_put(
            api_socket,
            "/boot-source",
            &serde_json::json!({
                "kernel_image_path": absolute_path(&self.config.kernel_path)?,
                "boot_args": "console=ttyS0 root=/dev/vda rw init=/sbin/init reboot=k panic=1 pci=off"
            }),
        )
        .await?;
        api_put(
            api_socket,
            "/drives/rootfs",
            &serde_json::json!({
                "drive_id": "rootfs",
                "path_on_host": absolute_path(&self.config.rootfs_path)?,
                "is_root_device": true,
                "is_read_only": false
            }),
        )
        .await?;
        api_put(
            api_socket,
            "/machine-config",
            &serde_json::json!({
                "vcpu_count": self.config.vcpu_count,
                "mem_size_mib": self.config.memory_mib,
                "smt": false
            }),
        )
        .await?;
        api_put(
            api_socket,
            "/vsock",
            &serde_json::json!({
                "guest_cid": self.config.guest_cid,
                "uds_path": vsock_socket
            }),
        )
        .await?;
        if let Some(tap_device) = &self.config.tap_device {
            api_put(
                api_socket,
                "/network-interfaces/eth0",
                &serde_json::json!({
                    "iface_id": "eth0",
                    "guest_mac": "06:00:AC:10:00:02",
                    "host_dev_name": tap_device
                }),
            )
            .await?;
        }
        api_put(
            api_socket,
            "/actions",
            &serde_json::json!({"action_type": "InstanceStart"}),
        )
        .await?;

        wait_for_socket(vsock_socket, self.config.timeout_secs).await
    }
}

fn spawn_firecracker(
    config: &FirecrackerConfig,
    api_socket: &Path,
    stderr_path: Option<&Path>,
) -> Result<Child, FirecrackerError> {
    let mut command = Command::new(&config.binary);
    command
        .arg("--api-sock")
        .arg(api_socket)
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    if let Some(stderr_path) = stderr_path {
        let stdout = std::fs::File::create(stderr_path)?;
        command.stdout(Stdio::from(stdout.try_clone()?));
        command.stderr(Stdio::from(stdout));
    } else {
        command.stdout(Stdio::null());
        command.stderr(Stdio::null());
    }
    Ok(command.spawn()?)
}

async fn wait_for_socket(path: &Path, timeout_secs: u64) -> Result<(), FirecrackerError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    while tokio::time::Instant::now() < deadline {
        if path.exists() {
            return Ok(());
        }
        sleep(Duration::from_millis(20)).await;
    }
    Err(FirecrackerError::Unavailable(format!(
        "socket did not appear: {}",
        path.display()
    )))
}

async fn api_put(
    socket_path: &Path,
    endpoint: &str,
    body: &serde_json::Value,
) -> Result<(), FirecrackerError> {
    let socket_path = socket_path.to_path_buf();
    let body = serde_json::to_vec(body)?;
    let endpoint = endpoint.to_string();
    tokio::task::spawn_blocking(move || {
        let mut stream = std::os::unix::net::UnixStream::connect(socket_path)?;
        let request = format!(
            "PUT {endpoint} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(request.as_bytes())?;
        stream.write_all(&body)?;
        stream.flush()?;

        let mut response = Vec::new();
        let mut byte = [0_u8; 1];
        while !response.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte)?;
            response.push(byte[0]);
            if response.len() > 64 * 1024 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Firecracker API headers are too large",
                ));
            }
        }
        let header_end = response
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid API response")
            })?;
        let status = String::from_utf8_lossy(&response[..header_end]);
        let status_line = status.lines().next().unwrap_or_default();
        if !status_line.contains(" 2") {
            let content_length = status
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    (name.eq_ignore_ascii_case("content-length"))
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            let mut body = vec![0_u8; content_length];
            if content_length > 0 {
                stream.read_exact(&mut body)?;
            }
            return Err(std::io::Error::other(format!(
                "{endpoint}: {status_line}: {}",
                String::from_utf8_lossy(&body).trim()
            )));
        }
        Ok(())
    })
    .await
    .map_err(|e| FirecrackerError::Api(e.to_string()))?
    .map_err(|e: std::io::Error| FirecrackerError::Api(e.to_string()))
}

fn absolute_path(path: &Path) -> Result<String, FirecrackerError> {
    std::fs::canonicalize(path)
        .map(|path| path.display().to_string())
        .map_err(|e| FirecrackerError::InvalidConfig(format!("{}: {e}", path.display())))
}

fn command_available(binary: &Path) -> bool {
    if binary.components().count() > 1 {
        return binary.is_file();
    }
    std::env::var_os("PATH")
        .unwrap_or_default()
        .to_string_lossy()
        .split(':')
        .map(PathBuf::from)
        .map(|dir| dir.join(binary))
        .any(|path| path.is_file())
}

fn validate_vm_name(name: &str) -> Result<(), FirecrackerError> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(FirecrackerError::InvalidConfig(
            "VM name must contain only letters, numbers, '.', '_' or '-'".into(),
        ));
    }
    Ok(())
}

fn read_vm_info(path: &Path) -> Result<FirecrackerVmInfo, FirecrackerError> {
    let data = std::fs::read(path)?;
    serde_json::from_slice(&data).map_err(FirecrackerError::Json)
}

fn write_vm_info(path: &Path, info: &FirecrackerVmInfo) -> Result<(), FirecrackerError> {
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(info)?)?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

fn refresh_vm_info(mut info: FirecrackerVmInfo) -> FirecrackerVmInfo {
    info.running = process_alive(info.pid);
    info
}

fn process_alive(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        false
    }
}

fn terminate_process(pid: u32, force: bool) -> Result<(), FirecrackerError> {
    #[cfg(target_os = "linux")]
    {
        let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
        let result = unsafe { libc::kill(pid as libc::pid_t, signal) };
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(FirecrackerError::Io(error));
            }
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (pid, force);
        Err(FirecrackerError::UnsupportedPlatform)
    }
}

fn remove_vm_files(info: &FirecrackerVmInfo, state_path: &Path) {
    let _ = std::fs::remove_file(state_path);
    let _ = std::fs::remove_file(&info.api_socket);
    let _ = std::fs::remove_file(&info.vsock_socket);
    let _ = std::fs::remove_file(&info.log_path);
}
