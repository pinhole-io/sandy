use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::time;

use super::error::SandboxError;
use super::result::{OutputSink, OutputStream};

/// Run a prepared command, enforcing a wall-clock timeout.
pub async fn run_command_with_timeout(
    cmd: Command,
    timeout: Duration,
    max_output_bytes: usize,
) -> Result<std::process::Output, SandboxError> {
    run_command_with_timeout_and_output(cmd, timeout, max_output_bytes, None).await
}

/// Run a prepared command and emit stdout/stderr chunks as they arrive.
pub async fn run_command_with_timeout_and_output(
    mut cmd: Command,
    timeout: Duration,
    max_output_bytes: usize,
    on_output: Option<OutputSink>,
) -> Result<std::process::Output, SandboxError> {
    cmd.kill_on_drop(true);
    let mut child = cmd.spawn().map_err(SandboxError::Io)?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| SandboxError::SpawnFailed("stdout pipe was not configured".into()))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| SandboxError::SpawnFailed("stderr pipe was not configured".into()))?;
    let run = async {
        let (stdout, stderr) = tokio::try_join!(
            read_bounded(
                &mut stdout,
                max_output_bytes,
                on_output.clone(),
                OutputStream::Stdout
            ),
            read_bounded(
                &mut stderr,
                max_output_bytes,
                on_output.clone(),
                OutputStream::Stderr
            ),
        )?;
        let status = child.wait().await.map_err(SandboxError::Io)?;
        Ok(std::process::Output {
            status,
            stdout,
            stderr,
        })
    };
    match time::timeout(timeout, run).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => Err(error),
        Err(_) => Err(SandboxError::TimedOut(timeout.as_secs())),
    }
}

async fn read_bounded<R: AsyncRead + Unpin>(
    reader: &mut R,
    max_output_bytes: usize,
    on_output: Option<OutputSink>,
    stream: OutputStream,
) -> Result<Vec<u8>, SandboxError> {
    let mut output = Vec::with_capacity(max_output_bytes.min(8192));
    let mut chunk = [0u8; 8192];
    loop {
        let count = reader.read(&mut chunk).await.map_err(SandboxError::Io)?;
        if count == 0 {
            return Ok(output);
        }
        let available = max_output_bytes.saturating_sub(output.len());
        let take = count.min(available);
        if take > 0 {
            output.extend_from_slice(&chunk[..take]);
            if let Some(sink) = on_output.as_ref() {
                sink(stream, &chunk[..take]);
            }
        }
        if count > available {
            return Err(SandboxError::OutputLimitExceeded(max_output_bytes));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stops_commands_that_exceed_output_limit() {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("printf 12345")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let error = run_command_with_timeout(command, Duration::from_secs(5), 4)
            .await
            .expect_err("output must be bounded");
        assert!(matches!(error, SandboxError::OutputLimitExceeded(4)));
    }

    #[tokio::test]
    async fn live_output_callback_fires_before_the_command_returns() {
        use std::sync::{Arc, Mutex};

        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink_seen = Arc::clone(&seen);
        let sink: OutputSink = Arc::new(move |stream, bytes| {
            sink_seen
                .lock()
                .expect("output sink")
                .push((stream, String::from_utf8_lossy(bytes).into_owned()));
        });

        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("printf hello")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let output =
            run_command_with_timeout_and_output(command, Duration::from_secs(5), 64, Some(sink))
                .await
                .expect("command");
        assert_eq!(output.stdout, b"hello");
        let chunks = seen.lock().expect("output sink");
        assert!(
            chunks
                .iter()
                .any(|(stream, text)| *stream == OutputStream::Stdout && text.contains("hello")),
            "callback must receive stdout before return: {chunks:?}"
        );
    }
}
