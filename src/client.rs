use std::fs::{DirBuilder, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, ensure};

use crate::harness::Harness;
use crate::paths;
use crate::protocol::{Outgoing, Request, Response, unix_ms};

const START_TIMEOUT: Duration = Duration::from_secs(3);
/// Counted from the connection. The request carries the same expiry, so the
/// daemon never acts on one its client already gave up on.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_REPLY_BYTES: usize = 1024 * 1024;

pub fn request(request: &Request) -> Result<Response> {
    let stream = UnixStream::connect(paths::socket_path()).context("daemon not running")?;
    exchange(stream, request, Instant::now() + REQUEST_TIMEOUT)
}

fn remaining(deadline: Instant) -> Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| anyhow!("daemon request timed out"))
}

fn exchange(mut stream: UnixStream, request: &Request, deadline: Instant) -> Result<Response> {
    let expires_at = std::time::SystemTime::now() + deadline.saturating_duration_since(Instant::now());
    let mut line = serde_json::to_string(&Outgoing { request, expires_at_ms: unix_ms(expires_at) })?;
    line.push('\n');
    let mut pending = line.as_bytes();
    while !pending.is_empty() {
        stream.set_write_timeout(Some(remaining(deadline)?))?;
        let written = stream.write(pending).context("writing daemon request (timeout or closed connection)")?;
        ensure!(written > 0, "daemon closed connection while writing request");
        pending = &pending[written..];
    }
    stream.shutdown(std::net::Shutdown::Write)?;

    let mut reply = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        stream.set_read_timeout(Some(remaining(deadline)?))?;
        let count = stream.read(&mut buffer).context("waiting for daemon reply (timeout or closed connection)")?;
        ensure!(count > 0, "daemon closed connection before completing its reply");
        let end = buffer[..count].iter().position(|&byte| byte == b'\n');
        reply.extend_from_slice(&buffer[..end.unwrap_or(count)]);
        ensure!(reply.len() <= MAX_REPLY_BYTES, "daemon reply exceeds 1 MiB limit");
        if end.is_some() {
            return serde_json::from_slice(&reply).context("malformed reply from daemon");
        }
    }
}

pub fn request_starting_daemon(request: &Request) -> Result<Response> {
    let deadline = Instant::now() + START_TIMEOUT;
    let path = paths::socket_path();
    match UnixStream::connect(&path) {
        Ok(stream) => return exchange(stream, request, Instant::now() + REQUEST_TIMEOUT),
        Err(error) if unavailable(&error) => spawn_daemon()?,
        Err(error) => return Err(error).context("connecting to daemon"),
    }
    loop {
        match UnixStream::connect(&path) {
            // Once connected, never retry a request or spawn another daemon:
            // it may have already acted on a request whose reply was delayed.
            Ok(stream) => return exchange(stream, request, Instant::now() + REQUEST_TIMEOUT),
            Err(error) if !unavailable(&error) => return Err(error).context("connecting to daemon"),
            Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            Err(_) => {
                return Err(anyhow!(
                    "daemon did not start within {:?}, see {}",
                    START_TIMEOUT,
                    paths::log_path().display()
                ));
            }
        }
    }
}

fn unavailable(error: &std::io::Error) -> bool {
    matches!(error.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused)
}

fn spawn_daemon() -> Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(paths::state_dir())?;
    let log = OpenOptions::new().create(true).append(true).mode(0o600).open(paths::log_path())?;
    let mut command = Command::new(std::env::current_exe()?);
    // The daemon serves every session and pane; it must not inherit the ones
    // this shell happens to run inside.
    for key in Harness::ALL
        .into_iter()
        .flat_map(|harness| harness.session_env().iter().copied())
        .chain(crate::mux::Mux::ALL.into_iter().flat_map(|mux| mux.ambient_env().iter().copied()))
    {
        command.env_remove(key);
    }
    command.arg("daemon").stdin(Stdio::null()).stdout(Stdio::from(log.try_clone()?)).stderr(Stdio::from(log));
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    command.spawn().context("failed to spawn daemon")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_replies_are_bounded_even_when_partial_bytes_keep_arriving() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let writer = thread::spawn(move || {
            let mut request = Vec::new();
            server.read_to_end(&mut request).unwrap();
            for _ in 0..100 {
                if server.write_all(b" ").is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
        });
        let start = Instant::now();
        let error = exchange(client, &Request::Status, start + Duration::from_millis(40)).unwrap_err();
        assert!(format!("{error:#}").contains("time"));
        assert!(start.elapsed() < Duration::from_millis(400));
        writer.join().unwrap();
    }

    #[test]
    fn daemon_request_handles_success_eof_and_silent_server() {
        let (client, mut server) = UnixStream::pair().unwrap();
        server.write_all(b"{\"type\":\"ok\"}\n").unwrap();
        assert!(matches!(exchange(client, &Request::Status, Instant::now() + START_TIMEOUT).unwrap(), Response::Ok));
        let (client, _silent_server) = UnixStream::pair().unwrap();
        assert!(exchange(client, &Request::Status, Instant::now() + Duration::from_millis(30)).is_err());
        let (client, mut server) = UnixStream::pair().unwrap();
        let reader = thread::spawn(move || {
            let mut request = Vec::new();
            server.read_to_end(&mut request).unwrap();
        });
        let error = exchange(client, &Request::Status, Instant::now() + START_TIMEOUT).unwrap_err();
        reader.join().unwrap();
        assert!(error.to_string().contains("before completing"), "{error:#}");
    }
}
