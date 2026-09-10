#!/usr/bin/env python3
import os
import sys

length = int(os.environ.get("CONTENT_LENGTH", "0"))
body = sys.stdin.read(length)
method = ""
if '"method":"' in body:
    method = body.split('"method":"')[1].split('"')[0]

if method.startswith("notifications/"):
    print("Status: 500 Internal Server Error")
    print()
else:
    print("Mcp-Session-Id: s500")
    print("Content-Type: application/json")
    print()
    print('{"jsonrpc":"2.0","id":1,"result":{}}')
