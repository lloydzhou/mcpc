mod daemon;
mod http_transport;
mod jsonrpc;
mod rpc;
mod session;
mod sse;
mod stdio_transport;
mod util;

use serde_json::Value;
use std::env;
use std::process;

const USAGE: &str = "Usage: mcpc connect URL|cmd:CMD [@SESSION] [-H HDR] [--protocol-version V]
       mcpc ls | close @SESSION | daemon stop | grep PATTERN
       mcpc @SESSION COMMAND [ARGS]

Session commands: tools-list [--full], tools-get TOOL, tools-call TOOL [K=V|K:=JSON...], ping

Global flags: --json  --insecure  --timeout N";

fn usage_err(msg: &str) -> ! {
    eprintln!("mcpc: {}", msg);
    eprintln!("{}", USAGE);
    process::exit(1);
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let (argv, json_out, _insecure, _timeout) = strip_global_flags(&args);

    if argv.len() < 2 {
        usage_err("missing command");
    }

    let cmd = argv[1].clone();
    match cmd.as_str() {
        "daemon" => handle_daemon(&argv),
        "connect" => handle_connect(&argv, json_out),
        "ls" => handle_ls(json_out),
        "close" => handle_close(&argv),
        "grep" => handle_grep(&argv),
        _ if cmd.starts_with('@') => handle_session(&cmd, &argv[2..], json_out),
        _ => usage_err(&format!("unknown command '{}'", cmd)),
    }
}

fn strip_global_flags(args: &[String]) -> (Vec<String>, bool, bool, Option<u64>) {
    let mut out = Vec::with_capacity(args.len());
    out.push(args[0].clone());
    let mut i = 1;
    let mut json_out = false;
    let mut insecure = false;
    let mut timeout: Option<u64> = None;
    while i < args.len() {
        let a = &args[i];
        if a == "--json" {
            json_out = true;
        } else if a == "--insecure" {
            insecure = true;
        } else if a == "--timeout" {
            if i + 1 < args.len() {
                timeout = args[i + 1].parse().ok();
                i += 1;
            }
        } else if let Some(v) = a.strip_prefix("--timeout=") {
            timeout = v.parse().ok();
        } else {
            out.push(a.clone());
        }
        i += 1;
    }
    (out, json_out, insecure, timeout)
}

fn handle_daemon(argv: &[String]) {
    if argv.len() < 3 {
        usage_err("usage: mcpc daemon run|stop");
    }
    match argv[2].as_str() {
        "run" => {
            let store = std::sync::Arc::new(session::SessionStore::new());
            if let Err(e) = daemon::run(store) {
                eprintln!("mcpc: daemon failed: {}", e);
                process::exit(1);
            }
        }
        "stop" => match rpc::stop_daemon() {
            Ok(()) => println!("daemon stopped"),
            Err(e) => {
                eprintln!("mcpc: stop failed: {}", e);
                process::exit(1);
            }
        },
        _ => usage_err("usage: mcpc daemon run|stop"),
    }
}

fn handle_connect(argv: &[String], _json_out: bool) {
    if argv.len() < 3 {
        usage_err("usage: mcpc connect URL|cmd:CMD [@NAME] [-H HEADER] [--protocol-version VER]");
    }
    let target = &argv[2];
    let name = argv
        .get(3)
        .map(|s| util::bare_name(s))
        .filter(|s| !s.is_empty())
        .unwrap_or("default");

    let mut header: Option<String> = None;
    let mut pver: Option<String> = None;
    let mut k = 3;
    while k < argv.len() {
        let a = &argv[k];
        if let Some(v) = a.strip_prefix("--header=") {
            header = Some(v.to_string());
        } else if a == "-H" && k + 1 < argv.len() {
            header = Some(argv[k + 1].clone());
            k += 1;
        } else if let Some(v) = a.strip_prefix("--protocol-version=") {
            pver = Some(v.to_string());
        } else if a == "--protocol-version" && k + 1 < argv.len() {
            pver = Some(argv[k + 1].clone());
            k += 1;
        }
        k += 1;
    }

    let mut req = serde_json::Map::new();
    req.insert("op".to_string(), Value::String("connect".to_string()));
    req.insert("name".to_string(), Value::String(name.to_string()));
    req.insert("target".to_string(), Value::String(target.clone()));
    if let Some(h) = header {
        req.insert("header".to_string(), Value::String(h));
    }
    if let Some(v) = pver {
        req.insert("protocol_version".to_string(), Value::String(v));
    }

    let resp = match rpc::rpc(&Value::Object(req).to_string()) {
        Some(r) => r,
        None => {
            eprintln!("mcpc: connect @{} failed: cannot reach daemon", name);
            process::exit(1);
        }
    };

    let parsed: Value = match serde_json::from_str(&resp) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("mcpc: daemon: {}", resp);
            process::exit(1);
        }
    };

    if !parsed.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        let err = parsed
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("daemon error");
        eprintln!("mcpc: connect @{} failed: {}", name, err);
        process::exit(1);
    }

    // Mirror the session cache locally.
    if let Some(result) = parsed.get("result") {
        let transport = if target.starts_with("cmd:") { "stdio" } else { "http" };
        let mut cache = result.clone();
        cache["transport"] = Value::String(transport.to_string());
        cache["source"] = Value::String(target.clone());
        session::cache_save(name, &cache);
    }
    println!("connected @{}", name);
}

fn handle_ls(json_out: bool) {
    let resp = match rpc::rpc(r#"{"op":"list"}"#) {
        Some(r) => r,
        None => {
            println!("no sessions (mcpc connect URL @name)");
            process::exit(0);
        }
    };
    let parsed: Value = match serde_json::from_str(&resp) {
        Ok(v) => v,
        Err(_) => {
            println!("no sessions (mcpc connect URL @name)");
            process::exit(0);
        }
    };
    if !parsed.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        println!("no sessions (mcpc connect URL @name)");
        process::exit(0);
    }
    let arr = parsed.get("result").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    for s in arr {
        if json_out {
            println!("{}", s);
        } else {
            let name = s.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            let tr = s.get("transport").and_then(|v| v.as_str()).unwrap_or("?");
            let pid = s.get("pid").and_then(|v| v.as_i64()).unwrap_or(0);
            println!("@{:-14} {:<6} live (daemon pid {})", name, tr, pid);
        }
    }
}

fn handle_close(argv: &[String]) {
    if argv.len() != 3 {
        usage_err("usage: mcpc close @NAME");
    }
    let name = util::bare_name(&argv[2]);
    let req = serde_json::json!({"op":"close","name":name}).to_string();
    let resp = match rpc::rpc(&req) {
        Some(r) => r,
        None => {
            eprintln!("mcpc: close failed: cannot reach daemon");
            process::exit(1);
        }
    };
    let parsed: Value = match serde_json::from_str(&resp) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("mcpc: close failed: {}", resp);
            process::exit(1);
        }
    };
    if !parsed.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        let err = parsed
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("daemon error");
        eprintln!("mcpc: close failed: {}", err);
        process::exit(1);
    }
    println!("closed @{}", name);
}

fn handle_grep(argv: &[String]) {
    if argv.len() != 3 {
        usage_err("usage: mcpc grep PATTERN");
    }
    let pattern = &argv[2];
    let home = util::mcpc_home();
    let servers_dir = home.join("servers");
    let mut hits = 0;
    let entries = match std::fs::read_dir(&servers_dir) {
        Ok(it) => it,
        Err(_) => {
            println!("no cached servers (mcpc connect URL @name)");
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            continue;
        }
        let data = match session::cache_load(&name) {
            Some(v) => v,
            None => continue,
        };
        let tools = data.get("tools").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        for t in tools {
            let nm = t.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            let desc = t.get("description").and_then(|v| v.as_str()).unwrap_or("");
            if nm.contains(pattern) || desc.contains(pattern) {
                println!("{:-10} {}", name, nm);
                hits += 1;
            }
        }
    }
    if hits == 0 {
        println!("no tools match '{}'", pattern);
    }
}

fn handle_session(name_arg: &str, sargs: &[String], json_out: bool) {
    let name = util::bare_name(name_arg);
    if sargs.is_empty() {
        usage_err("no session command (tools-list|tools-get|tools-call|ping)");
    }
    let cmd = &sargs[0];
    match cmd.as_str() {
        "ping" => handle_ping(name),
        "tools-list" => handle_tools_list(name, &sargs[1..], json_out),
        "tools-get" => handle_tools_get(name, &sargs[1..]),
        "tools-call" => handle_tools_call(name, &sargs[1..]),
        _ => usage_err(&format!("unknown session command '{}'", cmd)),
    }
}

fn handle_ping(name: &str) {
    let req = serde_json::json!({"op":"ping","name":name}).to_string();
    let resp = match rpc::rpc(&req) {
        Some(r) => r,
        None => {
            eprintln!("mcpc: @{}: ping failed", name);
            process::exit(1);
        }
    };
    let parsed: Value = match serde_json::from_str(&resp) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("mcpc: @{}: ping failed", name);
            process::exit(1);
        }
    };
    if !parsed.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        eprintln!(
            "mcpc: @{}: {}",
            name,
            parsed.get("error").and_then(|v| v.as_str()).unwrap_or("ping failed")
        );
        process::exit(1);
    }
    println!("pong @{}", name);
}

fn handle_tools_list(name: &str, sargs: &[String], json_out: bool) {
    let full = sargs.iter().any(|a| a == "--full");
    // Local cache first.
    if let Some(cache) = session::cache_load(name) {
        print_tools(&cache, full, json_out);
        return;
    }
    // Fallback to daemon.
    let req = serde_json::json!({"op":"tools","name":name}).to_string();
    let resp = match rpc::rpc(&req) {
        Some(r) => r,
        None => {
            eprintln!("mcpc: @{}: cannot reach daemon", name);
            process::exit(1);
        }
    };
    let parsed: Value = match serde_json::from_str(&resp) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("mcpc: @{}: bad daemon response", name);
            process::exit(1);
        }
    };
    if !parsed.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        eprintln!(
            "mcpc: @{}: {}",
            name,
            parsed.get("error").and_then(|v| v.as_str()).unwrap_or("daemon error")
        );
        process::exit(1);
    }
    if let Some(result) = parsed.get("result") {
        print_tools(result, full, json_out);
    }
}

fn print_tools(wrapper: &Value, full: bool, json_out: bool) {
    let tools = wrapper
        .get("tools")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if json_out {
        println!("{}", Value::Array(tools));
        return;
    }
    for t in tools {
        if full {
            println!("{}", t);
        } else {
            let name = t.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            let desc = t.get("description").and_then(|v| v.as_str()).unwrap_or("");
            let first = desc.lines().next().unwrap_or("");
            println!("{:-24} {}", name, first);
        }
    }
}

fn handle_tools_get(name: &str, sargs: &[String]) {
    if sargs.is_empty() {
        usage_err(&format!("usage: mcpc @{} tools-get TOOL", name));
    }
    let want = &sargs[0];
    let cache = session::cache_load(name)
        .unwrap_or_else(|| usage_err(&format!("no cached tools for @{} (connect first)", name)));
    let tools = cache
        .get("tools")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    for t in tools {
        let nm = t.get("name").and_then(|v| v.as_str()).unwrap_or("");
        if nm == want {
            println!("{}", t);
            return;
        }
    }
    usage_err(&format!("tool '{}' not found in @{}", want, name));
}

fn handle_tools_call(name: &str, sargs: &[String]) {
    if sargs.is_empty() {
        usage_err(&format!(
            "usage: mcpc @{} tools-call TOOL [K=V|K:=JSON...]",
            name
        ));
    }
    let tool = &sargs[0];
    let mut args = serde_json::Map::new();
    for a in sargs[1..].iter() {
        let eq_pos = match a.find('=') {
            Some(p) if p > 0 => p,
            _ => usage_err(&format!("bad argument '{}' (want K=V or K:=JSON)", a)),
        };
        let mut key = a[..eq_pos].to_string();
        let raw = key.ends_with(':');
        if raw {
            key.pop();
        }
        let value_str = &a[eq_pos + 1..];
        let value: Value = if raw && !value_str.is_empty() {
            serde_json::from_str(value_str).unwrap_or_else(|_| Value::String(value_str.to_string()))
        } else {
            Value::String(value_str.to_string())
        };
        args.insert(key, value);
    }

    let params_str = format!(
        r#"{{"name":{},"arguments":{}}}"#,
        serde_json::to_string(tool).unwrap(),
        Value::Object(args)
    );
    let req = format!(
        r#"{{"op":"call","name":{},"method":"tools/call","params":{}}}"#,
        serde_json::to_string(name).unwrap(),
        params_str
    );

    let resp = match rpc::rpc(&req) {
        Some(r) => r,
        None => {
            eprintln!("mcpc: tools-call failed: cannot reach daemon");
            process::exit(1);
        }
    };
    let parsed: Value = match serde_json::from_str(&resp) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("mcpc: tools-call failed: {}", resp);
            process::exit(1);
        }
    };
    #[derive(serde::Deserialize)]
    struct DaemonResp<'a> {
        ok: bool,
        #[serde(borrow)]
        result: Option<&'a serde_json::value::RawValue>,
    }
    #[derive(serde::Deserialize)]
    struct RpcResp<'a> {
        #[serde(borrow)]
        result: Option<&'a serde_json::value::RawValue>,
        #[serde(borrow)]
        error: Option<&'a serde_json::value::RawValue>,
    }

    let dresp: DaemonResp = match serde_json::from_str(&resp) {
        Ok(d) => d,
        Err(_) => {
            eprintln!("mcpc: tools-call failed: {}", resp);
            process::exit(1);
        }
    };
    if !dresp.ok {
        // Daemon-level errors go to stdout so callers can capture them consistently.
        println!("mcpc: tools-call failed: {}", parsed.get("error").and_then(|v| v.as_str()).unwrap_or("daemon error"));
        process::exit(1);
    }
    let raw_body = match dresp.result {
        Some(r) => r.get(),
        None => {
            println!("{}", resp);
            return;
        }
    };
    let rpc: RpcResp = match serde_json::from_str(raw_body) {
        Ok(r) => r,
        Err(_) => {
            println!("{}", raw_body);
            return;
        }
    };
    if let Some(err) = rpc.error {
        // JSON-RPC errors are emitted as the raw error object on stdout.
        println!("{}", err.get());
        process::exit(1);
    }
    if let Some(res) = rpc.result {
        println!("{}", res.get());
    } else {
        println!("{}", raw_body);
    }
}
