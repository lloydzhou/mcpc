#!/usr/bin/env python3
import os
import sys

log = os.environ.get("MCPC_REQUEST_LOG", os.path.join(os.path.dirname(__file__), "..", "requests.log"))
length = int(os.environ.get("CONTENT_LENGTH", "0"))
body = sys.stdin.read(length)

method = ""
id_val = ""
if '"method":"' in body:
    method = body.split('"method":"')[1].split('"')[0]
if '"id":' in body:
    rest = body.split('"id":')[1]
    id_val = rest.split(',')[0].split('}')[0]

with open(log, "a") as f:
    f.write(f"REQUEST method={method} sid={os.environ.get('HTTP_MCP_SESSION_ID', '')}\n")

print("Mcp-Session-Id: fixed-session-777")
print("Content-Type: application/json")
print()
if method == "initialize":
    print(f'{{"jsonrpc":"2.0","id":{id_val},"result":{{"protocolVersion":"2025-03-26","capabilities":{{"tools":{{}}}},"serverInfo":{{"name":"web-mcp","version":"1.0"}}}}}}')
elif method == "tools/list":
    print(f'{{"jsonrpc":"2.0","id":{id_val},"result":{{"tools":[{{"name":"web_search","description":"search the web","inputSchema":{{"type":"object"}}}}]}}}}')
elif method == "tools/call":
    print(f'{{"jsonrpc":"2.0","id":{id_val},"result":{{"content":[{{"type":"text","text":"web-result"}}]}}}}')
else:
    print(f'{{"jsonrpc":"2.0","id":{id_val},"error":{{"code":-32601,"message":"nope"}}}}')
