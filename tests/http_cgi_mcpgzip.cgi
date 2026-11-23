#!/usr/bin/env python3
"""gzip MCP CGI: gzip-compress the response only when the client asks.

The Accept-Encoding value is embedded in the tool description so an e2e test
can prove the header actually reached the server. CGI headers use the
`Name: value` form consumed by tests/http_server.py.
"""
import gzip
import os
import sys

length = int(os.environ.get("CONTENT_LENGTH", "0"))
body = sys.stdin.read(length)
accept = os.environ.get("HTTP_ACCEPT_ENCODING", "")

method = ""
id_val = ""
if '"method":"' in body:
    method = body.split('"method":"')[1].split('"')[0]
if '"id":' in body:
    rest = body.split('"id":')[1]
    id_val = rest.split(',')[0].split('}')[0]

if method == "initialize":
    payload = (
        f'{{"jsonrpc":"2.0","id":{id_val},"result":'
        f'{{"protocolVersion":"2025-03-26","capabilities":{{"tools":{{}}}},'
        f'"serverInfo":{{"name":"gzip-mcp","version":"1.0"}}}}}}'
    )
elif method == "tools/list":
    payload = (
        f'{{"jsonrpc":"2.0","id":{id_val},"result":{{"tools":['
        f'{{"name":"gz_tool","description":"gz:{accept}","inputSchema":{{"type":"object"}}}}]}}}}'
    )
elif method == "tools/call":
    payload = (
        f'{{"jsonrpc":"2.0","id":{id_val},"result":'
        f'{{"content":[{{"type":"text","text":"gz-text"}}],"isError":false}}}}'
    )
else:
    payload = f'{{"jsonrpc":"2.0","id":{id_val},"error":{{"code":-32601,"message":"nope"}}}}'

raw = (payload + "\n").encode("utf-8")
out = [
    "Mcp-Session-Id:gzip-session-1\n",
    "Content-Type:application/json\n",
]
if "gzip" in accept.lower():
    raw = gzip.compress(raw)
    out.append("Content-Encoding:gzip\n")
out.append(f"X-Body-Length:{len(raw)}\n\n")
sys.stdout.flush()
sys.stdout.buffer.write("".join(out).encode("utf-8"))
sys.stdout.buffer.write(raw)
sys.stdout.buffer.flush()
