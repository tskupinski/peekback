use serde::{Deserialize, Serialize};

/// A process identity that survives PID reuse: a recycled PID gets a
/// different start time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessId {
    pub pid: u32,
    pub started_at_us: u64,
}

impl ProcessId {
    pub fn is_running(self) -> bool {
        info(self.pid).is_some_and(|p| p.id == self)
    }

    pub fn tty(self) -> Option<u64> {
        info(self.pid).filter(|p| p.id == self).and_then(|p| p.tty)
    }

    pub fn foreground_tty(self) -> Option<u64> {
        info(self.pid).filter(|p| p.id == self && p.foreground).and_then(|p| p.tty)
    }
}

/// The agent that ran this hook: the nearest ancestor that is not a shell.
/// Agents run hook commands through a shell, so the direct parent is usually
/// `sh`.
pub fn hook_agent() -> Option<ProcessId> {
    let mut pid = std::os::unix::process::parent_id();
    for _ in 0..8 {
        let process = info(pid)?;
        if !is_shell(&process.name) {
            return Some(process.id);
        }
        pid = process.parent;
    }
    None
}

fn is_shell(name: &str) -> bool {
    matches!(name, "sh" | "bash" | "zsh" | "dash" | "fish" | "ksh")
}

struct Info {
    id: ProcessId,
    parent: u32,
    name: String,
    tty: Option<u64>,
    foreground: bool,
}

#[cfg(target_os = "macos")]
fn info(pid: u32) -> Option<Info> {
    use std::ffi::CStr;
    use std::mem::{size_of, zeroed};

    if pid <= 1 {
        return None;
    }
    let mut bsd: libc::proc_bsdinfo = unsafe { zeroed() };
    let size = size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let read =
        unsafe { libc::proc_pidinfo(pid as libc::c_int, libc::PROC_PIDTBSDINFO, 0, (&raw mut bsd).cast(), size) };
    if read != size {
        return None;
    }
    let name = unsafe { CStr::from_ptr(bsd.pbi_comm.as_ptr()) }.to_string_lossy().into_owned();
    Some(Info {
        id: ProcessId { pid, started_at_us: bsd.pbi_start_tvsec * 1_000_000 + bsd.pbi_start_tvusec },
        foreground: bsd.pbi_pgid != 0
            && bsd.pbi_pgid == bsd.e_tpgid
            && matches!(bsd.pbi_status, libc::SRUN | libc::SSLEEP),
        parent: bsd.pbi_ppid,
        name,
        // NODEV (all ones) when the process has no controlling terminal.
        tty: (bsd.e_tdev != u32::MAX).then_some(u64::from(bsd.e_tdev)),
    })
}

#[cfg(not(target_os = "macos"))]
fn info(_pid: u32) -> Option<Info> {
    None
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn a_reused_pid_is_not_the_same_process() {
        let me = info(std::process::id()).unwrap().id;
        assert!(me.is_running());
        assert!(!ProcessId { started_at_us: me.started_at_us + 1, ..me }.is_running());
        assert!(!ProcessId { pid: u32::MAX, started_at_us: 0 }.is_running());
    }

    #[test]
    fn a_stopped_process_keeps_its_tty_but_cannot_receive_delivery() {
        use std::os::fd::FromRawFd;
        use std::os::unix::process::CommandExt;
        use std::process::Command;

        let (mut master, mut slave) = (-1, -1);
        assert_eq!(
            unsafe {
                libc::openpty(
                    &raw mut master,
                    &raw mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            },
            0
        );
        let _master = unsafe { std::fs::File::from_raw_fd(master) };
        let slave = unsafe { std::fs::File::from_raw_fd(slave) };
        let mut command = Command::new("/bin/sleep");
        command.arg("30").stdin(slave.try_clone().unwrap()).stdout(slave.try_clone().unwrap()).stderr(slave);
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1
                    || libc::ioctl(0, libc::TIOCSCTTY.into(), 0) == -1
                    || libc::tcsetpgrp(0, libc::getpgrp()) == -1
                {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn().unwrap();
        let agent = info(child.id()).unwrap().id;
        let foreground = agent.foreground_tty();
        let stopped = unsafe { libc::kill(child.id() as i32, libc::SIGSTOP) };
        let mut status = 0;
        if stopped == 0 {
            unsafe { libc::waitpid(child.id() as i32, &raw mut status, libc::WUNTRACED) };
        }
        let still_alive = agent.is_running();
        let still_has_tty = agent.tty();
        let deliverable = agent.foreground_tty();
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(stopped, 0);
        assert!(foreground.is_some());
        assert!(still_alive);
        assert_eq!(still_has_tty, foreground);
        assert_eq!(deliverable, None);
    }

    #[test]
    fn shells_are_skipped_when_finding_the_agent() {
        assert!(is_shell("zsh") && is_shell("sh"));
        assert!(!is_shell("claude") && !is_shell("codex") && !is_shell("node"));
    }
}
