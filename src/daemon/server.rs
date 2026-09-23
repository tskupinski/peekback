use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
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
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let proxy = proxy.clone();
            thread::spawn(move || {
                if let Err(e) = handle(stream, proxy) {
                    eprintln!("socket request failed: {e}");
                }
            });
        }
    });
    Ok(())
}

fn handle(mut stream: UnixStream, proxy: EventLoopProxy<UserEvent>) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut line = String::new();
    BufReader::new(stream.try_clone()?).read_line(&mut line)?;
    let response = match serde_json::from_str::<Request>(line.trim()) {
        Ok(request) => {
            let (reply, receiver) = mpsc::channel();
            proxy
                .send_event(UserEvent::Request { request, reply })
                .map_err(|_| anyhow::anyhow!("event loop closed"))?;
            receiver.recv()?
        }
        Err(e) => Response::Error { message: format!("bad request: {e}") },
    };
    let mut out = serde_json::to_string(&response)?;
    out.push('\n');
    stream.write_all(out.as_bytes())?;
    Ok(())
}
