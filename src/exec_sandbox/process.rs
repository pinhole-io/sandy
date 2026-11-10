use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, Notify};
use tokio::time;

use super::error::SandboxError;
use super::policy::ExecPolicy;
use super::result::OutputStream;

const STOP_GRACE: Duration = Duration::from_secs(2);
const OUTPUT_CHUNK: usize = 8192;

/// The current state of one supervised process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessStatus {
    /// The process is accepting work.
    Running,
    /// A stop was requested and the process is shutting down.
    Stopping,
    /// The process exited without a stop request.
    Exited,
    /// The process stopped after a stop request.
    Stopped,
}

/// A bounded snapshot of one supervised process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessSnapshot {
    /// The process state.
    pub status: ProcessStatus,
    /// The exit code, when the process returned one.
    pub exit_code: Option<i32>,
    /// Captured standard output.
    pub stdout: String,
    /// Captured standard error.
    pub stderr: String,
    /// `true` when either output stream exceeded its cap.
    pub output_truncated: bool,
}

/// An owned process that is stopped when its last handle is dropped.
#[derive(Clone)]
pub struct SupervisedProcess {
    inner: Arc<ProcessInner>,
}

struct ProcessInner {
    state: Arc<Mutex<ProcessState>>,
    commands: mpsc::UnboundedSender<ProcessCommand>,
    done: Arc<Notify>,
}

enum ProcessCommand {
    Stop,
}

struct ProcessState {
    status: ProcessStatus,
    exit_code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    output_truncated: bool,
}

impl SupervisedProcess {
    /// Return the latest bounded process state.
    pub fn snapshot(&self) -> ProcessSnapshot {
        match self.inner.state.lock() {
            Ok(state) => snapshot(&state),
            Err(poisoned) => snapshot(&poisoned.into_inner()),
        }
    }

    /// Stop the process and wait until its output and exit state are settled.
    pub async fn stop(&self) -> ProcessSnapshot {
        if !is_active(&self.snapshot().status) {
            return self.snapshot();
        }

        let notified = self.inner.done.notified();
        if self.inner.commands.send(ProcessCommand::Stop).is_ok() {
            notified.await;
        }
        self.snapshot()
    }
}

pub(crate) fn spawn(
    mut child: Child,
    policy: &ExecPolicy,
) -> Result<SupervisedProcess, SandboxError> {
    let Some(stdout) = child.stdout.take() else {
        let _ = child.start_kill();
        return Err(SandboxError::SpawnFailed(
            "stdout pipe was not configured".into(),
        ));
    };
    let Some(stderr) = child.stderr.take() else {
        let _ = child.start_kill();
        return Err(SandboxError::SpawnFailed(
            "stderr pipe was not configured".into(),
        ));
    };

    let state = Arc::new(Mutex::new(ProcessState {
        status: ProcessStatus::Running,
        exit_code: None,
        stdout: Vec::new(),
        stderr: Vec::new(),
        output_truncated: false,
    }));
    let done = Arc::new(Notify::new());
    let (commands, requests) = mpsc::unbounded_channel();
    let reader_state = Arc::clone(&state);
    let stdout_reader = tokio::spawn(read_output(
        stdout,
        Arc::clone(&reader_state),
        OutputStream::Stdout,
        policy.max_output_bytes,
    ));
    let stderr_reader = tokio::spawn(read_output(
        stderr,
        Arc::clone(&reader_state),
        OutputStream::Stderr,
        policy.max_output_bytes,
    ));

    let supervisor_state = Arc::clone(&state);
    let supervisor_done = Arc::clone(&done);
    tokio::spawn(supervise(
        child,
        requests,
        supervisor_state,
        supervisor_done,
        stdout_reader,
        stderr_reader,
    ));

    Ok(SupervisedProcess {
        inner: Arc::new(ProcessInner {
            state,
            commands,
            done,
        }),
    })
}

async fn supervise(
    mut child: Child,
    mut requests: mpsc::UnboundedReceiver<ProcessCommand>,
    state: Arc<Mutex<ProcessState>>,
    done: Arc<Notify>,
    mut stdout_reader: tokio::task::JoinHandle<()>,
    mut stderr_reader: tokio::task::JoinHandle<()>,
) {
    let (requested_stop, status) = tokio::select! {
        result = child.wait() => {
            (false, result.ok())
        }
        _request = requests.recv() => {
            set_status(&state, ProcessStatus::Stopping);
            (true, terminate(&mut child).await)
        }
    };

    wait_for_reader(&mut stdout_reader).await;
    wait_for_reader(&mut stderr_reader).await;

    let mut state = lock_state(&state);
    state.status = if requested_stop {
        ProcessStatus::Stopped
    } else {
        ProcessStatus::Exited
    };
    state.exit_code = status.and_then(|status| status.code());
    drop(state);
    done.notify_waiters();
}

async fn terminate(child: &mut Child) -> Option<std::process::ExitStatus> {
    let signal_sent = if let Some(pid) = child.id() {
        Command::new("/bin/kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .map(|status| status.success())
            .unwrap_or(false)
    } else {
        false
    };

    if !signal_sent {
        let _ = child.start_kill();
        return child.wait().await.ok();
    }

    match time::timeout(STOP_GRACE, child.wait()).await {
        Ok(result) => result.ok(),
        Err(_) => {
            let _ = child.start_kill();
            child.wait().await.ok()
        }
    }
}

async fn wait_for_reader(reader: &mut tokio::task::JoinHandle<()>) {
    if time::timeout(Duration::from_secs(1), &mut *reader)
        .await
        .is_err()
    {
        reader.abort();
    }
}

async fn read_output<R>(
    mut reader: R,
    state: Arc<Mutex<ProcessState>>,
    stream: OutputStream,
    max_output_bytes: usize,
) where
    R: AsyncRead + Unpin,
{
    let mut chunk = [0_u8; OUTPUT_CHUNK];
    loop {
        let count = match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(count) => count,
        };
        let mut state = lock_state(&state);
        let output = match stream {
            OutputStream::Stdout => &mut state.stdout,
            OutputStream::Stderr => &mut state.stderr,
        };
        let room = max_output_bytes.saturating_sub(output.len());
        let take = room.min(count);
        if take > 0 {
            output.extend_from_slice(&chunk[..take]);
        }
        if take < count {
            state.output_truncated = true;
        }
    }
}

fn set_status(state: &Mutex<ProcessState>, status: ProcessStatus) {
    lock_state(state).status = status;
}

fn lock_state(state: &Mutex<ProcessState>) -> std::sync::MutexGuard<'_, ProcessState> {
    match state.lock() {
        Ok(state) => state,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn snapshot(state: &ProcessState) -> ProcessSnapshot {
    ProcessSnapshot {
        status: state.status,
        exit_code: state.exit_code,
        stdout: String::from_utf8_lossy(&state.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&state.stderr).into_owned(),
        output_truncated: state.output_truncated,
    }
}

fn is_active(status: &ProcessStatus) -> bool {
    matches!(status, ProcessStatus::Running | ProcessStatus::Stopping)
}

pub(crate) async fn spawn_bwrap(
    argv: &[String],
    policy: &ExecPolicy,
) -> Result<SupervisedProcess, SandboxError> {
    let program = argv
        .first()
        .cloned()
        .ok_or_else(|| SandboxError::SpawnFailed("sandbox command is empty".into()))?;
    let mut command = Command::new(program);
    command
        .args(&argv[1..])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    super::env::apply_env(&mut command, &policy.env);
    let child = command.spawn().map_err(SandboxError::Io)?;
    spawn(child, policy)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(script: &str) -> Child {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(script)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        match command.spawn() {
            Ok(child) => child,
            Err(error) => panic!("spawn test command: {error}"),
        }
    }

    fn policy(max_output_bytes: usize) -> ExecPolicy {
        ExecPolicy {
            cwd: std::env::current_dir().expect("cwd"),
            writable_roots: Vec::new(),
            allow_network: false,
            env: super::super::EnvPolicy::default(),
            max_output_bytes,
            env_overrides: Vec::new(),
        }
    }

    #[tokio::test]
    async fn captures_output_and_exit_status() {
        let process =
            spawn(command("printf hello; printf error >&2"), &policy(64)).expect("process");
        tokio::time::sleep(Duration::from_millis(50)).await;
        let snapshot = process.snapshot();
        assert_eq!(snapshot.status, ProcessStatus::Exited);
        assert_eq!(snapshot.exit_code, Some(0));
        assert_eq!(snapshot.stdout, "hello");
        assert_eq!(snapshot.stderr, "error");
    }

    #[tokio::test]
    async fn caps_output_and_continues_draining() {
        let process = spawn(command("printf 1234567890"), &policy(4)).expect("process");
        tokio::time::sleep(Duration::from_millis(50)).await;
        let snapshot = process.snapshot();
        assert_eq!(snapshot.stdout, "1234");
        assert!(snapshot.output_truncated);
        assert_eq!(snapshot.status, ProcessStatus::Exited);
    }

    #[tokio::test]
    async fn stop_terminates_a_running_process() {
        let process = spawn(command("sleep 1"), &policy(64)).expect("process");
        tokio::time::sleep(Duration::from_millis(20)).await;
        let snapshot = process.stop().await;
        assert_eq!(snapshot.status, ProcessStatus::Stopped);
    }
}
