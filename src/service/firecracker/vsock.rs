use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

use super::error::FirecrackerError;

const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

pub fn connect_to_guest(
    socket_path: &Path,
    guest_port: u32,
) -> Result<UnixStream, FirecrackerError> {
    let mut stream = UnixStream::connect(socket_path)?;
    stream.write_all(format!("CONNECT {guest_port}\n").as_bytes())?;

    let mut response = Vec::new();
    let mut byte = [0_u8; 1];
    while response.len() < 128 {
        stream.read_exact(&mut byte)?;
        response.push(byte[0]);
        if byte[0] == b'\n' {
            break;
        }
    }
    let response = String::from_utf8_lossy(&response);
    if !response.starts_with("OK") {
        return Err(FirecrackerError::Guest(format!(
            "vsock connection rejected: {}",
            response.trim()
        )));
    }
    Ok(stream)
}

pub fn send_json_line<T: serde::Serialize>(
    stream: &mut impl Write,
    value: &T,
) -> Result<(), FirecrackerError> {
    let mut data = serde_json::to_vec(value)?;
    data.push(b'\n');
    if data.len() > MAX_FRAME_BYTES {
        return Err(FirecrackerError::Guest("request is too large".into()));
    }
    stream.write_all(&data)?;
    stream.flush()?;
    Ok(())
}

pub fn receive_json_line<T: serde::de::DeserializeOwned>(
    stream: &mut impl Read,
) -> Result<T, FirecrackerError> {
    let mut data = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        stream.read_exact(&mut byte)?;
        if byte[0] == b'\n' {
            break;
        }
        data.push(byte[0]);
        if data.len() > MAX_FRAME_BYTES {
            return Err(FirecrackerError::Guest("response is too large".into()));
        }
    }
    serde_json::from_slice(&data).map_err(FirecrackerError::Json)
}

pub fn set_stream_timeout(stream: &UnixStream, timeout: std::time::Duration) -> io::Result<()> {
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::{receive_json_line, send_json_line};

    #[test]
    fn json_line_round_trip() {
        let value = serde_json::json!({"kind": "python", "code": "print(42)"});
        let mut encoded = Vec::new();
        send_json_line(&mut encoded, &value).expect("encode request");
        assert_eq!(encoded.last(), Some(&b'\n'));

        let decoded: serde_json::Value =
            receive_json_line(&mut Cursor::new(encoded)).expect("decode request");
        assert_eq!(decoded["code"], "print(42)");
    }

    #[test]
    fn response_rejects_oversized_frames() {
        let mut frame = vec![b'a'; 64 * 1024 * 1024 + 1];
        frame.push(b'\n');
        let error = receive_json_line::<serde_json::Value>(&mut Cursor::new(frame))
            .expect_err("oversized frame must fail");
        assert!(error.to_string().contains("too large"));
    }
}
