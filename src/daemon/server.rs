use std::fs;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, ensure};
use tao::event_loop::EventLoopProxy;

use crate::daemon::UserEvent;
use crate::paths;
use crate::protocol::{Request, Response};

/// Binds the socket and answers requests from a background thread. Each
/// request is handed to the main thread, which owns all state, and the reply
/// comes back over a one-shot channel.
pub fn start(proxy: EventLoopProxy<UserEvent>) -> Result<()> {
    let path = paths::socket_path();
    let _ = fs::remove_file(&path);
    let listener = UnixListener::bind(&path).with_context(|| format!("bind {}", path.display()))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    let active = Arc::new(AtomicUsize::new(0));
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            if active.load(Ordering::Acquire) >= 16 {
                continue;
            }
            active.fetch_add(1, Ordering::AcqRel);
            let active = active.clone();
            let proxy = proxy.clone();
            thread::spawn(move || {
                if let Err(e) = handle(stream, proxy) {
                    eprintln!("socket request failed: {e}");
                }
                active.fetch_sub(1, Ordering::AcqRel);
            });
        }
    });
    Ok(())
}

/// How long after reading a request the event loop may still apply it.
const APPLY_TIMEOUT: Duration = Duration::from_secs(2);
const _: () = assert!(
    crate::client::REQUEST_TIMEOUT.as_millis() > APPLY_TIMEOUT.as_millis(),
    "a client must outwait the daemon applying its request"
);

fn handle(mut stream: UnixStream, proxy: EventLoopProxy<UserEvent>) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let line = read_request(&mut stream, deadline)?;
    let response = match serde_json::from_slice::<Request>(&line) {
        Ok(request) => {
            let (reply, receiver) = mpsc::channel();
            proxy
                .send_event(UserEvent::Request { request, reply, deadline: Instant::now() + APPLY_TIMEOUT })
                .map_err(|_| anyhow::anyhow!("event loop closed"))?;
            receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()))?
        }
        Err(e) => Response::Error { message: format!("bad request: {e}") },
    };
    let mut out = serde_json::to_string(&response)?;
    out.push('\n');
    stream.set_nonblocking(false)?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.write_all(out.as_bytes())?;
    Ok(())
}

fn read_request(stream: &mut UnixStream, deadline: Instant) -> Result<Vec<u8>> {
    stream.set_nonblocking(true)?;
    let mut line = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| anyhow!("socket request timed out"))?;
        let mut poll = libc::pollfd { fd: stream.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        let ready = unsafe { libc::poll(&raw mut poll, 1, remaining.as_millis().clamp(1, i32::MAX as u128) as i32) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error.into());
        }
        if ready == 0 {
            continue;
        }
        let read = match stream.read(&mut buffer) {
            Ok(read) => read,
            Err(error) if matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) => {
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        ensure!(read > 0, "incomplete socket request");
        let end = buffer[..read].iter().position(|&byte| byte == b'\n');
        line.extend_from_slice(&buffer[..end.unwrap_or(read)]);
        ensure!(line.len() <= 64 * 1024, "socket request exceeds 64 KiB limit");
        if end.is_some() {
            return Ok(line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_require_a_delimiter_and_obey_size_and_time_limits() {
        let (mut server, mut client) = UnixStream::pair().unwrap();
        client.write_all(b"{\"type\":\"status\"}\n").unwrap();
        assert_eq!(
            read_request(&mut server, Instant::now() + Duration::from_secs(1)).unwrap(),
            b"{\"type\":\"status\"}"
        );
        assert!(read_request(&mut server, Instant::now() + Duration::from_millis(20)).is_err());
        let writer = thread::spawn(move || {
            let _ = client.write_all(&vec![b'x'; 65 * 1024]);
        });
        let error = read_request(&mut server, Instant::now() + Duration::from_secs(1)).unwrap_err();
        assert!(error.to_string().contains("limit"), "{error:#}");
        drop(server);
        writer.join().unwrap();
    }
}
