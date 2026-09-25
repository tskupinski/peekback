use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};

const TIMEOUT: Duration = Duration::from_secs(2);
const MAX_OUTPUT: usize = 4 * 1024 * 1024;

pub(super) fn output(cmd: &mut Command) -> Result<String> {
    execute(cmd, "", TIMEOUT)
}

pub(super) fn run(cmd: &mut Command) -> Result<()> {
    output(cmd).map(drop)
}

pub(super) fn run_with_stdin(cmd: &mut Command, input: &str) -> Result<()> {
    execute(cmd, input, TIMEOUT).map(drop)
}

fn nonblocking(stream: &impl AsRawFd) -> Result<()> {
    let fd = stream.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

fn drain(stream: &mut impl Read, bytes: &mut Vec<u8>) -> Result<bool> {
    let mut buffer = [0; 8192];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => return Ok(true),
            Ok(n) => {
                ensure!(bytes.len() + n <= MAX_OUTPUT, "terminal command output exceeds limit");
                bytes.extend_from_slice(&buffer[..n]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(false),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
}

fn execute(cmd: &mut Command, input: &str, timeout: Duration) -> Result<String> {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .with_context(|| format!("run {:?}", cmd.get_program()))?;
    let result = (|| -> Result<String> {
        let mut stdin = child.stdin.take();
        let mut stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");
        nonblocking(stdin.as_ref().expect("piped stdin"))?;
        nonblocking(&stdout)?;
        nonblocking(&stderr)?;
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut remaining = input.as_bytes();
        let deadline = Instant::now() + timeout;
        loop {
            ensure!(Instant::now() < deadline, "terminal command timed out; delivery may be incomplete");
            if remaining.is_empty() {
                stdin.take();
            } else if let Some(pipe) = stdin.as_mut() {
                match pipe.write(remaining) {
                    Ok(0) => bail!("terminal command closed stdin"),
                    Ok(n) => remaining = &remaining[n..],
                    Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) => {}
                    Err(e) => return Err(e.into()),
                }
            }
            let out_done = drain(&mut stdout, &mut out)?;
            let err_done = drain(&mut stderr, &mut err)?;
            if let Some(status) = child.try_wait()? {
                if out_done && err_done {
                    ensure!(
                        status.success(),
                        "{:?} failed: {}",
                        cmd.get_program(),
                        String::from_utf8_lossy(&err).trim()
                    );
                    ensure!(remaining.is_empty(), "terminal command exited before accepting all text");
                    return Ok(String::from_utf8_lossy(&out).trim().to_owned());
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    })();
    if result.is_err() {
        // Descendants can keep the pipes open after the command itself exits.
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
        let _ = child.wait();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_io_is_drained_while_writing() {
        let input = "x".repeat(200_000);
        assert_eq!(execute(Command::new("cat").arg("-"), &input, TIMEOUT).unwrap(), input);
        assert!(
            output(Command::new("sh").args(["-c", "echo error >&2; exit 1"]))
                .unwrap_err()
                .to_string()
                .contains("error")
        );
    }

    #[test]
    fn commands_and_inherited_pipes_have_a_deadline() {
        for script in ["exec sleep 10", "sleep 10 & exit 0"] {
            let started = Instant::now();
            let error = execute(Command::new("sh").args(["-c", script]), "", Duration::from_millis(50)).unwrap_err();
            assert!(error.to_string().contains("timed out"));
            assert!(started.elapsed() < Duration::from_secs(1));
        }
    }
}
