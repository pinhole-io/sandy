use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use super::command::run_command_with_timeout_and_output;
use super::error::SandboxError;
use super::policy::ExecPolicy;
use super::process::SupervisedProcess;
use super::result::OutputSink;
use super::CommandSandbox;
use super::ExecResult;
use async_trait::async_trait;
use tokio::process::Command;

static UNSHARE_NET_WORKS: OnceLock<AtomicBool> = OnceLock::new();
static BASIC_BWRAP_PROBE: Mutex<Option<BwrapProbe>> = Mutex::new(None);

#[derive(Clone)]
struct BwrapProbe {
    ok: bool,
    detail: String,
}

/// Build bubblewrap argv for unit tests and runtime.
///
/// `isolate_network` adds `--unshare-net` when true. Callers should pass
/// `!policy.allow_network && unshare_net_supported()` so hosts that cannot
/// configure loopback in a new net namespace (common in nested containers)
/// still get filesystem isolation.
///
/// `isolate_pid` adds `--unshare-pid --as-pid-1` when true so children die
/// with the sandbox. `--die-with-parent` is always set.
pub fn build_bwrap_argv(
    policy: &ExecPolicy,
    command: &str,
    isolate_network: bool,
    isolate_pid: bool,
) -> Result<Vec<String>, SandboxError> {
    build_bwrap_argv_with_read_only_roots(policy, &[], command, isolate_network, isolate_pid)
}

pub(super) fn build_bwrap_argv_with_read_only_roots(
    policy: &ExecPolicy,
    read_only_roots: &[PathBuf],
    command: &str,
    isolate_network: bool,
    isolate_pid: bool,
) -> Result<Vec<String>, SandboxError> {
    if !Path::new("/").exists() {
        return Err(SandboxError::InvalidPolicy("host root missing".into()));
    }

    let mut args = vec!["bwrap".to_string()];
    push_runtime_mounts(&mut args);

    let cwd = policy
        .cwd
        .canonicalize()
        .map_err(|e| SandboxError::InvalidPolicy(format!("cwd {}: {e}", policy.cwd.display())))?;

    let mut writable_hosts = Vec::with_capacity(policy.writable_roots.len());
    for root in &policy.writable_roots {
        let host = root.canonicalize().map_err(|e| {
            SandboxError::InvalidPolicy(format!("writable root {}: {e}", root.display()))
        })?;
        if host == Path::new("/") {
            return Err(SandboxError::InvalidPolicy(
                "host root cannot be writable".into(),
            ));
        }
        let s = host.display().to_string();
        args.push("--bind".into());
        args.push(s.clone());
        args.push(s);
        // Extra gitdirs are writable so fetch can update FETCH_HEAD. Hide hooks
        // so a sandboxed command cannot persist a hook into the host gitdir.
        if host != cwd && host.join("HEAD").is_file() {
            args.push("--tmpfs".into());
            args.push(format!("{}/hooks", host.display()));
        }
        writable_hosts.push(host);
    }

    for root in read_only_roots {
        let host = root.canonicalize().map_err(|error| {
            SandboxError::InvalidPolicy(format!("read-only root {}: {error}", root.display()))
        })?;
        if host == Path::new("/") {
            return Err(SandboxError::InvalidPolicy(
                "host root cannot be mounted".into(),
            ));
        }
        if writable_hosts
            .iter()
            .any(|writable| host.starts_with(writable) || writable.starts_with(&host))
        {
            return Err(SandboxError::InvalidPolicy(
                "read-only and writable roots overlap".into(),
            ));
        }
        let path = host.display().to_string();
        args.extend(["--ro-bind".into(), path.clone(), path]);
    }

    args.push("--dev".into());
    args.push("/dev".into());
    args.push("--proc".into());
    args.push("/proc".into());
    args.push("--tmpfs".into());
    args.push("/tmp".into());

    if isolate_network {
        args.push("--unshare-net".into());
    }

    args.push("--die-with-parent".into());
    if isolate_pid {
        args.push("--unshare-pid".into());
        args.push("--as-pid-1".into());
    }

    args.push("--chdir".into());
    args.push(cwd.display().to_string());

    args.push("--".into());
    args.push("sh".into());
    args.push("-c".into());
    args.push(command.to_string());
    Ok(args)
}

fn unshare_net_flag() -> &'static AtomicBool {
    UNSHARE_NET_WORKS.get_or_init(|| AtomicBool::new(probe_unshare_net()))
}

/// Whether `bwrap --unshare-net` works on this host (cached after first probe).
pub fn unshare_net_supported() -> bool {
    unshare_net_flag().load(Ordering::Relaxed)
}

static UNSHARE_PID_WORKS: OnceLock<AtomicBool> = OnceLock::new();

fn unshare_pid_flag() -> &'static AtomicBool {
    UNSHARE_PID_WORKS.get_or_init(|| AtomicBool::new(probe_unshare_pid()))
}

/// Whether `bwrap --unshare-pid --as-pid-1` works on this host.
pub fn unshare_pid_supported() -> bool {
    unshare_pid_flag().load(Ordering::Relaxed)
}

fn probe_unshare_pid() -> bool {
    run_probe(false, true).is_ok_and(|output| output.status.success())
}

fn probe_unshare_net() -> bool {
    run_probe(true, false).is_ok_and(|output| output.status.success())
}

fn basic_bwrap_supported() -> bool {
    basic_bwrap_probe().ok
}

fn probe_lock() -> std::sync::MutexGuard<'static, Option<BwrapProbe>> {
    BASIC_BWRAP_PROBE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn basic_bwrap_probe() -> BwrapProbe {
    let mut slot = probe_lock();
    if let Some(probe) = slot.as_ref() {
        return probe.clone();
    }
    let probe = probe_basic_bwrap();
    *slot = Some(probe.clone());
    probe
}

/// Clear the cached bubblewrap probe so a later call can see a newly loaded profile.
pub fn reset_bwrap_probe() {
    *probe_lock() = None;
}

/// AppArmor profile that grants `/usr/bin/bwrap` a `userns` permission.
pub fn bwrap_apparmor_profile() -> &'static str {
    include_str!("../../contrib/apparmor/bwrap")
}

/// Whether the bubblewrap failure is the Ubuntu AppArmor uid_map denial.
pub fn needs_bwrap_apparmor() -> bool {
    reason_needs_apparmor(bwrap_unavailable_reason().as_deref().unwrap_or(""))
}

pub(super) fn reason_needs_apparmor(reason: &str) -> bool {
    reason.contains("uid map")
}

fn bounded_detail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let trimmed = text.trim();
    const LIMIT: usize = 512;
    if trimmed.len() <= LIMIT {
        return trimmed.to_string();
    }
    trimmed.chars().take(LIMIT).collect()
}

pub(super) fn format_bwrap_unavailable_detail(detail: &str) -> String {
    if detail.is_empty() {
        return "bwrap is missing or cannot create a sandbox".to_string();
    }
    if detail.contains("uid map") {
        return format!(
            "{detail}. Ubuntu 24.04+ AppArmor blocks unprivileged user namespaces for /usr/bin/bwrap until a userns profile is loaded (sandy contrib/apparmor/bwrap)."
        );
    }
    detail.to_string()
}

/// Why auto/bubblewrap cannot run on this host. `None` when bubblewrap works.
pub fn bwrap_unavailable_reason() -> Option<String> {
    if BubblewrapSandbox.is_available() {
        return None;
    }
    if !BubblewrapSandbox::bwrap_on_path() {
        return Some("bwrap not found on PATH".to_string());
    }
    let probe = basic_bwrap_probe();
    Some(format_bwrap_unavailable_detail(&probe.detail))
}

fn probe_basic_bwrap() -> BwrapProbe {
    if which_bwrap().is_none() {
        return BwrapProbe {
            ok: false,
            detail: "bwrap not found on PATH".to_string(),
        };
    }
    match run_probe(false, false) {
        Ok(output) if output.status.success() => BwrapProbe {
            ok: true,
            detail: String::new(),
        },
        Ok(output) => BwrapProbe {
            ok: false,
            detail: bounded_detail(&output.stderr),
        },
        Err(error) => BwrapProbe {
            ok: false,
            detail: error.to_string(),
        },
    }
}

fn run_probe(
    isolate_network: bool,
    isolate_pid: bool,
) -> Result<std::process::Output, std::io::Error> {
    let Some(bwrap) = which_bwrap() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "bwrap not found on PATH",
        ));
    };
    std::process::Command::new(bwrap)
        .args(build_probe_argv(isolate_network, isolate_pid))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .output()
}

fn build_probe_argv(isolate_network: bool, isolate_pid: bool) -> Vec<String> {
    let mut args = Vec::new();
    push_runtime_mounts(&mut args);
    args.extend([
        "--dev".into(),
        "/dev".into(),
        "--proc".into(),
        "/proc".into(),
        "--tmpfs".into(),
        "/tmp".into(),
    ]);
    if isolate_network {
        args.push("--unshare-net".into());
    }
    if isolate_pid {
        args.extend([
            "--die-with-parent".into(),
            "--unshare-pid".into(),
            "--as-pid-1".into(),
        ]);
    }
    args.extend(["--".into(), "sh".into(), "-c".into(), "true".into()]);
    args
}

fn push_runtime_mounts(args: &mut Vec<String>) {
    for path in ["/usr", "/bin", "/sbin", "/lib", "/lib64", "/etc"] {
        if Path::new(path).exists() {
            args.extend(["--ro-bind".into(), path.into(), path.into()]);
        }
    }
}

async fn spawn_bwrap(
    argv: &[String],
    timeout: Duration,
    policy: &ExecPolicy,
    on_output: Option<OutputSink>,
) -> Result<std::process::Output, SandboxError> {
    let program = argv.first().cloned().unwrap_or_else(|| "bwrap".into());
    let mut cmd = Command::new(program);
    for arg in argv.iter().skip(1) {
        cmd.arg(arg);
    }
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    super::env::apply_env(&mut cmd, &policy.env);
    for (key, value) in &policy.env_overrides {
        cmd.env(key, value);
    }
    run_command_with_timeout_and_output(cmd, timeout, policy.max_output_bytes, on_output).await
}

pub struct BubblewrapSandbox;

impl BubblewrapSandbox {
    pub fn bwrap_on_path() -> bool {
        which_bwrap().is_some()
    }

    /// Run a fixed command with additional host paths mounted read-only.
    ///
    /// The host root and paths that overlap a writable root are rejected.
    pub async fn run_shell_with_read_only_roots(
        &self,
        policy: &ExecPolicy,
        read_only_roots: &[PathBuf],
        command: &str,
        timeout: Duration,
    ) -> Result<ExecResult, SandboxError> {
        if command.is_empty() || command.contains('\0') {
            return Err(SandboxError::InvalidPolicy("invalid command".into()));
        }
        if !self.is_available() {
            return Err(SandboxError::Unavailable(
                "bubblewrap is unavailable".into(),
            ));
        }
        if !policy.allow_network && !unshare_net_supported() {
            return Err(SandboxError::Unavailable(
                "network namespace isolation is unavailable".into(),
            ));
        }
        if !unshare_pid_supported() {
            return Err(SandboxError::Unavailable(
                "process namespace isolation is unavailable".into(),
            ));
        }
        let argv = build_bwrap_argv_with_read_only_roots(
            policy,
            read_only_roots,
            command,
            !policy.allow_network,
            true,
        )?;
        let output = spawn_bwrap(&argv, timeout, policy, None).await?;
        Ok(ExecResult::from_output(output))
    }

    /// Start a process with both network and process namespace isolation.
    ///
    /// This method fails closed when either namespace is unavailable. The
    /// returned handle owns the process until it exits or the handle drops.
    pub async fn start_shell_process(
        &self,
        policy: &ExecPolicy,
        command: &str,
    ) -> Result<SupervisedProcess, SandboxError> {
        if command.is_empty() || command.contains('\0') {
            return Err(SandboxError::InvalidPolicy("invalid command".into()));
        }
        if !self.is_available() {
            return Err(SandboxError::Unavailable(
                "bubblewrap is unavailable".into(),
            ));
        }
        if !unshare_net_supported() {
            return Err(SandboxError::Unavailable(
                "network namespace isolation is unavailable".into(),
            ));
        }
        if !unshare_pid_supported() {
            return Err(SandboxError::Unavailable(
                "process namespace isolation is unavailable".into(),
            ));
        }
        let argv = build_bwrap_argv(policy, command, true, true)?;
        super::process::spawn_bwrap(&argv, policy).await
    }
}

/// Bubblewrap backend that exposes only the workspace and runtime directories.
///
/// The caller must provide a workspace as `cwd`. Every writable root must
/// remain below that directory. The host root is never mounted.
pub struct WorkspaceBubblewrapSandbox;

impl WorkspaceBubblewrapSandbox {
    pub async fn start_shell_process(
        &self,
        policy: &ExecPolicy,
        command: &str,
    ) -> Result<SupervisedProcess, SandboxError> {
        let policy = workspace_policy(policy)?;
        validate_isolated_backend(command)?;
        let argv = build_bwrap_argv(&policy, command, true, true)?;
        super::process::spawn_bwrap(&argv, &policy).await
    }
}

#[async_trait]
impl CommandSandbox for WorkspaceBubblewrapSandbox {
    fn id(&self) -> &'static str {
        "workspace-bubblewrap"
    }

    fn is_available(&self) -> bool {
        BubblewrapSandbox.is_available() && unshare_net_supported() && unshare_pid_supported()
    }

    async fn run_shell(
        &self,
        policy: &ExecPolicy,
        command: &str,
        timeout: Duration,
    ) -> Result<ExecResult, SandboxError> {
        let policy = workspace_policy(policy)?;
        validate_isolated_backend(command)?;
        match super::warm::run_warm(&policy, &[], command, timeout, true).await {
            Ok(result) => Ok(result),
            Err(SandboxError::Unavailable(_)) => {
                let argv = build_bwrap_argv(&policy, command, true, true)?;
                let output = spawn_bwrap(&argv, timeout, &policy, None).await?;
                Ok(ExecResult::from_output(output))
            }
            Err(error) => Err(error),
        }
    }

    async fn run_shell_streaming(
        &self,
        policy: &ExecPolicy,
        command: &str,
        timeout: Duration,
        on_output: Option<OutputSink>,
    ) -> Result<ExecResult, SandboxError> {
        let policy = workspace_policy(policy)?;
        validate_isolated_backend(command)?;
        let argv = build_bwrap_argv(&policy, command, true, true)?;
        let output = spawn_bwrap(&argv, timeout, &policy, on_output).await?;
        Ok(ExecResult::from_output(output))
    }
}

fn validate_isolated_backend(command: &str) -> Result<(), SandboxError> {
    if command.is_empty() || command.contains('\0') {
        return Err(SandboxError::InvalidPolicy("invalid command".into()));
    }
    if !WorkspaceBubblewrapSandbox.is_available() {
        return Err(SandboxError::Unavailable(
            "workspace bubblewrap isolation is unavailable".into(),
        ));
    }
    Ok(())
}

fn workspace_policy(policy: &ExecPolicy) -> Result<ExecPolicy, SandboxError> {
    let cwd = policy
        .cwd
        .canonicalize()
        .map_err(|error| SandboxError::InvalidPolicy(format!("workspace: {error}")))?;
    if !cwd.is_dir() {
        return Err(SandboxError::InvalidPolicy(
            "workspace is not a directory".into(),
        ));
    }
    let mut writable_roots = Vec::with_capacity(policy.writable_roots.len().max(1));
    for root in &policy.writable_roots {
        let root = root
            .canonicalize()
            .map_err(|error| SandboxError::InvalidPolicy(format!("writable root: {error}")))?;
        if !root.starts_with(&cwd) {
            return Err(SandboxError::InvalidPolicy(
                "writable root is outside workspace".into(),
            ));
        }
        writable_roots.push(root);
    }
    if !writable_roots.iter().any(|root| root == &cwd) {
        writable_roots.push(cwd.clone());
    }
    Ok(ExecPolicy {
        cwd,
        writable_roots,
        allow_network: false,
        env: policy.env.clone(),
        max_output_bytes: policy.max_output_bytes,

        env_overrides: Vec::new(),
    })
}

fn which_bwrap() -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let candidate = dir.join("bwrap");
            candidate.is_file().then_some(candidate)
        })
    })
}

#[async_trait]
impl CommandSandbox for BubblewrapSandbox {
    fn id(&self) -> &'static str {
        "bubblewrap"
    }

    fn is_available(&self) -> bool {
        Self::bwrap_on_path() && basic_bwrap_supported()
    }

    async fn run_shell(
        &self,
        policy: &ExecPolicy,
        command: &str,
        timeout: Duration,
    ) -> Result<ExecResult, SandboxError> {
        if !policy.allow_network && !unshare_net_supported() {
            return Err(SandboxError::Unavailable(
                "network namespace isolation is unavailable".into(),
            ));
        }
        if !unshare_pid_supported() {
            return Err(SandboxError::Unavailable(
                "process namespace isolation is unavailable".into(),
            ));
        }
        let argv = build_bwrap_argv(policy, command, !policy.allow_network, true)?;
        let output = spawn_bwrap(&argv, timeout, policy, None).await?;
        Ok(ExecResult::from_output(output))
    }

    async fn run_shell_streaming(
        &self,
        policy: &ExecPolicy,
        command: &str,
        timeout: Duration,
        on_output: Option<OutputSink>,
    ) -> Result<ExecResult, SandboxError> {
        if !policy.allow_network && !unshare_net_supported() {
            return Err(SandboxError::Unavailable(
                "network namespace isolation is unavailable".into(),
            ));
        }
        if !unshare_pid_supported() {
            return Err(SandboxError::Unavailable(
                "process namespace isolation is unavailable".into(),
            ));
        }
        let argv = build_bwrap_argv(policy, command, !policy.allow_network, true)?;
        let output = spawn_bwrap(&argv, timeout, policy, on_output.clone()).await?;
        Ok(ExecResult::from_output(output))
    }

    async fn open_warm(
        &self,
        policy: &ExecPolicy,
    ) -> Result<Box<dyn super::WarmCommand>, SandboxError> {
        let session = super::warm::BwrapSession::start(policy, &[], !policy.allow_network).await?;
        Ok(Box::new(session))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::exec_sandbox::EnvPolicy;

    #[test]
    fn bwrap_argv_includes_unshare_net_when_requested() {
        let policy = ExecPolicy {
            cwd: PathBuf::from("/tmp"),
            writable_roots: vec![PathBuf::from("/tmp")],
            allow_network: false,
            env: EnvPolicy::default(),
            max_output_bytes: 1024 * 1024,

            env_overrides: Vec::new(),
        };
        let argv = build_bwrap_argv(&policy, "echo hi", true, false).expect("argv");
        assert!(argv.contains(&"--unshare-net".to_string()));
        assert!(argv.contains(&"--die-with-parent".to_string()));
        assert!(!argv.contains(&"--unshare-pid".to_string()));
        assert!(argv.ends_with(&["sh".into(), "-c".into(), "echo hi".into()]));
    }

    #[test]
    fn bwrap_argv_omits_unshare_net_when_not_requested() {
        let policy = ExecPolicy {
            cwd: PathBuf::from("/tmp"),
            writable_roots: vec![PathBuf::from("/tmp")],
            allow_network: true,
            env: EnvPolicy::default(),
            max_output_bytes: 1024 * 1024,

            env_overrides: Vec::new(),
        };
        let argv = build_bwrap_argv(&policy, "curl example.com", false, true).expect("argv");
        assert!(!argv.contains(&"--unshare-net".to_string()));
        assert!(argv.contains(&"--die-with-parent".to_string()));
        assert!(argv.contains(&"--unshare-pid".to_string()));
        assert!(argv.contains(&"--as-pid-1".to_string()));
    }

    #[test]
    fn probe_argv_mounts_every_runtime_path() {
        let argv = build_probe_argv(true, true);
        for path in ["/usr", "/bin", "/sbin", "/lib", "/lib64", "/etc"] {
            if Path::new(path).exists() {
                assert!(argv.windows(3).any(|window| {
                    window[0] == "--ro-bind" && window[1] == path && window[2] == path
                }));
            }
        }
        assert!(argv.contains(&"--unshare-net".to_string()));
        assert!(argv.contains(&"--unshare-pid".to_string()));
        assert!(argv.ends_with(&["sh".into(), "-c".into(), "true".into()]));
    }

    #[test]
    fn uid_map_failure_names_the_apparmor_profile() {
        let message =
            format_bwrap_unavailable_detail("bwrap: setting up uid map: Permission denied");
        assert!(message.contains("uid map"));
        assert!(message.contains("AppArmor"));
        assert!(message.contains("contrib/apparmor/bwrap"));
    }

    #[test]
    fn other_bwrap_stderr_is_passed_through() {
        assert_eq!(
            format_bwrap_unavailable_detail("bwrap: No permissions to create a new namespace"),
            "bwrap: No permissions to create a new namespace"
        );
    }

    #[test]
    fn empty_detail_says_bwrap_cannot_sandbox() {
        assert_eq!(
            format_bwrap_unavailable_detail(""),
            "bwrap is missing or cannot create a sandbox"
        );
    }

    #[test]
    fn uid_map_reason_needs_an_apparmor_profile() {
        assert!(reason_needs_apparmor(
            "bwrap: setting up uid map: Permission denied. Ubuntu 24.04+"
        ));
        assert!(!reason_needs_apparmor("bwrap not found on PATH"));
    }

    #[test]
    fn bundled_apparmor_profile_grants_userns() {
        let profile = bwrap_apparmor_profile();
        assert!(profile.contains("profile bwrap /usr/bin/bwrap"));
        assert!(profile.contains("userns,"));
    }

    #[test]
    fn extra_gitdir_bind_hides_hooks() {
        let workspace = tempfile::TempDir::new().expect("workspace");
        let gitdir = tempfile::TempDir::new().expect("gitdir");
        std::fs::write(gitdir.path().join("HEAD"), "ref: refs/heads/main").expect("HEAD");
        let policy = ExecPolicy {
            cwd: workspace.path().to_path_buf(),
            writable_roots: vec![workspace.path().to_path_buf(), gitdir.path().to_path_buf()],
            allow_network: false,
            env: EnvPolicy::default(),
            max_output_bytes: 1024 * 1024,

            env_overrides: Vec::new(),
        };
        let argv = build_bwrap_argv(&policy, "true", false, false).expect("argv");
        let gitdir_path = gitdir
            .path()
            .canonicalize()
            .expect("gitdir")
            .display()
            .to_string();
        assert!(argv.windows(3).any(|window| {
            window[0] == "--bind" && window[1] == gitdir_path && window[2] == gitdir_path
        }));
        assert!(argv.contains(&format!("{gitdir_path}/hooks")));
    }

    #[test]
    fn extra_read_root_is_read_only() {
        let workspace = tempfile::TempDir::new().expect("workspace");
        let readable = tempfile::TempDir::new().expect("readable");
        let policy = ExecPolicy {
            cwd: workspace.path().to_path_buf(),
            writable_roots: vec![workspace.path().to_path_buf()],
            allow_network: false,
            env: EnvPolicy::default(),
            max_output_bytes: 1024,

            env_overrides: Vec::new(),
        };
        let argv = build_bwrap_argv_with_read_only_roots(
            &policy,
            &[readable.path().to_path_buf()],
            "true",
            true,
            true,
        )
        .expect("argv");
        let path = readable.path().canonicalize().expect("readable path");
        let path = path.display().to_string();
        assert!(argv
            .windows(3)
            .any(|window| { window[0] == "--ro-bind" && window[1] == path && window[2] == path }));
        assert!(!argv
            .windows(3)
            .any(|window| window[0] == "--bind" && window[1] == path));
    }

    #[test]
    fn extra_read_root_rejects_the_host_root() {
        let workspace = tempfile::TempDir::new().expect("workspace");
        let policy = ExecPolicy {
            cwd: workspace.path().to_path_buf(),
            writable_roots: vec![workspace.path().to_path_buf()],
            allow_network: false,
            env: EnvPolicy::default(),
            max_output_bytes: 1024,

            env_overrides: Vec::new(),
        };
        let error = build_bwrap_argv_with_read_only_roots(
            &policy,
            &[PathBuf::from("/")],
            "true",
            true,
            true,
        )
        .expect_err("host root must be rejected");
        assert!(matches!(error, SandboxError::InvalidPolicy(_)));
    }

    #[tokio::test]
    async fn strict_workspace_rejects_a_writable_sibling() {
        let workspace = tempfile::TempDir::new().expect("workspace");
        let sibling = tempfile::TempDir::new().expect("sibling");
        let policy = ExecPolicy {
            cwd: workspace.path().to_path_buf(),
            writable_roots: vec![sibling.path().to_path_buf()],
            allow_network: false,
            env: EnvPolicy::default(),
            max_output_bytes: 1024,

            env_overrides: Vec::new(),
        };
        let error = WorkspaceBubblewrapSandbox
            .run_shell(&policy, "touch outside", Duration::from_secs(1))
            .await
            .expect_err("sibling must be rejected");
        assert!(matches!(error, SandboxError::InvalidPolicy(_)));
    }

    #[test]
    fn sandbox_argv_never_mounts_the_host_root() {
        let policy = ExecPolicy {
            cwd: PathBuf::from("/tmp"),
            writable_roots: vec![PathBuf::from("/tmp")],
            allow_network: false,
            env: EnvPolicy::default(),
            max_output_bytes: 1024,

            env_overrides: Vec::new(),
        };
        let argv = build_bwrap_argv(&policy, "true", true, true).expect("argv");
        assert!(!argv
            .windows(3)
            .any(|window| { window[0] == "--ro-bind" && window[1] == "/" && window[2] == "/" }));
    }
}
