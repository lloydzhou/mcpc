use crate::util;
use std::env;
use std::io::{BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const DAEMON_START_MS: u64 = 5_000;
/// Default deadline for one daemon round trip. A slow MCP server can spend
/// minutes inside a single tools/list, so this is generous; a stuck daemon
/// still returns control instead of hanging the shell forever.
const CLIENT_TIMEOUT_MS: u64 = 600_000;
// A sessions cache with a large tool list runs to tens of MB; the daemon
// writes it on one line, so the reader must accept it.
const MAX_LINE: usize = 256 * 1024 * 1024;
const POLL_READ_MS: u64 = 1_000;

fn dial() -> Option<UnixStream> {
    let path = util::socket_path();
    UnixStream::connect(path).ok()
}

/// Whether a daemon answers on the socket, without starting one.
pub fn is_running() -> bool {
    dial().is_some()
}

/// Wait until the daemon that served the socket before the restart is really
/// gone: the pid file names it, and a new daemon may reuse the same socket path.
pub fn wait_pid_gone(pid: Option<u32>, timeout_ms: u64) {
    let pid = match pid {
        Some(p) if p != std::process::id() => p,
        _ => return,
    };
    let start = Instant::now();
    while (start.elapsed().as_millis() as u64) < timeout_ms {
        if !util::process_exists(pid) || daemon_pid() != Some(pid) {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

pub fn daemon_pid() -> Option<u32> {
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
    rpc_with_timeout(line, CLIENT_TIMEOUT_MS)
}

/// Same as `rpc`, with an explicit deadline in milliseconds.
pub fn rpc_with_timeout(line: &str, timeout_ms: u64) -> Option<String> {
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

    // A read timeout makes the socket return WouldBlock instead of blocking
    // forever, so the deadline below is enforced: a daemon stuck in a slow
    // handshake can no longer hang every client command indefinitely.
    stream
        .set_read_timeout(Some(Duration::from_millis(POLL_READ_MS)))
        .ok()?;

    let mut reader = BufReader::new(stream);
    let start = Instant::now();
    // Read in chunks: read_line() would discard a partial line when the read
    // timeout fires, losing an already-started large response. Bytes are
    // accumulated first so a multi-byte character split across two chunks is
    // decoded whole when the line completes.
    let mut raw = Vec::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => return None,
            Ok(n) => {
                raw.extend_from_slice(&buf[..n]);
                if raw.len() > MAX_LINE {
                    return None;
                }
                if let Some(pos) = raw.iter().position(|b| *b == b'\n') {
                    let mut end = pos;
                    if end > 0 && raw[end - 1] == b'\r' {
                        end -= 1;
                    }
                    let line = String::from_utf8(raw[..end].to_vec()).ok()?;
                    return Some(line);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if start.elapsed().as_millis() as u64 >= timeout_ms {
                    return None;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
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
