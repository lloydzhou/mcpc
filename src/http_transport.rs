use crate::jsonrpc;
use crate::sse::SseParser;
use std::io::Read;
use std::time::Duration;

const HTTP_TIMEOUT_MS: u64 = 30_000;

pub struct HttpTransport {
    pub url: String,
    pub headers: Vec<String>,
    pub session_id: Option<String>,
}

impl HttpTransport {
    pub fn new(url: String, headers: Vec<String>) -> Self {
        Self {
            url,
            headers,
            session_id: None,
        }
    }

    /// Send a JSON-RPC request and return the raw response text.
    /// For notifications pass `want_id = -1`.
    pub fn roundtrip(&mut self, request: &str, want_id: i64) -> Option<String> {
        let (status, body) = self.send(request, want_id)?;
        if !(200..300).contains(&status) {
            eprintln!("mcpc: HTTP {}", status);
            return None;
        }
        body
    }

    /// Send a notification.  Any 2xx status counts as success.
    pub fn notify(&mut self, request: &str) -> bool {
        let (status, _body) = match self.send(request, -1) {
            Some(r) => r,
            None => return false,
        };
        (200..300).contains(&status)
    }

    fn send(&mut self, request: &str, want_id: i64) -> Option<(i32, Option<String>)> {
        let req = ureq::post(&self.url);
        let req = req
            .set("Content-Type", "application/json")
            .set("Accept", "application/json, text/event-stream")
            .timeout(Duration::from_millis(HTTP_TIMEOUT_MS));
        let mut req = req;
        for h in &self.headers {
            let mut parts = h.splitn(2, ':');
            let name = parts.next()?;
            let value = parts.next().unwrap_or("").trim_start();
            req = req.set(name, value);
        }
        if let Some(sid) = &self.session_id {
            req = req.set("Mcp-Session-Id", sid);
        }

        let resp = req.send_string(request).ok()?;
        let status = resp.status() as i32;
        let content_type = resp
            .header("Content-Type")
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();
        let new_session_id = resp.header("Mcp-Session-Id").map(|s| s.to_string());

        let is_sse = content_type.starts_with("text/event-stream");
        let mut body = String::new();
        let mut result: Option<String> = None;

        if is_sse && want_id >= 0 {
            let mut sse = SseParser::new();
            let mut reader = resp.into_reader();
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        sse.feed(&buf[..n]);
                        if let Some((event, data)) = sse.take_event() {
                            if event == "message" {
                                if let Some(parsed) = Self::parse_sse_data(&data, want_id) {
                                    result = Some(parsed);
                                    break;
                                }
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            if result.is_none() {
                sse.finish();
                if let Some((event, data)) = sse.take_event() {
                    if event == "message" {
                        result = Self::parse_sse_data(&data, want_id);
                    }
                }
            }
        } else {
            let mut reader = resp.into_reader();
            let _ = reader.read_to_string(&mut body);
            body = body.trim_end().to_string();
            if want_id >= 0 && !body.is_empty() {
                if let Some(parsed) = Self::parse_sse_data(&body, want_id) {
                    result = Some(parsed);
                }
            }
        }

        if let Some(sid) = new_session_id {
            self.session_id = Some(sid);
        }

        if want_id < 0 {
            // notification: body content is irrelevant
            Some((status, None))
        } else {
            Some((status, result))
        }
    }

    fn parse_sse_data(data: &str, want_id: i64) -> Option<String> {
        let parsed: serde_json::Value = serde_json::from_str(data).ok()?;
        if jsonrpc::id_matches(&parsed, want_id) {
            Some(data.to_string())
        } else {
            None
        }
    }
}
