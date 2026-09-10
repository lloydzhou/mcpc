use crate::http_transport::HttpTransport;
use crate::jsonrpc;
use crate::stdio_transport::StdioTransport;
use crate::util;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Transport {
    Http,
    Stdio,
}

pub struct Session {
    pub name: String,
    pub transport: Transport,
    pub source: String,
    pub http: Option<HttpTransport>,
    pub stdio: Option<StdioTransport>,
    pub server_info_json: String,
    pub tools_json: String,
    pub protocol_version: Option<String>,
    pub next_id: i64,
    pub dead: bool,
}

impl Session {
    pub fn new_http(
        name: String,
        url: String,
        headers: Vec<String>,
        protocol_version: Option<String>,
    ) -> Self {
        Self {
            name,
            transport: Transport::Http,
            source: url.clone(),
            http: Some(HttpTransport::new(url, headers)),
            stdio: None,
            server_info_json: "{}".to_string(),
            tools_json: "[]".to_string(),
            protocol_version,
            next_id: 1,
            dead: false,
        }
    }

    pub fn new_stdio(
        name: String,
        cmd: String,
        protocol_version: Option<String>,
    ) -> std::io::Result<Self> {
        let stdio = StdioTransport::spawn(&cmd)?;
        Ok(Self {
            name,
            transport: Transport::Stdio,
            source: cmd,
            http: None,
            stdio: Some(stdio),
            server_info_json: "{}".to_string(),
            tools_json: "[]".to_string(),
            protocol_version,
            next_id: 1,
            dead: false,
        })
    }

    fn next_id(&mut self) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn rpc_call(&mut self, method: &str, params: &str) -> Option<String> {
        if self.dead {
            return None;
        }
        let id = self.next_id();
        let req = jsonrpc::request(id, method, params);
        let resp = match self.transport {
            Transport::Http => self.http.as_mut()?.roundtrip(&req, id),
            Transport::Stdio => self.stdio.as_ref()?.roundtrip(&req, id),
        };
        if resp.is_none() {
            self.dead = true;
        }
        resp
    }

    pub fn notify(&mut self, method: &str) -> bool {
        let req = jsonrpc::notification(method);
        let ok = match self.transport {
            Transport::Http => self.http.as_mut().map(|h| h.notify(&req)).unwrap_or(false),
            Transport::Stdio => self.stdio.as_ref().map(|s| s.notify(&req)).unwrap_or(false),
        };
        if !ok {
            self.dead = true;
        }
        ok
    }

    pub fn handshake(&mut self) -> Result<(), String> {
        let ver = self
            .protocol_version
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "2025-03-26".to_string());
        let params = json!({
            "protocolVersion": ver,
            "capabilities": {},
            "clientInfo": {"name":"mcpc","version":"0.1.0"}
        })
        .to_string();
        let resp = self
            .rpc_call("initialize", &params)
            .ok_or_else(|| "initialize request failed".to_string())?;
        let parsed: Value = serde_json::from_str(&resp)
            .map_err(|e| format!("bad initialize response: {}", e))?;
        let result = parsed
            .get("result")
            .ok_or_else(|| "initialize response missing result".to_string())?;
        self.server_info_json = result
            .get("serverInfo")
            .map(|v| v.to_string())
            .unwrap_or_else(|| "{}".to_string());
        if !self.notify("notifications/initialized") {
            return Err("notifications/initialized failed".to_string());
        }
        let tools_resp = self
            .rpc_call("tools/list", "{}")
            .ok_or_else(|| "tools/list failed".to_string())?;
        let tools_parsed: Value = serde_json::from_str(&tools_resp)
            .map_err(|e| format!("bad tools/list response: {}", e))?;
        let tools = tools_parsed
            .get("result")
            .and_then(|r| r.get("tools"))
            .cloned()
            .unwrap_or_else(|| Value::Array(vec![]));
        self.tools_json = tools.to_string();
        Ok(())
    }

    pub fn cache_json(&self) -> Value {
        json!({
            "transport": match self.transport {
                Transport::Http => "http",
                Transport::Stdio => "stdio",
            },
            "source": self.source,
            "server_info": serde_json::from_str::<Value>(&self.server_info_json)
                .unwrap_or_else(|_| Value::Object(Default::default())),
            "tools": serde_json::from_str::<Value>(&self.tools_json)
                .unwrap_or_else(|_| Value::Array(vec![])),
        })
    }
}

pub struct SessionStore {
    sessions: Mutex<HashMap<String, Session>>,
}

impl SessionStore {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub fn insert(&self, session: Session) {
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.insert(session.name.clone(), session);
        }
    }

    pub fn remove(&self, name: &str) -> bool {
        let mut sessions = match self.sessions.lock() {
            Ok(g) => g,
            Err(_) => return false,
        };
        sessions.remove(name).is_some()
    }

    pub fn names(&self) -> Vec<String> {
        let sessions = match self.sessions.lock() {
            Ok(g) => g,
            Err(_) => return vec![],
        };
        sessions.keys().cloned().collect()
    }

    pub fn gc_dead(&self) {
        let mut sessions = match self.sessions.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let dead_names: Vec<String> = sessions
            .iter()
            .filter(|(_, s)| s.dead)
            .map(|(n, _)| n.clone())
            .collect();
        for name in dead_names {
            if let Some(s) = sessions.remove(&name) {
                drop(s);
            }
        }
    }

    pub fn with_session<F, T>(&self, name: &str, f: F) -> Option<T>
    where
        F: FnOnce(&mut Session) -> Option<T>,
    {
        let mut sessions = self.sessions.lock().ok()?;
        let session = sessions.get_mut(name)?;
        if session.dead {
            return None;
        }
        f(session)
    }

}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Save session cache to disk.
pub fn cache_save(name: &str, json: &Value) {
    if let Some(path) = util::server_path(name) {
        let _ = util::write_file(path, &json.to_string());
    }
}

/// Load session cache from disk.
pub fn cache_load(name: &str) -> Option<Value> {
    let path = util::server_path(name)?;
    let text = util::read_file(path).ok()?;
    serde_json::from_str(&text).ok()
}
