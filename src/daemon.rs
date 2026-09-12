use crate::session::{self, Session, SessionStore, Transport};
use crate::util;
use serde_json::{json, Value};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const MAX_LINE: usize = 4 * 1024 * 1024;
const CLIENT_TIMEOUT_MS: u64 = 30_000;
const POLL_INTERVAL_MS: u64 = 500;

static STOP: AtomicBool = AtomicBool::new(false);

pub fn run(store: Arc<SessionStore>) -> Result<(), String> {
    // Ignore SIGPIPE so a broken client socket does not kill the daemon.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }

    let home = util::mcpc_home();
    util::ensure_dir(&home).map_err(|e| format!("cannot create home dir: {}", e))?;

    let lock_path = util::lock_path();
    let lock_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .open(&lock_path)
        .map_err(|e| format!("cannot open lock file: {}", e))?;
    let lock_fd = lock_file.as_raw_fd();
    let locked = unsafe { libc::flock(lock_fd, libc::LOCK_EX | libc::LOCK_NB) } == 0;
    if !locked {
        return Err("another daemon already holds the lock".to_string());
    }

    let sock_path = util::socket_path();
    let pid_path = util::pid_path();
    let _ = std::fs::remove_file(&sock_path);

    let listener = UnixListener::bind(&sock_path)
        .map_err(|e| format!("cannot bind daemon socket: {}", e))?;
    let mut perms = std::fs::metadata(&sock_path)
        .map_err(|e| format!("cannot stat socket: {}", e))?
        .permissions();
    perms.set_mode(0o600);
    std::fs::set_permissions(&sock_path, perms)
        .map_err(|e| format!("cannot chmod socket: {}", e))?;

    listener
        .set_nonblocking(true)
        .map_err(|e| format!("cannot set nonblocking: {}", e))?;

    write_pid(&pid_path)?;

    // Reconnect cached sessions (servers/*.json) so authenticated headers
    // survive daemon restarts.
    session::restore_sessions(&store);

    while !STOP.load(Ordering::Relaxed) {
        // Retire dead sessions before accepting a new command.
        store.gc_dead();

        match listener.accept() {
            Ok((stream, _)) => {
                let store = Arc::clone(&store);
                thread::spawn(move || {
                    handle_connection(stream, store);
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
            }
            Err(_) => {
                thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
            }
        }
    }

    store.gc_dead();
    let _ = std::fs::remove_file(&sock_path);
    let _ = std::fs::remove_file(&pid_path);
    // lock_file dropped after flock is released automatically on close
    Ok(())
}

fn write_pid(path: &PathBuf) -> Result<(), String> {
    let mut f = File::create(path).map_err(|e| format!("cannot write pid file: {}", e))?;
    writeln!(f, "{}", std::process::id()).map_err(|e| format!("cannot write pid: {}", e))?;
    Ok(())
}

fn handle_connection(mut stream: UnixStream, store: Arc<SessionStore>) {
    let _ = stream.set_nonblocking(false);
    let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
    let line = match read_request_line(&mut reader) {
        Some(l) => l,
        None => return,
    };

    // Retire dead sessions from the previous request before handling this one.
    store.gc_dead();

    let response = handle_request(&line, &store);
    let mut resp_bytes = response.into_bytes();
    resp_bytes.push(b'\n');
    let _ = stream.write_all(&resp_bytes);
    let _ = stream.flush();
    // Give the kernel a moment to drain the socket buffer before closing.
    thread::sleep(Duration::from_millis(50));
    // Retire sessions this request may have marked dead.
    store.gc_dead();
}

fn read_request_line(reader: &mut BufReader<UnixStream>) -> Option<String> {
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

fn ok(result: Value) -> String {
    json!({"ok": true, "result": result}).to_string()
}

fn err<E: std::fmt::Display>(msg: E) -> String {
    json!({"ok": false, "error": msg.to_string()}).to_string()
}

#[derive(serde::Deserialize)]
struct DaemonRequest<'a> {
    #[serde(borrow)]
    op: &'a serde_json::value::RawValue,
    #[serde(borrow)]
    name: Option<&'a serde_json::value::RawValue>,
    #[serde(borrow)]
    target: Option<&'a serde_json::value::RawValue>,
    #[serde(borrow)]
    header: Option<&'a serde_json::value::RawValue>,
    #[serde(borrow)]
    protocol_version: Option<&'a serde_json::value::RawValue>,
    #[serde(borrow)]
    method: Option<&'a serde_json::value::RawValue>,
    #[serde(borrow)]
    params: Option<&'a serde_json::value::RawValue>,
}

fn raw_str<'a>(r: Option<&'a serde_json::value::RawValue>) -> Option<&'a str> {
    r.and_then(|v| {
        let s: String = serde_json::from_str(v.get()).ok()?;
        Some(Box::leak(s.into_boxed_str()) as &str)
    })
}

fn raw_string(r: Option<&serde_json::value::RawValue>) -> Option<String> {
    r.and_then(|v| serde_json::from_str::<String>(v.get()).ok())
}

fn handle_request(line: &str, store: &SessionStore) -> String {
    let req: DaemonRequest = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return err(format!("bad request: {}", e)),
    };
    let op = match raw_str(Some(req.op)) {
        Some(s) => s,
        None => return err("missing op"),
    };
    let name = raw_str(req.name);

    match op {
        "ping" => {
            let name = match name {
                Some(s) => s,
                None => return err("missing name"),
            };
            if store.with_session(name, |_s| Some(())).is_none() {
                return err(format!("no live session: {}", name));
            }
            ok(json!({"alive": true}))
        }
        "list" => {
            let mut arr = Vec::new();
            for name in store.names() {
                if let Some(info) = store.with_session(&name, |s| {
                    Some(json!({
                        "name": s.name.clone(),
                        "transport": match s.transport {
                            Transport::Http => "http",
                            Transport::Stdio => "stdio",
                        },
                        "pid": std::process::id() as i64,
                        "server_info": serde_json::from_str::<Value>(&s.server_info_json)
                            .unwrap_or_else(|_| Value::Object(Default::default())),
                    }))
                }) {
                    arr.push(info);
                }
            }
            ok(Value::Array(arr))
        }
        "connect" => {
            let target = raw_string(req.target).unwrap_or_default();
            let name = raw_string(req.name).unwrap_or_else(|| "default".to_string());
            let header = raw_string(req.header);
            let pver = raw_string(req.protocol_version);

            if target.is_empty() {
                return err("connect needs name+target");
            }

            // Check for duplicate live session.
            if store.with_session(&name, |_s| Some(())).is_some() {
                return err(format!("session '{}' already live", name));
            }

            let mut session = if target.starts_with("cmd:") {
                let cmd = &target[4..];
                Session::new_stdio(name.clone(), cmd.to_string(), pver)
            } else {
                let url = normalize_url(&target);
                let headers: Vec<String> = header.map(|h| vec![h]).unwrap_or_default();
                Session::new_http(name.clone(), url, headers, pver)
            };

            if let Err(e) = session.handshake() {
                return err(format!("handshake failed for @{}: {}", name, e));
            }

            let result = json!({
                "server_info": serde_json::from_str::<Value>(&session.server_info_json)
                    .unwrap_or_else(|_| Value::Object(Default::default())),
                "tools": serde_json::from_str::<Value>(&session.tools_json)
                    .unwrap_or_else(|_| Value::Array(vec![])),
            });
            session::cache_save(&name, &session.cache_json());
            store.insert(session);
            ok(result)
        }
        "close" => {
            let name = match name {
                Some(s) => s,
                None => return err("missing name"),
            };
            if store.remove(name) {
                ok(Value::Object(Default::default()))
            } else {
                err(format!("no such session: {}", name))
            }
        }
        "call" => {
            let name = match name {
                Some(s) => s,
                None => return err("missing name"),
            };
            let method = raw_string(req.method).unwrap_or_default();
            let params_str = req.params.map(|r| r.get()).unwrap_or("{}");

            let result =
                store.with_session(name, |s| Some(s.rpc_call(&method, params_str).ok_or(())));
            match result {
                None => err(format!("no live session: {}", name)),
                Some(Err(())) => err(format!("transport failed on @{}", name)),
                Some(Ok(resp)) => {
                    // Embed the raw JSON-RPC response object as the daemon result
                    // so field ordering from the server is preserved.
                    format!("{{\"ok\":true,\"result\":{}}}", resp)
                }
            }
        }
        "tools" => {
            let name = match name {
                Some(s) => s,
                None => return err("missing name"),
            };
            store
                .with_session(name, |s| {
                    Some(ok(json!({
                        "server_info": serde_json::from_str::<Value>(&s.server_info_json)
                            .unwrap_or_else(|_| Value::Object(Default::default())),
                        "tools": serde_json::from_str::<Value>(&s.tools_json)
                            .unwrap_or_else(|_| Value::Array(vec![])),
                    })))
                })
                .unwrap_or_else(|| err(format!("no live session: {}", name)))
        }
        "stop" => {
            STOP.store(true, Ordering::Relaxed);
            ok(json!({"stopping": true}))
        }
        _ => err(format!("unknown op: {}", op)),
    }
}

fn normalize_url(target: &str) -> String {
    if target.starts_with("http://") || target.starts_with("https://") {
        target.to_string()
    } else {
        format!("https://{}", target)
    }
}
