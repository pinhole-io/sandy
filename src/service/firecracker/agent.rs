use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::fd::FromRawFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, Stdio};

use super::protocol::{GuestRequest, GuestResponse};

const AGENT_PORT: u32 = 52;
const VMADDR_CID_ANY: u32 = u32::MAX;

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let listener = bind_vsock(AGENT_PORT)?;
    loop {
        let (stream, _) = listener.accept()?;
        if let Err(error) = handle_connection(stream) {
            tracing::warn!("Firecracker guest agent request failed: {error}");
        }
    }
}

fn handle_connection(stream: UnixStream) -> Result<(), Box<dyn std::error::Error>> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request = String::new();
    reader.read_line(&mut request)?;
    let request: GuestRequest = serde_json::from_str(request.trim())?;
    let response = execute(request);
    let mut stream = stream;
    serde_json::to_writer(&mut stream, &response)?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(())
}

fn execute(request: GuestRequest) -> GuestResponse {
    let result = match request {
        GuestRequest::Python { code } => run_program("exec-code", &["--code", &code], &[]),
        GuestRequest::Javascript { code } => run_program("exec-js", &["--code", &code], &[]),
        GuestRequest::Shell {
            command,
            workspace_hex,
            env,
        } => run_shell(&command, &workspace_hex, &env),
    };

    match result {
        Ok((stdout, stderr, exit_code, workspace_hex)) => GuestResponse {
            stdout,
            stderr,
            exit_code,
            workspace_hex,
            error: None,
        },
        Err(error) => GuestResponse {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 1,
            workspace_hex: None,
            error: Some(error),
        },
    }
}

fn run_program(
    subcommand: &str,
    args: &[&str],
    env: &[(String, String)],
) -> Result<(String, String, i32, Option<String>), String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut command = Command::new(executable);
    command
        .arg(subcommand)
        .args(args)
        .stdin(Stdio::null())
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("HOME", "/workspace")
        .env("SANDY_IN_FIRECRACKER", "1");
    for (key, value) in env {
        command.env(key, value);
    }
    let output = command.output().map_err(|error| error.to_string())?;
    Ok((
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code().unwrap_or(1),
        None,
    ))
}

fn run_shell(
    command: &str,
    workspace_hex: &str,
    env: &[(String, String)],
) -> Result<(String, String, i32, Option<String>), String> {
    let archive = decode_hex(workspace_hex)?;
    let archive_path = Path::new("/tmp/sandy-workspace.tar");
    fs::write(archive_path, archive).map_err(|error| error.to_string())?;
    fs::create_dir_all("/workspace").map_err(|error| error.to_string())?;
    let unpack = Command::new("tar")
        .args(["-xf", "/tmp/sandy-workspace.tar", "-C", "/workspace"])
        .output()
        .map_err(|error| error.to_string())?;
    if !unpack.status.success() {
        return Err(String::from_utf8_lossy(&unpack.stderr).into_owned());
    }

    let mut shell = Command::new("/bin/sh");
    shell
        .args(["-c", &format!("cd /workspace && {command}")])
        .stdin(Stdio::null())
        .env_clear();
    for (key, value) in env {
        shell.env(key, value);
    }
    let output = shell.output().map_err(|error| error.to_string())?;

    let archive = Command::new("tar")
        .args([
            "-cf",
            "/tmp/sandy-workspace-out.tar",
            "-C",
            "/workspace",
            ".",
        ])
        .output()
        .map_err(|error| error.to_string())?;
    let workspace_hex = if archive.status.success() {
        Some(encode_hex(
            &fs::read("/tmp/sandy-workspace-out.tar").map_err(|error| error.to_string())?,
        ))
    } else {
        None
    };

    Ok((
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code().unwrap_or(1),
        workspace_hex,
    ))
}

fn bind_vsock(port: u32) -> Result<std::os::unix::net::UnixListener, Box<dyn std::error::Error>> {
    let fd = unsafe { libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }

    let mut address: libc::sockaddr_vm = unsafe { std::mem::zeroed() };
    address.svm_family = libc::AF_VSOCK as libc::sa_family_t;
    address.svm_port = port;
    address.svm_cid = VMADDR_CID_ANY;
    let result = unsafe {
        libc::bind(
            fd,
            (&address as *const libc::sockaddr_vm).cast(),
            std::mem::size_of::<libc::sockaddr_vm>() as libc::socklen_t,
        )
    };
    if result < 0 {
        let error = std::io::Error::last_os_error();
        unsafe { libc::close(fd) };
        return Err(error.into());
    }
    if unsafe { libc::listen(fd, 16) } < 0 {
        let error = std::io::Error::last_os_error();
        unsafe { libc::close(fd) };
        return Err(error.into());
    }
    Ok(unsafe { std::os::unix::net::UnixListener::from_raw_fd(fd) })
}

fn decode_hex(value: &str) -> Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(2) {
        return Err("workspace archive has invalid hex length".into());
    }
    (0..value.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&value[index..index + 2], 16)
                .map_err(|_| "workspace archive has invalid hex".to_string())
        })
        .collect()
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push_str(&format!("{byte:02x}"));
    }
    result
}
