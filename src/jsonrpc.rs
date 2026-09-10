use serde_json::Value;

/// Build a JSON-RPC 2.0 request string.
pub fn request(id: i64, method: &str, params: &str) -> String {
    let params_clean = if params.is_empty() { "{}" } else { params };
    format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":{},\"method\":\"{}\",\"params\":{}}}",
        id, method, params_clean
    )
}

/// Build a JSON-RPC 2.0 notification string (no id).
pub fn notification(method: &str) -> String {
    format!("{{\"jsonrpc\":\"2.0\",\"method\":\"{}\"}}", method)
}

/// Extract the JSON-RPC id from a parsed response, returning the string representation.
fn id_string(value: &Value) -> Option<String> {
    match value.get("id")? {
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

/// Check whether a parsed JSON-RPC response matches the expected numeric id.
/// Accepts both numeric and string ids (if they equal the decimal representation).
pub fn id_matches(value: &Value, want_id: i64) -> bool {
    let version = value.get("jsonrpc");
    if version != Some(&Value::String("2.0".to_string())) {
        return false;
    }
    let want = want_id.to_string();
    id_string(value).map(|s| s == want).unwrap_or(false)
}

/// Return true if the value is a JSON-RPC 2.0 object.
fn is_jsonrpc2(value: &Value) -> bool {
    matches!(value.get("jsonrpc"), Some(Value::String(s)) if s == "2.0")
}

/// Parse a line and, if it is a valid JSON-RPC 2.0 response matching want_id, return the text.
pub fn try_match_line(line: &str, want_id: i64) -> Option<String> {
    let value: Value = serde_json::from_str(line).ok()?;
    if !is_jsonrpc2(&value) {
        return None;
    }
    if id_matches(&value, want_id) {
        Some(line.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request() {
        assert_eq!(
            request(1, "initialize", r#"{"x":1}"#),
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"x":1}}"#
        );
        assert_eq!(
            request(2, "tools/list", ""),
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#
        );
    }

    #[test]
    fn test_id_matches() {
        let v: Value = serde_json::from_str(r#"{"jsonrpc":"2.0","id":5,"result":{}}"#).unwrap();
        assert!(id_matches(&v, 5));
        assert!(!id_matches(&v, 4));

        let v2: Value =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":"5","result":{}}"#).unwrap();
        assert!(id_matches(&v2, 5));
    }

    #[test]
    fn test_try_match_line() {
        assert!(try_match_line(r#"{"jsonrpc":"2.0","id":3,"result":1}"#, 3).is_some());
        assert!(try_match_line(r#"{"jsonrpc":"2.0","id":3,"result":1}"#, 4).is_none());
        assert!(try_match_line("not json", 1).is_none());
    }
}
