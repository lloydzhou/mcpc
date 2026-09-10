use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// Resolve the mcpc home directory.
/// Priority: `$MCPC_HOME` > `$BA_HOME/mcpc` > `$HOME/.local/share/mcpc`.
pub fn mcpc_home() -> PathBuf {
    if let Some(h) = env::var_os("MCPC_HOME") {
        return PathBuf::from(h);
    }
    if let Some(h) = env::var_os("BA_HOME") {
        let mut p = PathBuf::from(h);
        p.push("mcpc");
        return p;
    }
    if let Some(h) = env::var_os("HOME") {
        let mut p = PathBuf::from(h);
        p.push(".local/share/mcpc");
        return p;
    }
    PathBuf::from("/tmp/mcpc")
}

pub fn ensure_dir<P: AsRef<Path>>(path: P) -> io::Result<()> {
    let path = path.as_ref();
    if !path.exists() {
        fs::create_dir_all(path)?;
    }
    Ok(())
}

pub fn read_file<P: AsRef<Path>>(path: P) -> io::Result<String> {
    let mut f = fs::File::open(path)?;
    let mut s = String::new();
    f.read_to_string(&mut s)?;
    Ok(s)
}

pub fn write_file<P: AsRef<Path>>(path: P, content: &str) -> io::Result<()> {
    if let Some(parent) = path.as_ref().parent() {
        ensure_dir(parent)?;
    }
    let mut f = fs::File::create(path)?;
    f.write_all(content.as_bytes())?;
    f.write_all(b"\n")?;
    f.flush()?;
    Ok(())
}

pub fn socket_path() -> PathBuf {
    let mut p = mcpc_home();
    p.push("daemon.sock");
    p
}

pub fn pid_path() -> PathBuf {
    let mut p = mcpc_home();
    p.push("daemon.pid");
    p
}

pub fn lock_path() -> PathBuf {
    let mut p = mcpc_home();
    p.push("daemon.lock");
    p
}

pub fn server_path(name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains('/') || name.contains('\\') || name == "." || name == ".." {
        return None;
    }
    let mut p = mcpc_home();
    p.push("servers");
    p.push(format!("{}.json", name));
    Some(p)
}

/// Check whether a process with this pid currently exists.
/// Mirrors the busybox C logic: kill(pid, 0) success OR errno == EPERM counts as alive.
pub fn process_exists(pid: u32) -> bool {
    unsafe {
        let rc = libc::kill(pid as libc::pid_t, 0);
        if rc == 0 {
            return true;
        }
        let err = io::Error::last_os_error().raw_os_error().unwrap_or(0);
        err == libc::EPERM
    }
}

/// Strip a leading '@' from a session name argument.
pub fn bare_name(name: &str) -> &str {
    name.strip_prefix('@').unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_path_filters() {
        assert!(server_path("echo").is_some());
        assert!(server_path("a/b").is_none());
        assert!(server_path(".").is_none());
        assert!(server_path("..").is_none());
    }
}
