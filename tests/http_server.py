#!/usr/bin/env python3
"""Minimal Streamable-HTTP MCP test server for mcpc e2e tests."""
import os
import subprocess
import sys
import time
from http.server import BaseHTTPRequestHandler, HTTPServer

WEB = sys.argv[1]
LOG = os.path.join(WEB, "requests.log")
PIDFILE = os.path.join(WEB, "httpd.pid")
os.environ["MCPC_REQUEST_LOG"] = LOG

def daemonize():
    pid = os.fork()
    if pid > 0:
        for _ in range(50):
            if os.path.exists(PIDFILE):
                break
            time.sleep(0.05)
        sys.exit(0)
    os.setsid()
    pid = os.fork()
    if pid > 0:
        sys.exit(0)
    os.chdir("/")
    os.umask(0)
    for fd in range(0, 3):
        try:
            os.close(fd)
        except OSError:
            pass
    sys.stdin = open(os.devnull, "r")
    sys.stdout = open(os.devnull, "w")
    sys.stderr = open(os.devnull, "w")

if len(sys.argv) > 3 and sys.argv[3] == "--daemon":
    daemonize()

class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        if not self.path.startswith("/cgi-bin/"):
            self.send_error(404)
            return
        script = os.path.join(WEB, "cgi-bin", os.path.basename(self.path))
        if not os.path.isfile(script):
            self.send_error(404, "No such CGI script")
            return
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length)
        env = os.environ.copy()
        env["CONTENT_LENGTH"] = str(length)
        env["CONTENT_TYPE"] = self.headers.get("Content-Type", "")
        env["REQUEST_METHOD"] = "POST"
        env["SCRIPT_NAME"] = self.path
        env["PATH_INFO"] = ""
        for name, value in self.headers.items():
            key = "HTTP_" + name.upper().replace("-", "_")
            env[key] = value
        try:
            proc = subprocess.run(
                [sys.executable, script],
                input=body,
                capture_output=True,
                env=env,
                timeout=60,
            )
        except Exception as e:
            self.send_error(500, str(e))
            return
        if proc.returncode != 0:
            self.send_error(500, proc.stderr.decode("utf-8", "replace"))
            return
        out = proc.stdout.decode("utf-8", "replace")
        lines = out.splitlines()
        i = 0
        status = 200
        content_type = "application/json"
        extra_headers = []
        while i < len(lines) and lines[i]:
            low = lines[i].lower()
            if low.startswith("status:"):
                status = int(lines[i].split()[1])
            elif low.startswith("content-type:"):
                content_type = lines[i].split(":", 1)[1].strip()
            elif ":" in lines[i]:
                extra_headers.append(lines[i].split(":", 1))
            i += 1
        body_lines = lines[i + 1:]
        body_out = "\n".join(body_lines) + ("\n" if body_lines else "")
        body_bytes = body_out.encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body_bytes)))
        for k, v in extra_headers:
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(body_bytes)

    def do_GET(self):
        self.send_error(404)

os.makedirs(os.path.join(WEB, "cgi-bin"), exist_ok=True)
open(LOG, "w").close()

# warm up a python subprocess once: the first exec of the interpreter on a
# fresh CI runner can take far longer than a request timeout (gatekeeper /
# first-launch overhead), which would otherwise stall the first CGI call.
try:
    subprocess.run([sys.executable, "-c", "pass"], timeout=120)
except Exception:
    pass

with open(PIDFILE, "w") as f:
    f.write(str(os.getpid()))

port = int(sys.argv[2])
server = HTTPServer(("127.0.0.1", port), Handler)
server.serve_forever()
