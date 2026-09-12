use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;
use rmcp::transport::child_process::TokioChildProcess;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use rmcp::{RoleClient, ServiceExt};
use serde_json::{json, Value};

use crate::util;

#[derive(Clone, Copy, PartialEq)]
pub enum Transport {
    Http,
    Stdio,
}

/// A live MCP client session backed by the official rmcp SDK.
/// `serve()` (initialize handshake) completes inside `handshake()`;
/// all subsequent requests go through the shared tokio runtime.
pub struct Session {
    pub name: String,
    pub transport: Transport,
    /// URL for http sessions, shell command for stdio sessions.
    pub source: String,
    /// Raw "K: V" header strings; persisted to the cache so the daemon can
    /// reconnect with the same Authorization after a restart.
    pub headers: Vec<String>,
    pub server_info_json: String,
    pub tools_json: String,
    pub dead: bool,
    service: Option<RunningService<RoleClient, ()>>,
}

static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

pub fn rt() -> &'static tokio::runtime::Runtime {
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to create tokio runtime")
    })
}

fn parse_headers(
    headers: &[String],
) -> Result<HashMap<http::HeaderName, http::HeaderValue>, String> {
    let mut map = HashMap::new();
    for h in headers {
        let (k, v) = match h.split_once(':') {
            Some(kv) => kv,
            None => return Err(format!("invalid header {:?}, expect \"K: V\"", h)),
        };
        let name = k
            .trim()
            .parse::<http::HeaderName>()
            .map_err(|e| format!("invalid header name {:?}: {}", k.trim(), e))?;
        let value = http::HeaderValue::from_str(v.trim())
            .map_err(|e| format!("invalid header value for {}: {}", name, e))?;
        map.insert(name, value);
    }
    Ok(map)
}

impl Session {
    pub fn new_http(
        name: String,
        url: String,
        headers: Vec<String>,
        _protocol_version: Option<String>,
    ) -> Session {
        Session {
            name,
            transport: Transport::Http,
            source: url,
            headers,
            server_info_json: String::new(),
            tools_json: String::new(),
            dead: false,
            service: None,
        }
    }

    pub fn new_stdio(
        name: String,
        cmd: String,
        _protocol_version: Option<String>,
    ) -> Session {
        Session {
            name,
            transport: Transport::Stdio,
            source: cmd,
            headers: Vec::new(),
            server_info_json: String::new(),
            tools_json: String::new(),
            dead: false,
            service: None,
        }
    }

    /// Run the initialize handshake and fetch the tool list.
    /// On success the session is live; on failure it is unusable.
    pub fn handshake(&mut self) -> Result<(), String> {
        let headers = parse_headers(&self.headers)?;
        let transport_kind = self.transport;
        let source = self.source.clone();

        let service = rt().block_on(async move {
            match transport_kind {
                Transport::Http => {
                    let config =
                        StreamableHttpClientTransportConfig::with_uri(source.clone())
                            .custom_headers(headers);
                    ()
                        .serve(StreamableHttpClientTransport::from_config(config))
                        .await
                        .map_err(|e| format!("initialize failed: {}", e))
                }
                Transport::Stdio => {
                    let mut command = tokio::process::Command::new("sh");
                    command.arg("-c").arg(&source);
                    let transport = TokioChildProcess::new(command)
                        .map_err(|e| format!("spawn failed: {}", e))?;
                    ()
                        .serve(transport)
                        .await
                        .map_err(|e| format!("initialize failed: {}", e))
                }
            }
        })?;

        let tools = rt()
            .block_on(service.list_tools(None))
            .map_err(|e| format!("tools/list failed: {}", e))?;

        if let Some(info) = service.peer_info() {
            self.server_info_json = serde_json::to_string(&*info).unwrap_or_default();
        }
        self.tools_json = serde_json::to_string(&tools.tools).unwrap_or_else(|_| "[]".into());
        self.service = Some(service);
        Ok(())
    }

    /// Call a tool. `params_str` is the raw JSON `{"name":..,"arguments":{..}}`
    /// object. Returns the serialized CallToolResult on success; None marks the
    /// session dead (mirrors the old rpc_call failure semantics).
    pub fn rpc_call(&mut self, _method: &str, params_str: &str) -> Option<String> {
        let params: Value = serde_json::from_str(params_str).unwrap_or(Value::Null);
        let tool = params
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        if tool.is_empty() {
            eprintln!("mcpc: tools/call missing tool name");
            return None;
        }

        let mut request = CallToolRequestParams::new(tool);
        if let Value::Object(map) = params.get("arguments").cloned().unwrap_or(Value::Null) {
            request = request.with_arguments(map);
        }

        let service = match self.service.as_ref() {
            Some(s) => s,
            None => {
                self.dead = true;
                return None;
            }
        };
        match rt().block_on(service.call_tool(request)) {
            Ok(result) => Some(serde_json::to_string(&result).unwrap_or_else(|_| "{}".into())),
            Err(e) => {
                self.dead = true;
                eprintln!("mcpc: tools/call on @{} failed: {}", self.name, e);
                None
            }
        }
    }

    /// Gracefully close the underlying service (closes the transport and,
    /// for stdio, shuts the child process down).
    pub fn shutdown(&mut self) {
        if let Some(mut service) = self.service.take() {
            let _ = rt().block_on(async {
                let _ = service.close_with_timeout(Duration::from_secs(2)).await;
            });
        }
    }

    /// Cache JSON persisted to `servers/{name}.json`. Headers are included so
    /// the daemon can restore authenticated sessions after a restart.
    pub fn cache_json(&self) -> Value {
        json!({
            "name": self.name,
            "transport": match self.transport {
                Transport::Http => "http",
                Transport::Stdio => "stdio",
            },
            "source": self.source,
            "headers": self.headers,
            "server_info": serde_json::from_str::<Value>(&self.server_info_json)
                .unwrap_or(Value::Null),
            "tools": serde_json::from_str::<Value>(&self.tools_json)
                .unwrap_or(Value::Array(vec![])),
        })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub struct SessionStore {
    sessions: Mutex<HashMap<String, Session>>,
}

impl SessionStore {
    pub fn new() -> Self {
        SessionStore {
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub fn insert(&self, session: Session) -> bool {
        let mut map = self.sessions.lock().unwrap();
        if map.contains_key(&session.name) {
            return false;
        }
        map.insert(session.name.clone(), session);
        true
    }

    pub fn remove(&self, name: &str) -> bool {
        let mut map = self.sessions.lock().unwrap();
        map.remove(name).is_some()
    }

    pub fn names(&self) -> Vec<String> {
        let map = self.sessions.lock().unwrap();
        map.keys().cloned().collect()
    }

    pub fn gc_dead(&self) {
        let mut map = self.sessions.lock().unwrap();
        let dead: Vec<String> = map
            .iter()
            .filter(|(_, s)| s.dead)
            .map(|(k, _)| k.clone())
            .collect();
        for name in dead {
            map.remove(&name);
        }
    }

    pub fn with_session<F, T>(&self, name: &str, f: F) -> Option<T>
    where
        F: FnOnce(&mut Session) -> Option<T>,
    {
        let mut map = self.sessions.lock().unwrap();
        map.get_mut(name).and_then(f)
    }
}

pub fn cache_save(name: &str, cache: &Value) {
    let path = match util::server_path(name) {
        Some(p) => p,
        None => return,
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&path, serde_json::to_string_pretty(cache).unwrap_or_default()) {
        eprintln!("mcpc: cannot write cache for @{}: {}", name, e);
    }
}

pub fn cache_load(name: &str) -> Option<Value> {
    let path = util::server_path(name)?;
    let raw = util::read_file(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Reconnect sessions from `servers/*.json` after a daemon restart.
/// Failures are logged and skipped so a single stale server cannot block startup.
pub fn restore_sessions(store: &SessionStore) {
    let dir = util::mcpc_home().join("servers");
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let name = match path.file_stem().and_then(|s| s.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let cache = match cache_load(&name) {
            Some(c) => c,
            None => continue,
        };
        let source = match cache.get("source").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let headers: Vec<String> = cache
            .get("headers")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|h| h.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let mut session = if let Some(cmd) = source.strip_prefix("cmd:") {
            Session::new_stdio(name.clone(), cmd.to_string(), None)
        } else {
            Session::new_http(name.clone(), source.clone(), headers, None)
        };
        if let Err(e) = session.handshake() {
            eprintln!("mcpc: restore @{} skipped: {}", name, e);
            continue;
        }
        if !store.insert(session) {
            eprintln!("mcpc: restore @{} skipped: duplicate name", name);
        }
    }
}
