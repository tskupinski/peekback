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

/// The agent in the foreground of the terminal a process is attached to, such
/// as a pane's shell. It is named by the command it was started as: the
/// native Claude Code executable is a file named after its version.
pub fn foreground_agent(pid: u32) -> Option<session_activity::Agent> {
    let leader = info(pid)?.foreground_group;
    agent_named(started_as(leader)?.rsplit('/').next()?)
}

fn agent_named(name: &str) -> Option<session_activity::Agent> {
    match name {
        "claude" => Some(session_activity::Agent::Claude),
        "codex" => Some(session_activity::Agent::Codex),
        _ => None,
    }
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
    foreground_group: u32,
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
        foreground_group: bsd.e_tpgid,
        name,
        // NODEV (all ones) when the process has no controlling terminal.
        tty: (bsd.e_tdev != u32::MAX).then_some(u64::from(bsd.e_tdev)),
    })
}

#[cfg(not(target_os = "macos"))]
fn info(_pid: u32) -> Option<Info> {
    None
}

/// `argv[0]`. `KERN_PROCARGS2` holds `argc`, the executable path, padding,
/// then the arguments, each NUL-terminated.
#[cfg(target_os = "macos")]
fn started_as(pid: u32) -> Option<String> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];
    let mut size: libc::size_t = 0;
    let null = std::ptr::null_mut();
    if unsafe { libc::sysctl(mib.as_mut_ptr(), 3, null, &mut size, null, 0) } != 0 {
        return None;
    }
    let mut buffer = vec![0u8; size];
    if unsafe { libc::sysctl(mib.as_mut_ptr(), 3, buffer.as_mut_ptr().cast(), &mut size, null, 0) } != 0 {
        return None;
    }
    let after_count = buffer.get(size_of::<libc::c_int>()..size)?;
    let after_path = &after_count[after_count.iter().position(|&b| b == 0)?..];
    let argument = &after_path[after_path.iter().position(|&b| b != 0)?..];
    let end = argument.iter().position(|&b| b == 0).unwrap_or(argument.len());
    Some(String::from_utf8_lossy(&argument[..end]).into_owned())
}

#[cfg(not(target_os = "macos"))]
fn started_as(_pid: u32) -> Option<String> {
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

    #[test]
    fn a_process_is_named_by_what_it_was_started_as() {
        let me = started_as(std::process::id()).unwrap();
        assert!(me.contains("peekback"), "{me}");
        assert_eq!(agent_named("codex"), Some(session_activity::Agent::Codex));
        assert_eq!(agent_named("2.1.282"), None);
    }
}
