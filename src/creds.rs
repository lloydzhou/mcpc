//! Owner-only credential store, modelled on `~/.dsh/.credentials.yaml`.
//!
//! `servers/*.json` cache files stay shareable: sensitive header values are
//! replaced by a reference and only the real value lives here, in a `0600`
//! document the store refuses to read when anyone but its owner can.

use std::collections::HashMap;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;

use crate::util;

pub const REF_REDACTED: &str = "<stored>";

fn is_sensitive(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "authorization"
        || n == "cookie"
        || n == "proxy-authorization"
        || n.contains("token")
        || n.contains("api-key")
        || n.contains("apikey")
        || n.contains("secret")
}

fn split_header(header: &str) -> Option<(&str, &str)> {
    let (k, v) = header.split_once(':')?;
    Some((k.trim(), v.trim()))
}

fn cred_path() -> PathBuf {
    util::mcpc_home().join("credentials")
}

fn record_path(name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains('/') || name.contains('\\') || name == "." || name == ".." {
        return None;
    }
    Some(cred_path().join(format!("{}.json", name)))
}

/// Read one record; refuse a file any other user can read.
fn read_record(path: &PathBuf) -> Result<Option<HashMap<String, String>>, String> {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot stat {}: {}", path.display(), e)),
    };
    let mode = meta.permissions().mode() & 0o077;
    if mode != 0 {
        return Err(format!(
            "{} is readable beyond its owner (mode {:o}); run \"chmod 600 {}\" before starting again",
            path.display(),
            meta.permissions().mode() & 0o777,
            path.display()
        ));
    }
    let raw = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
    serde_json::from_str::<HashMap<String, String>>(&raw)
        .map(Some)
        .map_err(|e| format!("cannot parse {}: {}", path.display(), e))
}

/// Split the header list into a cache-safe copy and the sensitive values that
/// must never be written to `servers/*.json`.
pub fn redact(headers: &[String]) -> (Vec<String>, Vec<(String, String)>) {
    let mut out = Vec::with_capacity(headers.len());
    let mut secrets = Vec::new();
    for h in headers {
        match split_header(h) {
            Some((k, v)) if is_sensitive(k) && !v.is_empty() => {
                out.push(format!("{}: {} {}", k, REF_REDACTED, k));
                secrets.push((k.to_string(), v.to_string()));
            }
            _ => out.push(h.clone()),
        }
    }
    (out, secrets)
}

/// Restore header values that the cache holds as references. Returns the
/// headers and, when a record was refused or is missing, why — the daemon has
/// no terminal, so the caller must surface it.
pub fn resolve(name: &str, headers: Vec<String>) -> (Vec<String>, Option<String>) {
    if !headers.iter().any(|h| h.contains(REF_REDACTED)) {
        return (headers, None);
    }
    let (stored, problem) = match record_path(name).map(|p| read_record(&p)) {
        Some(Err(e)) => (HashMap::new(), Some(e)),
        Some(Ok(Some(m))) => (m, None),
        _ => (HashMap::new(), None),
    };
    let mut missing = Vec::new();
    let out = headers
        .into_iter()
        .map(|h| match split_header(&h) {
            Some((k, v)) if v == format!("{} {}", REF_REDACTED, k) => match stored.get(k) {
                Some(real) => format!("{}: {}", k, real),
                None => {
                    missing.push(k.to_string());
                    h
                }
            },
            _ => h,
        })
        .collect();
    let problem = problem.or_else(|| {
        (!missing.is_empty()).then(|| {
            format!(
                "no stored credentials for {}; send it again with -H",
                missing.join(", ")
            )
        })
    });
    (out, problem)
}

/// Persist the sensitive part of `headers`; the cache only keeps references.
pub fn store(name: &str, headers: &[String]) {
    let (_, secrets) = redact(headers);
    let path = match record_path(name) {
        Some(p) => p,
        None => return,
    };
    if secrets.is_empty() {
        let _ = std::fs::remove_file(&path);
        return;
    }
    if let Some(dir) = cred_path().parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::create_dir_all(cred_path());
    let _ = std::fs::set_permissions(cred_path(), std::fs::Permissions::from_mode(0o700));
    // Merge so headers given on a later connect do not drop the others.
    let mut map = match read_record(&path) {
        Ok(Some(m)) => m,
        Ok(None) => HashMap::new(),
        Err(e) => {
            eprintln!("mcpc: {}; not storing credentials", e);
            return;
        }
    };
    map.extend(secrets);
    let body = serde_json::to_string_pretty(&map).unwrap_or_default();
    let created = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, body.as_bytes()));
    if let Err(e) = created {
        eprintln!("mcpc: cannot write {}: {}", path.display(), e);
        return;
    }
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
}

/// Remove one session's stored credentials.
pub fn forget(name: &str) {
    if let Some(p) = record_path(name) {
        let _ = std::fs::remove_file(p);
    }
}
