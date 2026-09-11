# mcpc

A standalone Model Context Protocol (MCP) client CLI written in Rust.

`mcpc` talks to MCP servers over **Streamable HTTP** or **stdio JSON-RPC** and keeps a small local daemon so that multiple CLI invocations share the same connection/session.

## Install

```bash
cargo install --path .
# or via Homebrew
brew install lloydzhou/tap/mcpc
```

```bash
# or via APT (Debian/Ubuntu; the repository is hosted alongside bash-agent)
curl -fsSL https://lloydzhou.github.io/bash-agent/install.sh | sudo bash
sudo apt-get install mcpc
```

```bash
# or via AUR (Arch Linux; mcpc-bin for the prebuilt binary)
yay -S mcpc
```

## Usage

```bash
# Streamable HTTP server
mcpc connect https://example.com/mcp @myserver

# stdio JSON-RPC server
mcpc connect cmd:"node server.js" @myserver

# list cached/live sessions
mcpc ls

# call a tool
mcpc @myserver tools-call echo msg=hello

# ping a session
mcpc @myserver ping

# close a session
mcpc close @myserver

# stop the daemon
mcpc daemon stop
```

## CLI overview

```
mcpc connect URL [@NAME]
mcpc connect cmd:COMMAND [@NAME]
mcpc ls | close @NAME | daemon stop | grep PATTERN
mcpc @NAME tools-list [--full] | tools-get TOOL | tools-call TOOL [K=V|K:=JSON...] | ping
```

Global flags: `--json`, `--insecure`, `--timeout N`, `-H HEADER`, `--protocol-version VER`.

## Daemon design

- Every `mcpc` command is a short-lived process that talks to a local Unix socket.
- If the daemon is not running it is started implicitly in the background.
- The daemon holds the actual transport connection and mirrors session metadata to `$MCPC_HOME/servers/*.json` so tools can be listed/grepped offline.
- `daemon stop` kills stdio child processes and cleans up the socket/pid files.

## Data directory

`$MCPC_HOME` → `$BA_HOME/mcpc` → `~/.local/share/mcpc`.

## Development

```bash
cargo build
cargo test
bash tests/test.sh          # e2e tests (stdio + HTTP + SSE)
```

## Relationship to bash-agent

This tool implements the MCP client half of the functionality discussed in
[bash-agent#88](https://github.com/lloydzhou/bash-agent/issues/88). It is a
standalone Rust port of the `mcpc` applet from
[lloydzhou/busyagent](https://github.com/lloydzhou/busyagent) PR #4, sharing the
same CLI/JSON contract so it can be composed with `bash-agent` or used on its
own.

## License

MIT
