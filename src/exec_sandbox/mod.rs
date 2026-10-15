mod bubblewrap;
mod warm;
mod command;
#[cfg(feature = "docker")]
mod docker;
mod env;
mod error;
#[cfg(feature = "firecracker")]
mod firecracker;
mod git_roots;
mod none;
mod policy;
mod process;
mod result;
mod stub;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tracing::warn;

/// Count of `Auto` selections that fell back to [`NoneSandbox`].
static NONE_SANDBOX_AUTO_FALLBACKS: AtomicU64 = AtomicU64::new(0);

pub use bubblewrap::{
    build_bwrap_argv, bwrap_apparmor_profile, bwrap_unavailable_reason, needs_bwrap_apparmor,
    reset_bwrap_probe, BubblewrapSandbox, WorkspaceBubblewrapSandbox,
};
pub use error::SandboxError;
#[cfg(feature = "firecracker")]
pub use firecracker::FirecrackerShellSandbox;
pub use none::NoneSandbox;
pub use policy::{EnvPolicy, ExecPolicy};
pub use process::{ProcessSnapshot, ProcessStatus, SupervisedProcess};
pub use result::ExecResult;

pub use result::{emit_result_output, CommandSandbox, OutputSink, OutputStream, WarmCommand};
pub use warm::run_warm;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExecBackend {
    #[default]
    Auto,
    None,
    Bubblewrap,
    Docker,
    #[cfg(feature = "firecracker")]
    Firecracker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NetworkPolicy {
    Allow,
    Deny,
    #[default]
    Ask,
}

#[derive(Debug, Clone)]
pub struct SandboxSelection {
    pub enabled: bool,
    pub backend: ExecBackend,
    pub network: NetworkPolicy,
    /// When true, an unavailable isolation backend returns [`SandboxError::Unavailable`]
    /// instead of [`NoneSandbox`]. Explicit [`ExecBackend::None`] still selects none.
    pub fail_closed: bool,
}

impl Default for SandboxSelection {
    fn default() -> Self {
        Self {
            enabled: true,
            backend: ExecBackend::Auto,
            network: NetworkPolicy::Ask,
            fail_closed: false,
        }
    }
}

/// Number of times `Auto` fell back to unsandboxed host execution.
pub fn none_sandbox_fallback_count() -> u64 {
    NONE_SANDBOX_AUTO_FALLBACKS.load(Ordering::Relaxed)
}

/// Reset the Auto→none fallback counter (tests and embedder probes).
pub fn reset_none_sandbox_fallback_count() {
    NONE_SANDBOX_AUTO_FALLBACKS.store(0, Ordering::Relaxed);
}

/// Pick a sandbox backend based on config and compile-time features.
///
/// `Auto` prefers bubblewrap, then Docker (if compiled). When no isolation
/// backend is available it emits a warn, increments
/// [`none_sandbox_fallback_count`], and returns [`NoneSandbox`] unless
/// [`SandboxSelection::fail_closed`] is set.
pub fn select_backend(
    selection: &SandboxSelection,
) -> Result<Arc<dyn CommandSandbox>, SandboxError> {
    if !selection.enabled {
        return Ok(Arc::new(NoneSandbox));
    }

    match selection.backend {
        ExecBackend::None => Ok(Arc::new(NoneSandbox)),
        ExecBackend::Bubblewrap => pick_bubblewrap_or_none(selection.fail_closed),
        ExecBackend::Docker => pick_docker_or_warn(selection.fail_closed),
        #[cfg(feature = "firecracker")]
        ExecBackend::Firecracker => pick_firecracker_or_none(selection.fail_closed),
        ExecBackend::Auto => auto_select(selection.fail_closed),
    }
}

fn record_auto_none_fallback(reason: &str) {
    let count = NONE_SANDBOX_AUTO_FALLBACKS.fetch_add(1, Ordering::Relaxed) + 1;
    warn!(
        reason,
        fallback_count = count,
        backend = "none",
        "sandbox Auto selected none; host execution is not isolated"
    );
}

fn none_or_unavailable(
    fail_closed: bool,
    reason: String,
) -> Result<Arc<dyn CommandSandbox>, SandboxError> {
    if fail_closed {
        return Err(SandboxError::Unavailable(reason));
    }
    Ok(Arc::new(NoneSandbox))
}

fn auto_select(fail_closed: bool) -> Result<Arc<dyn CommandSandbox>, SandboxError> {
    #[cfg(target_os = "linux")]
    {
        let bwrap = BubblewrapSandbox;
        if bwrap.is_available() {
            return Ok(Arc::new(bwrap));
        }
    }

    #[cfg(target_os = "macos")]
    {
        use stub::SeatbeltSandbox;
        let seatbelt = SeatbeltSandbox;
        if seatbelt.is_available() {
            return Ok(Arc::new(seatbelt));
        }
    }

    #[cfg(windows)]
    {
        use stub::WindowsSandbox;
        let windows = WindowsSandbox;
        if windows.is_available() {
            return Ok(Arc::new(windows));
        }
    }

    #[cfg(feature = "docker")]
    {
        if let Some(d) = docker::docker_backend_if_available() {
            return Ok(d);
        }
    }

    record_auto_none_fallback("no isolation backend available");
    none_or_unavailable(
        fail_closed,
        "no isolation backend available (bubblewrap/docker)".into(),
    )
}

fn pick_bubblewrap_or_none(fail_closed: bool) -> Result<Arc<dyn CommandSandbox>, SandboxError> {
    let bwrap = BubblewrapSandbox;
    if bwrap.is_available() {
        return Ok(Arc::new(bwrap));
    }
    let reason = bwrap_unavailable_reason().unwrap_or_else(|| "unknown".into());
    warn!(
        reason = reason.as_str(),
        "bubblewrap backend requested but bwrap is unavailable or cannot sandbox; using none"
    );
    none_or_unavailable(fail_closed, format!("bubblewrap unavailable: {reason}"))
}

fn pick_docker_or_warn(fail_closed: bool) -> Result<Arc<dyn CommandSandbox>, SandboxError> {
    #[cfg(feature = "docker")]
    {
        if let Some(d) = docker::docker_backend_if_available() {
            return Ok(d);
        }
        warn!("docker backend requested but Docker is unavailable; using none");
        none_or_unavailable(fail_closed, "Docker is unavailable".into())
    }

    #[cfg(not(feature = "docker"))]
    {
        warn!(
            "docker backend requested but sandy was built without the `docker` feature; using none"
        );
        none_or_unavailable(
            fail_closed,
            "sandy was built without the `docker` feature".into(),
        )
    }
}

#[cfg(feature = "firecracker")]
fn pick_firecracker_or_none(fail_closed: bool) -> Result<Arc<dyn CommandSandbox>, SandboxError> {
    if let Some(backend) = firecracker::firecracker_backend_if_available() {
        Ok(backend)
    } else {
        warn!("firecracker backend requested but Firecracker, KVM, kernel, or rootfs is unavailable; using none");
        none_or_unavailable(
            fail_closed,
            "Firecracker, KVM, kernel, or rootfs is unavailable".into(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection(backend: ExecBackend, fail_closed: bool) -> SandboxSelection {
        SandboxSelection {
            enabled: true,
            backend,
            network: NetworkPolicy::Deny,
            fail_closed,
        }
    }

    #[test]
    fn disabled_selection_uses_none() {
        let backend = select_backend(&SandboxSelection {
            enabled: false,
            backend: ExecBackend::Bubblewrap,
            network: NetworkPolicy::Deny,
            fail_closed: false,
        })
        .expect("disabled selection stays none");
        assert_eq!(backend.id(), "none");
    }

    #[test]
    fn explicit_none_backend() {
        let before = none_sandbox_fallback_count();
        let backend =
            select_backend(&selection(ExecBackend::None, false)).expect("explicit none is allowed");
        assert_eq!(backend.id(), "none");
        assert_eq!(none_sandbox_fallback_count(), before);
    }

    #[test]
    fn explicit_none_is_allowed_when_fail_closed() {
        let backend = select_backend(&selection(ExecBackend::None, true))
            .expect("explicit none is not a fallback");
        assert_eq!(backend.id(), "none");
    }

    #[test]
    fn fail_closed_rejects_unavailable_requested_backend() {
        let result = select_backend(&selection(ExecBackend::Docker, true));
        #[cfg(feature = "docker")]
        {
            if let Ok(backend) = result {
                assert_eq!(backend.id(), "docker");
            } else {
                assert!(matches!(result, Err(SandboxError::Unavailable(_))));
            }
        }
        #[cfg(not(feature = "docker"))]
        {
            assert!(matches!(result, Err(SandboxError::Unavailable(_))));
        }
    }

    #[test]
    fn auto_none_fallback_records_metric() {
        reset_none_sandbox_fallback_count();
        record_auto_none_fallback("test");
        assert_eq!(none_sandbox_fallback_count(), 1);
        let err = none_or_unavailable(true, "no isolation backend".into());
        assert!(matches!(err, Err(SandboxError::Unavailable(_))));
        let ok = none_or_unavailable(false, "no isolation backend".into())
            .expect("fail-open returns none");
        assert_eq!(ok.id(), "none");
    }

    #[cfg(feature = "firecracker")]
    #[test]
    fn explicit_firecracker_backend_is_safe_when_unavailable() {
        let backend = select_backend(&selection(ExecBackend::Firecracker, false))
            .expect("fail-open firecracker falls back");
        assert!(backend.id() == "firecracker" || backend.id() == "none");
    }
}
