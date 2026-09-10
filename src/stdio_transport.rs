use crate::jsonrpc;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const STDIO_TIMEOUT_MS: u64 = 30_000;
const MAX_LINE: usize = 4 * 1024 * 1024;

pub struct StdioTransport {
    _child: Child,
    stdin: Mutex<ChildStdin>,
    stdout: Arc<Mutex<BufReader<ChildStdout>>>,
}

impl StdioTransport {
    /// Spawn a stdio MCP server via `sh -c COMMAND`.
    pub fn spawn(cmd: &str) -> std::io::Result<Self> {
        let mut child = unsafe {
            Command::new("/bin/sh")
                .arg("-c")
                .arg(cmd)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .pre_exec(|| {
                    libc::setsid();
                    Ok(())
                })
                .spawn()?
        };
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        Ok(Self {
            _child: child,
            stdin: Mutex::new(stdin),
            stdout: Arc::new(Mutex::new(BufReader::new(stdout))),
        })
    }

    /// Send a JSON-RPC request and wait for the response matching `want_id`.
    pub fn roundtrip(&self, request: &str, want_id: i64) -> Option<String> {
        {
            let mut stdin = self.stdin.lock().ok()?;
            stdin.write_all(request.as_bytes()).ok()?;
            stdin.write_all(b"\n").ok()?;
            stdin.flush().ok()?;
        }

        let stdout = Arc::clone(&self.stdout);
        loop {
            let line = read_line_timeout(Arc::clone(&stdout), STDIO_TIMEOUT_MS)?;
            if line.is_empty() {
                continue;
            }
            if let Some(resp) = jsonrpc::try_match_line(&line, want_id) {
                return Some(resp);
            }
            // Non-matching JSON-RPC line: notification or garbage; keep reading.
        }
    }

    /// Send a notification (no response expected).  Returns true on success.
    pub fn notify(&self, request: &str) -> bool {
        let mut stdin = match self.stdin.lock() {
            Ok(g) => g,
            Err(_) => return false,
        };
        stdin.write_all(request.as_bytes()).is_ok()
            && stdin.write_all(b"\n").is_ok()
            && stdin.flush().is_ok()
    }
}

/// Read one newline-terminated line from `reader` with a timeout.
fn read_line_timeout(reader: Arc<Mutex<BufReader<ChildStdout>>>, timeout_ms: u64) -> Option<String> {
    let (tx, rx) = std::sync::mpsc::channel::<Option<String>>();

    thread::spawn(move || {
        let guard = reader.lock();
        let res = match guard {
            Ok(mut g) => {
                let mut buf = String::new();
                match g.read_line(&mut buf) {
                    Ok(0) => None,
                    Ok(_) => {
                        while buf.ends_with('\n') || buf.ends_with('\r') {
                            buf.pop();
                        }
                        if buf.len() > MAX_LINE {
                            None
                        } else {
                            Some(buf)
                        }
                    }
                    Err(_) => None,
                }
            }
            Err(_) => None,
        };
        let _ = tx.send(res);
    });

    rx.recv_timeout(Duration::from_millis(timeout_ms)).unwrap_or_default()
}
