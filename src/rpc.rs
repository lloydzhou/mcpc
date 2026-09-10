use crate::util;
use std::env;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const DAEMON_START_MS: u64 = 5_000;
const CLIENT_TIMEOUT_MS: u64 = 30_000;
const MAX_LINE: usize = 4 * 1024 * 1024;

fn dial() -> Option<UnixStream> {
    let path = util::socket_path();
    UnixStream::connect(path).ok()
}

fn daemon_pid() -> Option<u32> {
    let text = util::read_file(util::pid_path()).ok()?;
    text.trim().parse::<u32>().ok()
}

/// Start the daemon in the background if it is not already running.
pub fn ensure_daemon() -> Result<(), String> {
    if dial().is_some() {
        return Ok(());
    }

    // Self-heal: kill any stale daemon that still owns its pid but is not answering.
    if let Some(pid) = daemon_pid() {
        if pid != std::process::id() && util::process_exists(pid) {
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGTERM);
            }
            let start = Instant::now();
            while (start.elapsed().as_millis() as u64) < DAEMON_START_MS {
                if !util::process_exists(pid) {
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
    }

    let exe = env::current_exe().map_err(|e| format!("cannot find executable: {}", e))?;
    let _child = unsafe {
        Command::new(&exe)
            .arg("daemon")
            .arg("run")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .pre_exec(|| {
                libc::setsid();
                close_fds_from(3);
                Ok(())
            })
            .spawn()
            .map_err(|e| format!("cannot spawn daemon: {}", e))?
    };

    let start = Instant::now();
    while (start.elapsed().as_millis() as u64) < DAEMON_START_MS {
        if dial().is_some() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err("daemon did not come up".to_string())
}

unsafe fn close_fds_from(first: libc::c_int) {
    let mut lim: libc::rlimit = std::mem::zeroed();
    let max = if libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) == 0 && lim.rlim_cur <= 65536 {
        lim.rlim_cur as libc::c_int
    } else {
        65536
    };
    for fd in first..max {
        libc::close(fd);
    }
}

/// Send one line JSON request to the daemon and return the response line.
pub fn rpc(line: &str) -> Option<String> {
    let mut stream = match dial() {
        Some(s) => s,
        None => {
            ensure_daemon().ok()?;
            dial()?
        }
    };

    stream.write_all(line.as_bytes()).ok()?;
    stream.write_all(b"\n").ok()?;
    stream.flush().ok()?;

    let mut reader = BufReader::new(stream);
    let start = Instant::now();
    let mut line = String::new();
    loop {
        match reader.read_line(&mut line) {
            Ok(0) => return None,
            Ok(_) => {
                while line.ends_with('\n') || line.ends_with('\r') {
                    line.pop();
                }
                if line.len() > MAX_LINE {
                    return None;
                }
                return Some(line);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if start.elapsed().as_millis() as u64 >= CLIENT_TIMEOUT_MS {
                    return None;
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return None,
        }
    }
}

/// Stop the daemon and wait for the socket to disappear.
pub fn stop_daemon() -> Result<(), String> {
    let _ = rpc(r#"{"op":"stop"}"#).ok_or("stop request failed")?;
    let sock = util::socket_path();
    let start = Instant::now();
    while (start.elapsed().as_millis() as u64) < DAEMON_START_MS {
        if !sock.exists() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}
