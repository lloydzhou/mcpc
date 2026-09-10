#!/usr/bin/env python3
import os
import sys

length = int(os.environ.get("CONTENT_LENGTH", "0"))
body = sys.stdin.read(length)
method = ""
id_val = ""
if '"method":"' in body:
    method = body.split('"method":"')[1].split('"')[0]
if '"id":' in body:
    rest = body.split('"id":')[1]
    id_val = rest.split(',')[0].split('}')[0]

print("Content-Type: text/event-stream")
print()
if method == "initialize":
    print(f'event: message\ndata: {{"jsonrpc":"2.0","id":{id_val},"result":{{"protocolVersion":"2025-03-26","capabilities":{{"tools":{{}}}},"serverInfo":{{"name":"sse-mcp","version":"1.0"}}}}}}\n')
elif method == "tools/list":
    print(f'event: message\ndata: {{"jsonrpc":"2.0","id":{id_val},"result":{{"tools":[{{"name":"sse_tool","description":"over sse","inputSchema":{{"type":"object"}}}}]}}}}\n')
elif method == "tools/call":
    print(f'event: message\ndata: {{"jsonrpc":"2.0","id":{id_val},"result":{{"content":[{{"type":"text","text":"sse-result"}}]}}}}\n')
