use bollard::container::LogOutput;
use bollard::exec::{CreateExecOptions, StartExecResults};
use bollard::models::ContainerCreateBody;
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, KillContainerOptionsBuilder, ListImagesOptionsBuilder,
    LogsOptionsBuilder, RemoveContainerOptionsBuilder,
};
use bollard::Docker;
use futures_util::stream::StreamExt;
use std::collections::HashMap;
use std::env;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::Command as TokioCommand;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tokio::time;
use tracing::{debug, info, warn};

use crate::service::docker::config::{DockerConfig, DOCKER_UV_PYTHON_INSTALL_DIR};
use crate::service::docker::error::DockerError;

const OUTPUT_TRUNCATED_MARKER: &str = "\n[output truncated at limit]";
const POLL_INTERVAL_MS: u64 = 500;
const DOCKER_CONNECT_TIMEOUT_SECS: u64 = 120;

/// Service for managing Docker containers and executing code within them.
pub struct DockerService {
    docker: Docker,
    config: DockerConfig,
    concurrency: Arc<Semaphore>,
    /// Held until `remove_container` for persistent / keep-alive / explicit creates.
    persistent_permits: Arc<Mutex<HashMap<String, OwnedSemaphorePermit>>>,
}

impl DockerService {
    /// Create a new Docker service instance
    /// Connects to the system Docker daemon via the default socket
    /// Supports standard Docker, Docker Desktop, and Colima
    pub async fn new(config: DockerConfig) -> Result<Self, DockerError> {
        // Validate configuration
        config.validate()?;

        // Try to connect to Docker daemon
        // First check DOCKER_HOST, then try Colima sockets, then default
        let docker = if let Ok(docker_host) = env::var("DOCKER_HOST") {
            // Use DOCKER_HOST if set
            debug!("Connecting to Docker via DOCKER_HOST: {}", docker_host);
            Docker::connect_with_socket(
                &docker_host,
                DOCKER_CONNECT_TIMEOUT_SECS,
                bollard::API_DEFAULT_VERSION,
            )
            .map_err(|e| {
                DockerError::ConnectionFailed(format!(
                    "Failed to connect to Docker daemon at {}: {}",
                    docker_host, e
                ))
            })?
        } else if let Ok(home) = env::var("HOME") {
            // Try Colima sockets (new location first, then old)
            let colima_socket_new = format!("{}/.config/colima/default/docker.sock", home);
            let colima_socket_old = format!("{}/.colima/default/docker.sock", home);

            if Path::new(&colima_socket_new).exists() {
                debug!(
                    "Connecting to Docker via Colima socket (new): {}",
                    colima_socket_new
                );
                Docker::connect_with_socket(
                    &format!("unix://{}", colima_socket_new),
                    DOCKER_CONNECT_TIMEOUT_SECS,
                    bollard::API_DEFAULT_VERSION,
                )
                .map_err(|e| {
                    DockerError::ConnectionFailed(format!(
                        "Failed to connect to Colima Docker daemon: {}",
                        e
                    ))
                })?
            } else if Path::new(&colima_socket_old).exists() {
                debug!(
                    "Connecting to Docker via Colima socket (old): {}",
                    colima_socket_old
                );
                Docker::connect_with_socket(
                    &format!("unix://{}", colima_socket_old),
                    DOCKER_CONNECT_TIMEOUT_SECS,
                    bollard::API_DEFAULT_VERSION,
                )
                .map_err(|e| {
                    DockerError::ConnectionFailed(format!(
                        "Failed to connect to Colima Docker daemon: {}",
                        e
                    ))
                })?
            } else {
                // Fall back to default connection
                debug!("Connecting to Docker via local defaults");
                Docker::connect_with_local_defaults().map_err(|e| {
                    DockerError::ConnectionFailed(format!(
                        "Failed to connect to Docker daemon: {}. \
                        Make sure Docker is running and accessible.",
                        e
                    ))
                })?
            }
        } else {
            // No HOME, use default connection
            debug!("Connecting to Docker via local defaults (no HOME)");
            Docker::connect_with_local_defaults().map_err(|e| {
                DockerError::ConnectionFailed(format!(
                    "Failed to connect to Docker daemon: {}. \
                    Make sure Docker is running and accessible.",
                    e
                ))
            })?
        };

        let max = config.sandbox.max_concurrent;
        let concurrency = Arc::new(Semaphore::new(max));
        let persistent_permits = Arc::new(Mutex::new(HashMap::new()));

        Ok(Self {
            docker,
            config,
            concurrency,
            persistent_permits,
        })
    }

    /// Create a Docker service with default configuration
    pub async fn with_defaults() -> Result<Self, DockerError> {
        Self::new(DockerConfig::default()).await
    }

    fn try_acquire_concurrency_permit(&self) -> Result<OwnedSemaphorePermit, DockerError> {
        self.concurrency
            .clone()
            .try_acquire_owned()
            .map_err(|_| DockerError::ConcurrencyLimitReached)
    }

    async fn store_persistent_permit(&self, container_id: String, permit: OwnedSemaphorePermit) {
        let mut map = self.persistent_permits.lock().await;
        map.insert(container_id, permit);
    }

    async fn take_persistent_permit(&self, container_id: &str) -> Option<OwnedSemaphorePermit> {
        let mut map = self.persistent_permits.lock().await;
        map.remove(container_id)
    }

    /// Get the full image name (name:tag)
    fn image_full_name(&self) -> String {
        format!("{}:{}", self.config.image_name, self.config.image_tag)
    }

    fn default_sandbox_env(&self) -> HashMap<String, String> {
        let pv = self.config.sandbox.python_version.clone();
        [
            ("HOME".to_string(), "/workspace".to_string()),
            (
                "PATH".to_string(),
                "/usr/local/bin:/usr/bin:/bin".to_string(),
            ),
            (
                "XDG_CACHE_HOME".to_string(),
                "/workspace/.cache".to_string(),
            ),
            (
                "UV_CACHE_DIR".to_string(),
                "/workspace/.cache/uv".to_string(),
            ),
            ("SANDY_IN_DOCKER".to_string(), "1".to_string()),
            ("UV_PYTHON".to_string(), pv),
            (
                "UV_PYTHON_INSTALL_DIR".to_string(),
                DOCKER_UV_PYTHON_INSTALL_DIR.to_string(),
            ),
            (
                "npm_config_cache".to_string(),
                "/workspace/.cache/npm".to_string(),
            ),
            (
                "npm_config_update_notifier".to_string(),
                "false".to_string(),
            ),
            ("npm_config_fund".to_string(), "false".to_string()),
            ("npm_config_audit".to_string(), "false".to_string()),
            ("LANG".to_string(), "C.UTF-8".to_string()),
            ("LC_ALL".to_string(), "C.UTF-8".to_string()),
        ]
        .into_iter()
        .collect()
    }

    fn merged_container_env(&self, env: Option<HashMap<String, String>>) -> Option<Vec<String>> {
        let mut map = self.default_sandbox_env();
        if let Some(user) = env {
            map.extend(user);
        }
        Some(
            map.into_iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect(),
        )
    }

    /// Validate bind-mount source paths for sandbox safety.
    fn validate_mount_sources(mounts: &[(String, String)]) -> Result<(), DockerError> {
        for (host_path, _) in mounts {
            let p = host_path.trim();
            if !p.starts_with('/') {
                return Err(DockerError::InvalidConfig(format!(
                    "Mount source must be an absolute path: {}",
                    host_path
                )));
            }
            if p.contains("/..") || p.ends_with("/..") || p == ".." {
                return Err(DockerError::InvalidConfig(format!(
                    "Mount source must not contain '..': {}",
                    host_path
                )));
            }
            let components: Vec<_> = Path::new(p).components().map(|c| c.as_os_str()).collect();
            if components.iter().any(|c| c == &std::ffi::OsStr::new("..")) {
                return Err(DockerError::InvalidConfig(format!(
                    "Mount source must not contain '..': {}",
                    host_path
                )));
            }
            const SENSITIVE: &[&str] = &["/etc", "/proc", "/sys", "/dev", "/root", "/home"];
            let norm = p.trim_end_matches('/');
            for prefix in SENSITIVE {
                if norm == *prefix || norm.starts_with(&format!("{}/", prefix)) {
                    return Err(DockerError::InvalidConfig(format!(
                        "Mount source is not allowed under {}: {}",
                        prefix, host_path
                    )));
                }
            }
        }
        Ok(())
    }

    fn build_host_config(
        &self,
        mounts: Option<Vec<(String, String)>>,
    ) -> Result<bollard::models::HostConfig, DockerError> {
        use bollard::models::{HostConfig, Mount, MountType};

        let sandbox = &self.config.sandbox;
        let docker_mounts: Option<Vec<Mount>> = if let Some(mounts) = mounts {
            Self::validate_mount_sources(&mounts)?;
            Some(
                mounts
                    .into_iter()
                    .map(|(host_path, container_path)| Mount {
                        target: Some(container_path),
                        source: Some(host_path),
                        typ: Some(MountType::BIND),
                        read_only: Some(false),
                        ..Default::default()
                    })
                    .collect(),
            )
        } else {
            None
        };

        let mut tmpfs: HashMap<String, String> = HashMap::new();
        // Explicit `exec` so interpreters under cache dirs remain executable if the daemon defaults differ.
        tmpfs.insert(
            "/tmp".to_string(),
            "rw,exec,nosuid,size=67108864,mode=1777".to_string(),
        );
        // Root of tmpfs defaults to root:root; sandbox user in the image is uid/gid 1000 (see Dockerfile).
        tmpfs.insert(
            "/workspace".to_string(),
            "rw,exec,nosuid,size=134217728,uid=1000,gid=1000,mode=755".to_string(),
        );

        Ok(HostConfig {
            memory: Some(sandbox.memory_bytes),
            memory_swap: Some(sandbox.memory_swap_bytes),
            nano_cpus: Some(sandbox.nano_cpus),
            pids_limit: Some(sandbox.pids_limit),
            network_mode: if sandbox.network_disabled {
                Some("none".to_string())
            } else {
                None
            },
            readonly_rootfs: Some(sandbox.readonly_rootfs),
            tmpfs: Some(tmpfs),
            cap_drop: Some(vec!["ALL".to_string()]),
            security_opt: Some(vec!["no-new-privileges:true".to_string()]),
            mounts: docker_mounts,
            ..Default::default()
        })
    }

    fn extend_output_truncated(buf: &mut Vec<u8>, chunk: &[u8], max: usize) -> bool {
        let remaining = max.saturating_sub(buf.len());
        if remaining == 0 {
            if !buf
                .windows(OUTPUT_TRUNCATED_MARKER.len())
                .any(|w| w == OUTPUT_TRUNCATED_MARKER.as_bytes())
            {
                buf.extend_from_slice(OUTPUT_TRUNCATED_MARKER.as_bytes());
            }
            return true;
        }
        if chunk.len() <= remaining {
            buf.extend_from_slice(chunk);
            false
        } else {
            buf.extend_from_slice(&chunk[..remaining]);
            buf.extend_from_slice(OUTPUT_TRUNCATED_MARKER.as_bytes());
            true
        }
    }

    async fn kill_container_force(&self, container_id: &str) {
        let _ = self
            .docker
            .kill_container(
                container_id,
                Some(
                    KillContainerOptionsBuilder::default()
                        .signal("SIGKILL")
                        .build(),
                ),
            )
            .await;
    }

    /// Best-effort: kill a stuck exec process by PID (persistent containers).
    async fn kill_exec_process(&self, container_id: &str, exec_id: &str) {
        let inspect = self.docker.inspect_exec(exec_id).await.ok();
        if let Some(pid) = inspect.and_then(|i| i.pid).filter(|p| *p > 0) {
            let cmd = vec![
                "sh".to_string(),
                "-c".to_string(),
                format!("kill -9 {} 2>/dev/null || true", pid),
            ];
            let exec_config = CreateExecOptions {
                cmd: Some(cmd),
                attach_stdout: Some(false),
                attach_stderr: Some(false),
                ..Default::default()
            };
            if let Ok(exec_result) = self.docker.create_exec(container_id, exec_config).await {
                let _ = self.docker.start_exec(&exec_result.id, None).await;
            }
        }
    }

    async fn wait_for_container_exit_or_kill(&self, container_id: &str) -> Result<(), DockerError> {
        let timeout_secs = self.config.sandbox.timeout_secs;
        let deadline = time::Instant::now() + time::Duration::from_secs(timeout_secs);

        let wait_result = time::timeout(time::Duration::from_secs(timeout_secs), async {
            let mut wait_stream = self.docker.wait_container(container_id, None);
            match wait_stream.next().await {
                Some(Ok(_)) => Ok::<bool, DockerError>(true),
                Some(Err(e)) => {
                    warn!(
                        "Container wait stream error: {}, will check container state directly",
                        e
                    );
                    Ok::<bool, DockerError>(false)
                }
                None => Ok::<bool, DockerError>(false),
            }
        })
        .await;

        if wait_result.is_err() {
            self.kill_container_force(container_id).await;
            return Err(DockerError::ExecutionTimedOut(timeout_secs));
        }

        // Wait stream reported the container finished successfully.
        if matches!(wait_result, Ok(Ok(true))) {
            return Ok(());
        }

        // Stream ended or errored — poll until exit or deadline.
        loop {
            if time::Instant::now() >= deadline {
                self.kill_container_force(container_id).await;
                return Err(DockerError::ExecutionTimedOut(timeout_secs));
            }

            let inspect_result = self
                .docker
                .inspect_container(container_id, None)
                .await
                .map_err(|e| {
                    DockerError::ExecutionFailed(format!("Failed to inspect container: {}", e))
                })?;

            if let Some(state) = inspect_result.state {
                if matches!(
                    state.status,
                    Some(
                        bollard::models::ContainerStateStatusEnum::EXITED
                            | bollard::models::ContainerStateStatusEnum::DEAD
                    )
                ) {
                    break;
                }
            }

            time::sleep(time::Duration::from_millis(POLL_INTERVAL_MS)).await;
        }

        Ok(())
    }

    /// Check if the Docker image exists
    pub async fn image_exists(&self) -> Result<bool, DockerError> {
        let image_name = self.image_full_name();

        // Try inspect_image first (most reliable)
        match self.docker.inspect_image(&image_name).await {
            Ok(_) => Ok(true),
            Err(e) => {
                let error_msg = e.to_string().to_lowercase();

                // If it's a "not found" error, try fallback method
                if error_msg.contains("not found")
                    || error_msg.contains("no such image")
                    || error_msg.contains("404")
                {
                    // Fallback: list all images and check repo_tags
                    let list_options = ListImagesOptionsBuilder::default().all(true).build();

                    let images =
                        self.docker
                            .list_images(Some(list_options))
                            .await
                            .map_err(|e| {
                                DockerError::ImageNotFound(format!("Failed to list images: {}", e))
                            })?;

                    debug!("Looking for image: '{}'", image_name);
                    debug!("Checking {} images...", images.len());

                    // Check if any image has the tag we're looking for
                    let exists = images.iter().any(|img| {
                        // Check repo_tags (may be empty for some images)
                        if !img.repo_tags.is_empty() {
                            img.repo_tags.iter().any(|tag| {
                                let matches = tag == &image_name
                                    || tag == &self.config.image_name
                                    || tag.starts_with(&format!("{}:", self.config.image_name))
                                    || (tag.split(':').next().unwrap_or("")
                                        == self.config.image_name);
                                if matches {
                                    debug!("Found matching image: '{}'", tag);
                                }
                                matches
                            })
                        } else {
                            false
                        }
                    });

                    if !exists {
                        warn!(
                            "Image '{}' not found in {} listed images",
                            image_name,
                            images.len()
                        );

                        // Last resort: try to inspect by ID or try creating a container
                        // Sometimes images exist but aren't in list_images
                        debug!("Attempting direct container creation test...");
                        let test_config = ContainerCreateBody {
                            image: Some(image_name.clone()),
                            cmd: Some(vec!["echo".to_string(), "test".to_string()]),
                            ..Default::default()
                        };
                        match self.docker.create_container(None, test_config).await {
                            Ok(container) => {
                                // Image exists! Clean up the test container
                                let _ = self
                                    .docker
                                    .remove_container(
                                        &container.id,
                                        Some(
                                            RemoveContainerOptionsBuilder::default()
                                                .force(true)
                                                .build(),
                                        ),
                                    )
                                    .await;
                                debug!("Image exists (verified by test container creation)");
                                return Ok(true);
                            }
                            Err(e) => {
                                let err_msg = e.to_string().to_lowercase();
                                if err_msg.contains("no such image")
                                    || err_msg.contains("not found")
                                {
                                    debug!("Image confirmed not found via container creation test");
                                } else {
                                    // Other error might mean image exists but can't create container
                                    warn!("Container creation test error (might indicate image exists): {}", e);
                                }
                            }
                        }
                    }

                    Ok(exists)
                } else {
                    // For other errors (connection issues, etc.), return an error
                    Err(DockerError::ImageNotFound(format!(
                        "Failed to check if image exists: {}",
                        e
                    )))
                }
            }
        }
    }

    /// Build the runtime image using the Docker CLI (`docker build`), using `dockerfile_path`,
    /// `build_context`, and `image_name` / `image_tag` from [`DockerConfig`].
    ///
    /// Stdout/stderr are inherited so build progress is visible in the terminal.
    pub async fn build_image_via_cli(&self, python_version: &str) -> Result<(), DockerError> {
        let full_name = self.image_full_name();
        info!(
            "docker build --build-arg PYTHON_VERSION={} -f {} -t {} {}",
            python_version, self.config.dockerfile_path, full_name, self.config.build_context
        );

        let status = TokioCommand::new("docker")
            .arg("build")
            .arg("--build-arg")
            .arg(format!("PYTHON_VERSION={}", python_version))
            .arg("-f")
            .arg(&self.config.dockerfile_path)
            .arg("-t")
            .arg(&full_name)
            .arg(&self.config.build_context)
            .stdin(Stdio::null())
            .status()
            .await
            .map_err(|e| {
                DockerError::BuildFailed(format!(
                    "Failed to spawn `docker build` (is the Docker CLI installed and on PATH?): {e}"
                ))
            })?;

        if !status.success() {
            return Err(DockerError::BuildFailed(format!(
                "`docker build` exited with status {status}"
            )));
        }

        Ok(())
    }

    /// Create a container with the specified command
    pub async fn create_container(
        &self,
        command: Vec<String>,
        env: Option<HashMap<String, String>>,
    ) -> Result<String, DockerError> {
        self.create_container_with_mounts(command, env, None).await
    }

    async fn create_container_inner(
        &self,
        command: Vec<String>,
        env: Option<HashMap<String, String>>,
        mounts: Option<Vec<(String, String)>>,
        user_override: Option<String>,
    ) -> Result<String, DockerError> {
        let image_name = self.image_full_name();
        let container_name = format!("{}_{}", self.config.image_name, uuid::Uuid::new_v4());

        let host_config = Some(self.build_host_config(mounts)?);

        let user = user_override.unwrap_or_else(|| self.config.sandbox.container_user.clone());

        let container_config = ContainerCreateBody {
            image: Some(image_name),
            cmd: Some(command),
            env: self.merged_container_env(env),
            user: Some(user),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            host_config,
            ..Default::default()
        };

        let options = CreateContainerOptionsBuilder::default()
            .name(&container_name)
            .build();

        let result = self
            .docker
            .create_container(Some(options), container_config)
            .await
            .map_err(|e| {
                DockerError::ContainerCreationFailed(format!("Failed to create container: {}", e))
            })?;

        Ok(result.id)
    }

    /// Create a container with the specified command and volume mounts.
    /// Acquires a concurrency slot until the container is removed via [`remove_container`].
    pub async fn create_container_with_mounts(
        &self,
        command: Vec<String>,
        env: Option<HashMap<String, String>>,
        mounts: Option<Vec<(String, String)>>,
    ) -> Result<String, DockerError> {
        let permit = self.try_acquire_concurrency_permit()?;
        let id = self
            .create_container_inner(command, env, mounts, None)
            .await?;
        self.store_persistent_permit(id.clone(), permit).await;
        Ok(id)
    }

    /// Start a container by ID
    pub async fn start_container(&self, container_id: &str) -> Result<(), DockerError> {
        self.docker
            .start_container(container_id, None)
            .await
            .map_err(|e| {
                DockerError::ContainerStartFailed(format!("Failed to start container: {}", e))
            })?;

        Ok(())
    }

    /// Check if a container is running
    pub async fn is_container_running(&self, container_id: &str) -> Result<bool, DockerError> {
        let inspect_result = self
            .docker
            .inspect_container(container_id, None)
            .await
            .map_err(|e| {
                DockerError::ExecutionFailed(format!("Failed to inspect container: {}", e))
            })?;

        if let Some(state) = inspect_result.state {
            if let Some(status) = &state.status {
                return Ok(matches!(
                    status,
                    bollard::models::ContainerStateStatusEnum::RUNNING
                ));
            }
        }

        Ok(false)
    }

    /// Ensure a container is running, restarting it if necessary
    pub async fn ensure_container_running(&self, container_id: &str) -> Result<(), DockerError> {
        let is_running = self.is_container_running(container_id).await?;

        if !is_running {
            // Container is not running - try to start it
            // This works for both stopped and exited containers
            debug!(
                "Container {} is not running, attempting to start it...",
                container_id
            );
            match self.start_container(container_id).await {
                Ok(_) => {
                    // Give it a moment to start
                    time::sleep(time::Duration::from_millis(POLL_INTERVAL_MS)).await;
                    // Verify it's actually running now
                    let is_running_after = self.is_container_running(container_id).await?;
                    if !is_running_after {
                        return Err(DockerError::ContainerStartFailed(format!(
                            "Container {} started but is not running",
                            container_id
                        )));
                    }
                    debug!("Container {} successfully started", container_id);
                }
                Err(e) => {
                    // Check if container exists at all
                    let inspect_result = self.docker.inspect_container(container_id, None).await;
                    match inspect_result {
                        Ok(_) => {
                            // Container exists but failed to start
                            return Err(DockerError::ContainerStartFailed(format!(
                                "Failed to restart container {}: {}",
                                container_id, e
                            )));
                        }
                        Err(_) => {
                            // Container doesn't exist
                            return Err(DockerError::ContainerStartFailed(format!(
                                "Container {} does not exist. It may have been removed.",
                                container_id
                            )));
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Execute a command in a container and wait for completion
    pub async fn exec_in_container(
        &self,
        container_id: &str,
        command: Vec<String>,
    ) -> Result<(String, String, i32), DockerError> {
        self.exec_in_container_with_env(container_id, command, None)
            .await
    }

    pub async fn exec_in_container_with_env(
        &self,
        container_id: &str,
        command: Vec<String>,
        env: Option<Vec<String>>,
    ) -> Result<(String, String, i32), DockerError> {
        let exec_config = CreateExecOptions {
            cmd: Some(command),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            env,
            user: Some(self.config.sandbox.container_user.clone()),
            ..Default::default()
        };

        let exec_result = self
            .docker
            .create_exec(container_id, exec_config)
            .await
            .map_err(|e| DockerError::ExecutionFailed(format!("Failed to create exec: {}", e)))?;

        let exec_response = self
            .docker
            .start_exec(&exec_result.id, None)
            .await
            .map_err(|e| DockerError::ExecutionFailed(format!("Failed to start exec: {}", e)))?;

        let max_out = self.config.sandbox.max_output_bytes;
        let timeout_secs = self.config.sandbox.timeout_secs;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut truncated = false;

        match exec_response {
            StartExecResults::Attached {
                mut output,
                input: _,
            } => {
                let read_stream = async {
                    while let Some(result) = output.next().await {
                        match result {
                            Ok(LogOutput::StdOut { message }) => {
                                if Self::extend_output_truncated(&mut stdout, &message, max_out) {
                                    truncated = true;
                                    break;
                                }
                            }
                            Ok(LogOutput::StdErr { message }) => {
                                if Self::extend_output_truncated(&mut stderr, &message, max_out) {
                                    truncated = true;
                                    break;
                                }
                            }
                            Ok(LogOutput::Console { message }) => {
                                if Self::extend_output_truncated(&mut stdout, &message, max_out) {
                                    truncated = true;
                                    break;
                                }
                            }
                            Ok(LogOutput::StdIn { .. }) => {}
                            Err(e) => {
                                return Err(DockerError::ExecutionFailed(format!(
                                    "Exec stream error: {}",
                                    e
                                )));
                            }
                        }
                    }
                    Ok::<(), DockerError>(())
                };

                match time::timeout(time::Duration::from_secs(timeout_secs), read_stream).await {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => return Err(e),
                    Err(_) => {
                        self.kill_exec_process(container_id, &exec_result.id).await;
                        return Err(DockerError::ExecutionTimedOut(timeout_secs));
                    }
                }
                if truncated {
                    return Err(DockerError::OutputTruncated(max_out));
                }
            }
            StartExecResults::Detached => {
                time::sleep(time::Duration::from_millis(100)).await;
            }
        }

        // Get exit code
        let inspect_result = self
            .docker
            .inspect_exec(&exec_result.id)
            .await
            .map_err(|e| DockerError::ExecutionFailed(format!("Failed to inspect exec: {}", e)))?;

        let exit_code = inspect_result
            .exit_code
            .unwrap_or(-1)
            .try_into()
            .unwrap_or(-1);

        Ok((
            String::from_utf8_lossy(&stdout).to_string(),
            String::from_utf8_lossy(&stderr).to_string(),
            exit_code,
        ))
    }

    /// Run a command in a new container and return the output
    pub async fn run_command(
        &self,
        command: Vec<String>,
        env: Option<HashMap<String, String>>,
    ) -> Result<(String, String, i32), DockerError> {
        self.run_command_with_mounts(command, env, None).await
    }

    /// Run a command in a new container with volume mounts and return the output
    pub async fn run_command_with_mounts(
        &self,
        command: Vec<String>,
        env: Option<HashMap<String, String>>,
        mounts: Option<Vec<(String, String)>>,
    ) -> Result<(String, String, i32), DockerError> {
        self.run_command_with_mounts_and_cleanup(command, env, mounts, true)
            .await
    }

    /// Run a command in a new container with volume mounts and return the output
    /// If `remove_after` is false, the container will be kept running and the container ID
    /// will be included in the error message (as a workaround since we can't change the return type)
    pub async fn run_command_with_mounts_and_cleanup(
        &self,
        command: Vec<String>,
        env: Option<HashMap<String, String>>,
        mounts: Option<Vec<(String, String)>>,
        remove_after: bool,
    ) -> Result<(String, String, i32), DockerError> {
        let permit = self.try_acquire_concurrency_permit()?;
        let mut permit_holder = Some(permit);

        let container_id = self
            .create_container_inner(command.clone(), env, mounts, None)
            .await?;
        if let Err(e) = self.start_container(&container_id).await {
            let _ = self
                .docker
                .remove_container(
                    &container_id,
                    Some(RemoveContainerOptionsBuilder::default().force(true).build()),
                )
                .await;
            drop(permit_holder.take());
            return Err(e);
        }

        let wait_result = self.wait_for_container_exit_or_kill(&container_id).await;

        if let Err(e) = wait_result {
            let _ = self.remove_container(&container_id, true).await;
            drop(permit_holder.take());
            return Err(e);
        }

        // Get logs
        let logs_options = LogsOptionsBuilder::default()
            .stdout(true)
            .stderr(true)
            .build();

        let mut logs = self.docker.logs(&container_id, Some(logs_options));

        let max_out = self.config.sandbox.max_output_bytes;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        while let Some(chunk) = logs.next().await {
            match chunk {
                Ok(LogOutput::StdOut { message }) => {
                    if Self::extend_output_truncated(&mut stdout, &message, max_out) {
                        let _ = self.remove_container(&container_id, true).await;
                        drop(permit_holder.take());
                        return Err(DockerError::OutputTruncated(max_out));
                    }
                }
                Ok(LogOutput::StdErr { message }) => {
                    if Self::extend_output_truncated(&mut stderr, &message, max_out) {
                        let _ = self.remove_container(&container_id, true).await;
                        drop(permit_holder.take());
                        return Err(DockerError::OutputTruncated(max_out));
                    }
                }
                Ok(LogOutput::Console { message }) => {
                    if Self::extend_output_truncated(&mut stdout, &message, max_out) {
                        let _ = self.remove_container(&container_id, true).await;
                        drop(permit_holder.take());
                        return Err(DockerError::OutputTruncated(max_out));
                    }
                }
                Ok(LogOutput::StdIn { .. }) => {}
                Err(e) => {
                    let _ = self.remove_container(&container_id, true).await;
                    drop(permit_holder.take());
                    return Err(DockerError::ExecutionFailed(format!(
                        "Log stream error: {}",
                        e
                    )));
                }
            }
        }

        // Get exit code
        let inspect_result = match self.docker.inspect_container(&container_id, None).await {
            Ok(r) => r,
            Err(e) => {
                let _ = self.remove_container(&container_id, true).await;
                drop(permit_holder.take());
                return Err(DockerError::ExecutionFailed(format!(
                    "Failed to inspect container: {}",
                    e
                )));
            }
        };

        let exit_code = inspect_result
            .state
            .and_then(|s| s.exit_code)
            .unwrap_or(-1)
            .try_into()
            .unwrap_or(-1);

        // Clean up container if requested
        if remove_after {
            let _ = self.remove_container(&container_id, true).await;
            // Ephemeral run: permit was never stored in the map; release it here.
            drop(permit_holder.take());
        } else {
            info!("Container {} kept alive (not removed)", container_id);
            if let Some(p) = permit_holder.take() {
                self.store_persistent_permit(container_id.clone(), p).await;
            }
        }

        Ok((
            String::from_utf8_lossy(&stdout).to_string(),
            String::from_utf8_lossy(&stderr).to_string(),
            exit_code,
        ))
    }

    /// Run a command in a new container with volume mounts and return the output and container ID
    /// The container is NOT removed, allowing it to be reused
    pub async fn run_command_with_mounts_keep_alive(
        &self,
        command: Vec<String>,
        env: Option<HashMap<String, String>>,
        mounts: Option<Vec<(String, String)>>,
    ) -> Result<(String, String, i32, String), DockerError> {
        let permit = self.try_acquire_concurrency_permit()?;

        let container_id = self
            .create_container_inner(command.clone(), env, mounts, None)
            .await?;
        if let Err(e) = self.start_container(&container_id).await {
            let _ = self
                .docker
                .remove_container(
                    &container_id,
                    Some(RemoveContainerOptionsBuilder::default().force(true).build()),
                )
                .await;
            return Err(e);
        }
        self.store_persistent_permit(container_id.clone(), permit)
            .await;

        let wait_result = self.wait_for_container_exit_or_kill(&container_id).await;

        if let Err(e) = wait_result {
            let _ = self.remove_container(&container_id, true).await;
            return Err(e);
        }

        // Get logs
        let logs_options = LogsOptionsBuilder::default()
            .stdout(true)
            .stderr(true)
            .build();

        let mut logs = self.docker.logs(&container_id, Some(logs_options));

        let max_out = self.config.sandbox.max_output_bytes;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        while let Some(chunk) = logs.next().await {
            match chunk {
                Ok(LogOutput::StdOut { message }) => {
                    if Self::extend_output_truncated(&mut stdout, &message, max_out) {
                        let _ = self.remove_container(&container_id, true).await;
                        return Err(DockerError::OutputTruncated(max_out));
                    }
                }
                Ok(LogOutput::StdErr { message }) => {
                    if Self::extend_output_truncated(&mut stderr, &message, max_out) {
                        let _ = self.remove_container(&container_id, true).await;
                        return Err(DockerError::OutputTruncated(max_out));
                    }
                }
                Ok(LogOutput::Console { message }) => {
                    if Self::extend_output_truncated(&mut stdout, &message, max_out) {
                        let _ = self.remove_container(&container_id, true).await;
                        return Err(DockerError::OutputTruncated(max_out));
                    }
                }
                Ok(LogOutput::StdIn { .. }) => {}
                Err(e) => {
                    let _ = self.remove_container(&container_id, true).await;
                    return Err(DockerError::ExecutionFailed(format!(
                        "Log stream error: {}",
                        e
                    )));
                }
            }
        }

        // Get exit code
        let inspect_result = self
            .docker
            .inspect_container(&container_id, None)
            .await
            .map_err(|e| {
                DockerError::ExecutionFailed(format!("Failed to inspect container: {}", e))
            })?;

        let exit_code = inspect_result
            .state
            .and_then(|s| s.exit_code)
            .unwrap_or(-1)
            .try_into()
            .unwrap_or(-1);

        // Container is NOT removed - caller is responsible for cleanup
        Ok((
            String::from_utf8_lossy(&stdout).to_string(),
            String::from_utf8_lossy(&stderr).to_string(),
            exit_code,
            container_id,
        ))
    }

    /// Remove a container
    pub async fn remove_container(
        &self,
        container_id: &str,
        force: bool,
    ) -> Result<(), DockerError> {
        let _ = self.take_persistent_permit(container_id).await;

        let options = RemoveContainerOptionsBuilder::default()
            .force(force)
            .build();

        self.docker
            .remove_container(container_id, Some(options))
            .await
            .map_err(|e| {
                DockerError::ExecutionFailed(format!("Failed to remove container: {}", e))
            })?;

        Ok(())
    }

    /// Execute Python code in a container
    pub async fn execute_python_code(
        &self,
        code: &str,
    ) -> Result<(String, String, i32), DockerError> {
        self.execute_python_code_with_mounts(code, None).await
    }

    /// Execute Python code in a container with volume mounts
    pub async fn execute_python_code_with_mounts(
        &self,
        code: &str,
        mounts: Option<Vec<(String, String)>>,
    ) -> Result<(String, String, i32), DockerError> {
        self.execute_python_code_with_mounts_and_cleanup(code, mounts, true)
            .await
    }

    /// Execute Python code in a container with volume mounts
    /// If `remove_after` is false, the container will be kept running
    pub async fn execute_python_code_with_mounts_and_cleanup(
        &self,
        code: &str,
        mounts: Option<Vec<(String, String)>>,
        remove_after: bool,
    ) -> Result<(String, String, i32), DockerError> {
        let command = vec![
            "exec-code".to_string(),
            "--code".to_string(),
            code.to_string(),
        ];

        self.run_command_with_mounts_and_cleanup(command, None, mounts, remove_after)
            .await
    }

    /// Execute JavaScript code in a container
    pub async fn execute_javascript_code(
        &self,
        code: &str,
    ) -> Result<(String, String, i32), DockerError> {
        self.execute_javascript_code_with_mounts(code, None).await
    }

    /// Execute JavaScript code in a container with volume mounts
    pub async fn execute_javascript_code_with_mounts(
        &self,
        code: &str,
        mounts: Option<Vec<(String, String)>>,
    ) -> Result<(String, String, i32), DockerError> {
        self.execute_javascript_code_with_mounts_and_cleanup(code, mounts, true)
            .await
    }

    /// Execute JavaScript code in a container with volume mounts
    /// If `remove_after` is false, the container will be kept running
    pub async fn execute_javascript_code_with_mounts_and_cleanup(
        &self,
        code: &str,
        mounts: Option<Vec<(String, String)>>,
        remove_after: bool,
    ) -> Result<(String, String, i32), DockerError> {
        let command = vec![
            "exec-js".to_string(),
            "--code".to_string(),
            code.to_string(),
        ];

        self.run_command_with_mounts_and_cleanup(command, None, mounts, remove_after)
            .await
    }

    /// Execute JavaScript code in an existing container using exec
    pub async fn execute_javascript_code_in_container(
        &self,
        container_id: &str,
        code: &str,
    ) -> Result<(String, String, i32), DockerError> {
        self.execute_javascript_code_in_container_with_env(container_id, code, None)
            .await
    }

    /// Execute JavaScript code in an existing container using exec with environment variables
    pub async fn execute_javascript_code_in_container_with_env(
        &self,
        container_id: &str,
        code: &str,
        env: Option<Vec<String>>,
    ) -> Result<(String, String, i32), DockerError> {
        self.ensure_container_running(container_id).await?;

        let command = vec![
            "sandy".to_string(),
            "exec-js".to_string(),
            "--code".to_string(),
            code.to_string(),
        ];

        self.exec_in_container_with_env(container_id, command, env)
            .await
    }

    /// Execute Python code in an existing container using exec
    /// This allows reusing a running container for multiple executions
    /// Automatically ensures the container is running before executing
    pub async fn execute_python_code_in_container(
        &self,
        container_id: &str,
        code: &str,
    ) -> Result<(String, String, i32), DockerError> {
        self.execute_python_code_in_container_with_env(container_id, code, None)
            .await
    }

    /// Execute Python code in an existing container using exec with environment variables
    pub async fn execute_python_code_in_container_with_env(
        &self,
        container_id: &str,
        code: &str,
        env: Option<Vec<String>>,
    ) -> Result<(String, String, i32), DockerError> {
        self.ensure_container_running(container_id).await?;

        let command = vec![
            "sandy".to_string(),
            "exec-code".to_string(),
            "--code".to_string(),
            code.to_string(),
        ];

        self.exec_in_container_with_env(container_id, command, env)
            .await
    }

    /// Create a long-running container that can be reused for multiple Python executions
    /// The container runs a sleep command to keep it alive
    /// Overrides the Dockerfile entrypoint to use /bin/sh since the image has ENTRYPOINT ["sandy"]
    pub async fn create_persistent_container(
        &self,
        mounts: Option<Vec<(String, String)>>,
    ) -> Result<String, DockerError> {
        let permit = self.try_acquire_concurrency_permit()?;

        let image_name = self.image_full_name();
        let container_name = format!("{}_{}", self.config.image_name, uuid::Uuid::new_v4());

        let host_config = Some(self.build_host_config(mounts)?);

        let container_config = ContainerCreateBody {
            image: Some(image_name),
            entrypoint: Some(vec!["/bin/sh".to_string()]),
            cmd: Some(vec!["-c".to_string(), "sleep infinity".to_string()]),
            user: Some(self.config.sandbox.container_user.clone()),
            env: self.merged_container_env(None),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            host_config,
            ..Default::default()
        };

        let options = CreateContainerOptionsBuilder::default()
            .name(&container_name)
            .build();

        let result = self
            .docker
            .create_container(Some(options), container_config)
            .await
            .map_err(|e| {
                DockerError::ContainerCreationFailed(format!("Failed to create container: {}", e))
            })?;

        let container_id = result.id;
        if let Err(e) = self.start_container(&container_id).await {
            let _ = self
                .docker
                .remove_container(
                    &container_id,
                    Some(RemoveContainerOptionsBuilder::default().force(true).build()),
                )
                .await;
            return Err(e);
        }
        self.store_persistent_permit(container_id.clone(), permit)
            .await;
        Ok(container_id)
    }

    /// Execute a Python script file in a container
    pub async fn execute_python_script(
        &self,
        script_path: &str,
    ) -> Result<(String, String, i32), DockerError> {
        let command = vec![
            "exec-code".to_string(),
            "--file".to_string(),
            script_path.to_string(),
        ];

        self.run_command(command, None).await
    }
}
