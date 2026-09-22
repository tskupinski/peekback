use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};

use crate::paths;
use crate::protocol::{Request, Response};

const START_TIMEOUT: Duration = Duration::from_secs(3);

pub fn request(request: &Request) -> Result<Response> {
    let mut stream = UnixStream::connect(paths::socket_path()).context("daemon not running")?;
    let mut line = serde_json::to_string(request)?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    stream.shutdown(std::net::Shutdown::Write)?;

    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply)?;
    serde_json::from_str(reply.trim()).context("malformed reply from daemon")
}

pub fn request_starting_daemon(request: &Request) -> Result<Response> {
    if let Ok(response) = self::request(request) {
        return Ok(response);
    }
    spawn_daemon()?;
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        match self::request(request) {
            Ok(response) => return Ok(response),
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

fn spawn_daemon() -> Result<()> {
    std::fs::create_dir_all(paths::state_dir())?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths::log_path())?;
    Command::new(std::env::current_exe()?)
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .process_group(0)
        .spawn()
        .context("failed to spawn daemon")?;
    Ok(())
}
