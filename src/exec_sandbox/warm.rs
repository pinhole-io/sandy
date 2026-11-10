//! One long-lived bubblewrap process for a fixed mount set.
//!
//! Later commands travel over stdin. The child writes each result into a
//! private directory that is mounted only for that session. A failed start
//! returns [`SandboxError`] and does not run the command on the host.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::Mutex;

use super::bubblewrap::{
    build_bwrap_argv_with_read_only_roots, unshare_net_supported, unshare_pid_supported,
    BubblewrapSandbox,
};
use super::env::apply_env;
use super::error::SandboxError;
use super::policy::ExecPolicy;
use super::result::{ExecResult, WarmCommand};
use super::CommandSandbox;

const CACHE_LIMIT: usize = 4;

struct CacheSlot {
    key: SessionKey,
    session: Arc<Mutex<BwrapSession>>,
}

static CACHE: Mutex<Vec<CacheSlot>> = Mutex::const_new(Vec::new());

#[derive(Clone, PartialEq, Eq)]
struct SessionKey {
    writable: Vec<PathBuf>,
    read_only: Vec<PathBuf>,
    isolate_network: bool,
}

pub struct BwrapSession {
    child: Child,
    stdin: ChildStdin,
    out_dir: tempfile::TempDir,
    writable: Vec<PathBuf>,
    read_only: Vec<PathBuf>,
    alive: bool,
}

impl BwrapSession {
    pub async fn start(
        policy: &ExecPolicy,
        read_only_roots: &[PathBuf],
        isolate_network: bool,
    ) -> Result<Self, SandboxError> {
        if !BubblewrapSandbox.is_available() {
            return Err(SandboxError::Unavailable(
                "bubblewrap is unavailable".into(),
            ));
        }
        if isolate_network && !unshare_net_supported() {
            return Err(SandboxError::Unavailable(
                "network namespace isolation is unavailable".into(),
            ));
        }
        if !unshare_pid_supported() {
            return Err(SandboxError::Unavailable(
                "process namespace isolation is unavailable".into(),
            ));
        }
        let out_dir = tempfile::TempDir::new().map_err(|error| {
            SandboxError::SpawnFailed(format!("warm session directory: {error}"))
        })?;
        if out_dir.path().display().to_string().contains('\'') {
            return Err(SandboxError::InvalidPolicy(
                "warm session path is not usable".into(),
            ));
        }
        let mut mounted = policy.clone();
        mounted.writable_roots.push(out_dir.path().to_path_buf());
        let command = supervisor(out_dir.path());
        let mut argv = build_bwrap_argv_with_read_only_roots(
            &mounted,
            read_only_roots,
            &command,
            isolate_network,
            true,
        )?;
        // `--tmpfs /tmp` is applied after the root binds. A workspace or
        // control directory under `/tmp` must be mounted again afterwards.
        lift_tmp_mounts(&mut argv);
        let mut cmd = Command::new(argv.first().cloned().unwrap_or_else(|| "bwrap".into()));
        for arg in argv.iter().skip(1) {
            cmd.arg(arg);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        apply_env(&mut cmd, &policy.env);
        let mut child = cmd
            .spawn()
            .map_err(|error| SandboxError::SpawnFailed(format!("warm bubblewrap: {error}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| SandboxError::SpawnFailed("warm bubblewrap stdin is missing".into()))?;
        let writable = canonicalize_roots(&policy.writable_roots)?;
        let read_only = canonicalize_roots(read_only_roots)?;
        let mut stderr = child.stderr.take();
        let mut session = Self {
            child,
            stdin,
            out_dir,
            writable,
            read_only,
            alive: true,
        };
        if let Err(error) = session
            .run(policy.cwd.as_path(), "true", 1024, Duration::from_secs(5))
            .await
        {
            let detail = read_stderr(stderr.as_mut()).await;
            return Err(SandboxError::Unavailable(format!(
                "warm bubblewrap failed to start: {error}; {detail}"
            )));
        }
        if let Some(mut stderr) = stderr {
            tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                let mut buf = [0_u8; 1024];
                loop {
                    match stderr.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                }
            });
        }
        Ok(session)
    }

    fn status_path(&self) -> PathBuf {
        self.out_dir.path().join("status")
    }

    fn stdout_path(&self) -> PathBuf {
        self.out_dir.path().join("stdout")
    }

    fn stderr_path(&self) -> PathBuf {
        self.out_dir.path().join("stderr")
    }

    async fn send(&mut self, cwd: &Path, command: &str) -> Result<(), SandboxError> {
        let cwd = cwd.display().to_string();
        let cwd_len = cwd.len().to_string();
        let cmd_len = command.len().to_string();
        self.stdin
            .write_all(cwd_len.as_bytes())
            .await
            .map_err(dead)?;
        self.stdin.write_all(b"\n").await.map_err(dead)?;
        self.stdin.write_all(cwd.as_bytes()).await.map_err(dead)?;
        self.stdin
            .write_all(cmd_len.as_bytes())
            .await
            .map_err(dead)?;
        self.stdin.write_all(b"\n").await.map_err(dead)?;
        self.stdin
            .write_all(command.as_bytes())
            .await
            .map_err(dead)?;
        self.stdin.flush().await.map_err(dead)?;
        Ok(())
    }

    async fn wait_status(&mut self, timeout: Duration) -> Result<i32, SandboxError> {
        let path = self.status_path();
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(Some(status)) = self.child.try_wait() {
                self.alive = false;
                let _ = status;
                return Err(SandboxError::Unavailable("warm bubblewrap exited".into()));
            }
            if let Some(code) = read_status(&path) {
                return Ok(code);
            }
            if Instant::now() >= deadline {
                self.alive = false;
                let _ = self.child.start_kill();
                return Err(SandboxError::TimedOut(timeout.as_secs()));
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

#[async_trait]
impl WarmCommand for BwrapSession {
    async fn run(
        &mut self,
        cwd: &Path,
        command: &str,
        max_output_bytes: usize,
        timeout: Duration,
    ) -> Result<ExecResult, SandboxError> {
        if !self.alive {
            return Err(SandboxError::Unavailable(
                "warm bubblewrap is closed".into(),
            ));
        }
        if command.is_empty() || command.contains('\0') {
            return Err(SandboxError::InvalidPolicy("invalid command".into()));
        }
        let cwd = cwd.canonicalize().map_err(|error| {
            SandboxError::InvalidPolicy(format!("cwd is outside the session: {error}"))
        })?;
        if !cwd_inside(&cwd, &self.writable, &self.read_only) {
            return Err(SandboxError::InvalidPolicy(
                "cwd is outside the session roots".into(),
            ));
        }
        let _ = std::fs::remove_file(self.status_path());
        self.send(&cwd, command).await?;
        let code = self.wait_status(timeout).await?;
        let stdout = read_capped(&self.stdout_path(), max_output_bytes)?;
        let stderr = read_capped(&self.stderr_path(), max_output_bytes)?;
        Ok(ExecResult {
            stdout,
            stderr,
            exit_code: Some(code),
        })
    }

    fn host_pid(&self) -> u32 {
        self.child.id().unwrap_or(0)
    }
}

/// Run `command` in a cached bubblewrap session for this mount set.
///
/// A new session starts when the writable roots, read-only roots, or network
/// flag change. [`SandboxError::Unavailable`] means the caller should use a
/// one-shot bubblewrap. This function never runs the command on the host.
pub async fn run_warm(
    policy: &ExecPolicy,
    read_only_roots: &[PathBuf],
    command: &str,
    timeout: Duration,
    isolate_network: bool,
) -> Result<ExecResult, SandboxError> {
    let key = SessionKey {
        writable: canonicalize_roots(&policy.writable_roots)?,
        read_only: canonicalize_roots(read_only_roots)?,
        isolate_network,
    };
    let slot = {
        let mut cache = CACHE.lock().await;
        cache.retain(|slot| match slot.session.try_lock() {
            Ok(guard) => guard.alive,
            Err(_) => true,
        });
        if let Some(slot) = cache.iter().find(|slot| slot.key == key) {
            Arc::clone(&slot.session)
        } else {
            let session = BwrapSession::start(policy, read_only_roots, isolate_network).await?;
            let session = Arc::new(Mutex::new(session));
            cache.push(CacheSlot {
                key,
                session: Arc::clone(&session),
            });
            if cache.len() > CACHE_LIMIT {
                cache.remove(0);
            }
            session
        }
    };
    let mut session = slot.lock().await;
    if !session.alive {
        return Err(SandboxError::Unavailable(
            "warm bubblewrap is closed".into(),
        ));
    }
    let result = session
        .run(
            policy.cwd.as_path(),
            command,
            policy.max_output_bytes,
            timeout,
        )
        .await;
    if result.is_err() {
        session.alive = false;
        let _ = session.child.start_kill();
    }
    result
}

fn lift_tmp_mounts(argv: &mut Vec<String>) {
    let mut lifted = Vec::new();
    let mut index = 0;
    while index + 2 < argv.len() {
        let kind = argv[index].as_str();
        let source = argv[index + 1].as_str();
        let dest = argv[index + 2].as_str();
        let hides_under_tmp = (kind == "--bind" || kind == "--ro-bind")
            && source.starts_with("/tmp/")
            && source == dest;
        if hides_under_tmp {
            lifted.extend(argv.drain(index..index + 3));
            continue;
        }
        index += 1;
    }
    if lifted.is_empty() {
        return;
    }
    let insert_at = argv
        .windows(2)
        .position(|window| window[0] == "--tmpfs" && window[1] == "/tmp")
        .map_or(argv.len(), |idx| idx + 2);
    argv.splice(insert_at..insert_at, lifted);
}

fn supervisor(out_dir: &Path) -> String {
    let out = out_dir.display();
    format!(
        r#"out='{out}'
while IFS= read -r cwd_len; do
  [ "$cwd_len" = exit ] && exit 0
  cwd=$(dd bs=1 count="$cwd_len" 2>/dev/null) || exit 1
  IFS= read -r cmd_len || exit 1
  cmd=$(dd bs=1 count="$cmd_len" 2>/dev/null) || exit 1
  : > "$out/stdout"
  : > "$out/stderr"
  rm -f "$out/status"
  if cd "$cwd" 2>>"$out/stderr"; then
    env -i PATH="$PATH" HOME="${{HOME-}}" sh -c "$cmd" >"$out/stdout" 2>>"$out/stderr"
    code=$?
  else
    code=1
  fi
  for entry in /proc/[0-9]*; do
    pid=${{entry#/proc/}}
    [ "$pid" = 1 ] && continue
    kill -9 "$pid" 2>/dev/null || true
  done
  printf '%s\n' "$code" > "$out/status"
done
"#
    )
}

fn canonicalize_roots(roots: &[PathBuf]) -> Result<Vec<PathBuf>, SandboxError> {
    let mut out = Vec::with_capacity(roots.len());
    for root in roots {
        let path = root.canonicalize().map_err(|error| {
            SandboxError::InvalidPolicy(format!("session root {}: {error}", root.display()))
        })?;
        out.push(path);
    }
    Ok(out)
}

fn cwd_inside(cwd: &Path, writable: &[PathBuf], read_only: &[PathBuf]) -> bool {
    writable
        .iter()
        .chain(read_only.iter())
        .any(|root| cwd.starts_with(root))
}

fn read_status(path: &Path) -> Option<i32> {
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    text.parse().ok()
}

fn read_capped(path: &Path, max: usize) -> Result<String, SandboxError> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(String::new());
        }
        Err(error) => return Err(error.into()),
    };
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 8192];
    use std::io::Read;
    loop {
        let n = file.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        if buf.len().saturating_add(n) > max {
            return Err(SandboxError::OutputLimitExceeded(max));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

async fn read_stderr(stderr: Option<&mut tokio::process::ChildStderr>) -> String {
    let Some(stderr) = stderr else {
        return "no stderr".to_string();
    };
    use tokio::io::AsyncReadExt;
    let mut buf = vec![0_u8; 1024];
    let Ok(n) = tokio::time::timeout(Duration::from_millis(200), stderr.read(&mut buf)).await
    else {
        return "stderr unread".to_string();
    };
    match n {
        Ok(0) => "stderr empty".to_string(),
        Ok(n) => String::from_utf8_lossy(&buf[..n]).into_owned(),
        Err(error) => format!("stderr: {error}"),
    }
}

fn dead(error: std::io::Error) -> SandboxError {
    SandboxError::Unavailable(format!("warm bubblewrap stdin: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use crate::exec_sandbox::EnvPolicy;

    #[tokio::test]
    async fn two_commands_share_one_bwrap_and_hide_a_sibling() {
        if !BubblewrapSandbox::bwrap_on_path() {
            return;
        }
        if !unshare_net_supported() || !unshare_pid_supported() {
            return;
        }
        let root = tempfile::tempdir().expect("workspace");
        let sibling = tempfile::tempdir().expect("sibling");
        std::fs::write(sibling.path().join("secret"), b"nope").expect("secret");
        let policy = ExecPolicy {
            cwd: root.path().to_path_buf(),
            writable_roots: vec![root.path().to_path_buf()],
            allow_network: false,
            env: EnvPolicy::Clean { allowlist: vec![] },
            max_output_bytes: 64 * 1024,

            env_overrides: Vec::new(),
        };
        let mut session = BubblewrapSandbox
            .open_warm(&policy)
            .await
            .expect("warm session");
        let first = session
            .run(root.path(), "echo hi", 64 * 1024, Duration::from_secs(10))
            .await
            .expect("first command");
        let pid = session.host_pid();
        assert!(pid > 0);
        let second = session
            .run(root.path(), "echo hi", 64 * 1024, Duration::from_secs(10))
            .await
            .expect("second command");
        assert_eq!(session.host_pid(), pid);
        assert_eq!(first.stdout.trim(), "hi");
        assert_eq!(second.stdout.trim(), "hi");
        let secret = sibling.path().join("secret");
        let leaked = session
            .run(
                root.path(),
                &format!("cat '{}'", secret.display()),
                64 * 1024,
                Duration::from_secs(10),
            )
            .await
            .expect("sibling command");
        assert_ne!(leaked.exit_code, Some(0));
        assert!(!leaked.stdout.contains("nope"));
    }
}
