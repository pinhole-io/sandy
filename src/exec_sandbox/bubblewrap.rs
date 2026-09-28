use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use tokio::process::Command;
use tracing::warn;

use super::command::run_command_with_timeout_and_output;
use super::error::SandboxError;
use super::policy::ExecPolicy;
use super::result::OutputSink;
use super::CommandSandbox;
use super::ExecResult;

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
    if !Path::new("/").exists() {
        return Err(SandboxError::InvalidPolicy("host root missing".into()));
    }

    let mut args = vec!["bwrap".to_string()];
    args.push("--ro-bind".into());
    args.push("/".into());
    args.push("/".into());

    let cwd = policy
        .cwd
        .canonicalize()
        .map_err(|e| SandboxError::InvalidPolicy(format!("cwd {}: {e}", policy.cwd.display())))?;

    for root in &policy.writable_roots {
        let host = root.canonicalize().map_err(|e| {
            SandboxError::InvalidPolicy(format!("writable root {}: {e}", root.display()))
        })?;
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

fn mark_unshare_net_unsupported() {
    unshare_net_flag().store(false, Ordering::Relaxed);
}

static UNSHARE_PID_WORKS: OnceLock<AtomicBool> = OnceLock::new();

fn unshare_pid_flag() -> &'static AtomicBool {
    UNSHARE_PID_WORKS.get_or_init(|| AtomicBool::new(probe_unshare_pid()))
}

/// Whether `bwrap --unshare-pid --as-pid-1` works on this host.
pub fn unshare_pid_supported() -> bool {
    unshare_pid_flag().load(Ordering::Relaxed)
}

fn mark_unshare_pid_unsupported() {
    unshare_pid_flag().store(false, Ordering::Relaxed);
}

fn probe_unshare_pid() -> bool {
    which_bwrap().is_some_and(|bwrap| {
        std::process::Command::new(bwrap)
            .args([
                "--die-with-parent",
                "--unshare-pid",
                "--as-pid-1",
                "--ro-bind",
                "/",
                "/",
                "--dev",
                "/dev",
                "--proc",
                "/proc",
                "--tmpfs",
                "/tmp",
                "--",
                "true",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

fn probe_unshare_net() -> bool {
    which_bwrap().is_some_and(|bwrap| {
        std::process::Command::new(bwrap)
            .args([
                "--unshare-net",
                "--ro-bind",
                "/",
                "/",
                "--dev",
                "/dev",
                "--proc",
                "/proc",
                "--tmpfs",
                "/tmp",
                "--",
                "true",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
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
    let Some(bwrap) = which_bwrap() else {
        return BwrapProbe {
            ok: false,
            detail: "bwrap not found on PATH".to_string(),
        };
    };
    match std::process::Command::new(bwrap)
        .args([
            "--ro-bind",
            "/",
            "/",
            "--dev",
            "/dev",
            "--proc",
            "/proc",
            "--tmpfs",
            "/tmp",
            "--",
            "true",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .output()
    {
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

fn isolate_network_for_policy(policy: &ExecPolicy) -> bool {
    !policy.allow_network && unshare_net_supported()
}

fn is_loopback_netns_error(stderr: &str) -> bool {
    stderr.contains("loopback")
        || stderr.contains("RTM_NEWADDR")
        || stderr.contains("Operation not permitted") && stderr.contains("bwrap")
}

fn is_pid_ns_error(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("unshare-pid") || lower.contains("pid namespace") || lower.contains("as-pid-1")
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
    run_command_with_timeout_and_output(cmd, timeout, policy.max_output_bytes, on_output).await
}

pub struct BubblewrapSandbox;

impl BubblewrapSandbox {
    pub fn bwrap_on_path() -> bool {
        which_bwrap().is_some()
    }
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
        let isolate = isolate_network_for_policy(policy);
        let isolate_pid = unshare_pid_supported();
        let argv = build_bwrap_argv(policy, command, isolate, isolate_pid)?;
        let output = spawn_bwrap(&argv, timeout, policy, None).await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if isolate && is_loopback_netns_error(&stderr) {
                warn!(
                    "bwrap --unshare-net failed ({stderr}); retrying without network namespace isolation"
                );
                mark_unshare_net_unsupported();
                let argv = build_bwrap_argv(policy, command, false, isolate_pid)?;
                let output = spawn_bwrap(&argv, timeout, policy, None).await?;
                return Ok(ExecResult::from_output(output));
            }
            if isolate_pid && is_pid_ns_error(&stderr) {
                warn!(
                    "bwrap --unshare-pid failed ({stderr}); retrying without process namespace isolation"
                );
                mark_unshare_pid_unsupported();
                let argv = build_bwrap_argv(policy, command, isolate, false)?;
                let output = spawn_bwrap(&argv, timeout, policy, None).await?;
                return Ok(ExecResult::from_output(output));
            }
        }

        Ok(ExecResult::from_output(output))
    }

    async fn run_shell_streaming(
        &self,
        policy: &ExecPolicy,
        command: &str,
        timeout: Duration,
        on_output: Option<OutputSink>,
    ) -> Result<ExecResult, SandboxError> {
        let isolate = isolate_network_for_policy(policy);
        let isolate_pid = unshare_pid_supported();
        let argv = build_bwrap_argv(policy, command, isolate, isolate_pid)?;
        let output = spawn_bwrap(&argv, timeout, policy, on_output.clone()).await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if isolate && is_loopback_netns_error(&stderr) {
                warn!(
                    "bwrap --unshare-net failed ({stderr}); retrying without network namespace isolation"
                );
                mark_unshare_net_unsupported();
                let argv = build_bwrap_argv(policy, command, false, isolate_pid)?;
                let output = spawn_bwrap(&argv, timeout, policy, on_output).await?;
                return Ok(ExecResult::from_output(output));
            }
            if isolate_pid && is_pid_ns_error(&stderr) {
                warn!(
                    "bwrap --unshare-pid failed ({stderr}); retrying without process namespace isolation"
                );
                mark_unshare_pid_unsupported();
                let argv = build_bwrap_argv(policy, command, isolate, false)?;
                let output = spawn_bwrap(&argv, timeout, policy, on_output).await?;
                return Ok(ExecResult::from_output(output));
            }
        }

        Ok(ExecResult::from_output(output))
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
        };
        let argv = build_bwrap_argv(&policy, "curl example.com", false, true).expect("argv");
        assert!(!argv.contains(&"--unshare-net".to_string()));
        assert!(argv.contains(&"--die-with-parent".to_string()));
        assert!(argv.contains(&"--unshare-pid".to_string()));
        assert!(argv.contains(&"--as-pid-1".to_string()));
    }

    #[test]
    fn detects_pid_namespace_errors() {
        assert!(is_pid_ns_error(
            "bwrap: setting up pid namespace: Permission denied"
        ));
        assert!(!is_pid_ns_error("bwrap: loopback: Failed RTM_NEWADDR"));
    }

    #[test]
    fn detects_loopback_netns_errors() {
        assert!(is_loopback_netns_error(
            "bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted\n"
        ));
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
}
