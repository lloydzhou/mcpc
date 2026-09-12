#!/bin/sh
set -eu

cd "$(dirname "$0")/.."
MCPC="${MCPC:-./target/debug/mcpc}"
export BA_HOME="$PWD/tests/mc.home"

cleanup() {
    "$MCPC" daemon stop >/dev/null 2>&1 || true
    rm -rf "$BA_HOME" tests/mcpc-stdio.log tests/*.tmp.sh tests/mc.e2e
}
trap cleanup EXIT

cleanup
mkdir -p "$BA_HOME"

fail=0
assert_eq() {
    name="$1"
    got="$2"
    want="$3"
    if [ "$got" != "$want" ]; then
        echo "FAIL: $name"
        echo "  want: $want"
        echo "  got:  $got"
        fail=$((fail + 1))
    else
        echo "PASS: $name"
    fi
}

# --- stdio server ---------------------------------------------------------
cat > tests/echo-mcp.tmp.sh <<'ECHO'
#!/bin/sh
while IFS= read -r line; do
	method=$(printf '%s' "$line" | grep -o '"method":"[^"]*"' | head -1 | cut -d'"' -f4)
	id=$(printf '%s' "$line" | grep -o '"id":[0-9]*' | head -1 | cut -d: -f2)
	printf '%s\n' "$line" >> "tests/mcpc-stdio.log"
	case "$method" in
	initialize)
		printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"echo-mcp","version":"1.0"}}}\n' "$id"
		;;
	notifications/*)
		:
		;;
	tools/list)
		printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","description":"echo back its arguments","inputSchema":{"type":"object","properties":{"msg":{"type":"string"}}}},{"name":"add","description":"add two numbers","inputSchema":{"type":"object"}}]}}\n' "$id"
		;;
	tools/call)
		case "$line" in
		*'"name":"nope"'*)
			printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32601,"message":"tool not found"}}\n' "$id"
			;;
		*)
			printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"echo-called"}],"isError":false}}\n' "$id"
			;;
		esac
		;;
	ping)
		printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
		;;
	*)
		printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32601,"message":"unknown method %s"}}\n' "$id" "$method"
		;;
	esac
done
ECHO
chmod +x tests/echo-mcp.tmp.sh

assert_eq "usage" "$($MCPC 2>&1 | grep -c 'Usage: mcpc')" "1"
assert_eq "connect" "$($MCPC connect cmd:"$PWD"/tests/echo-mcp.tmp.sh @echo 2>&1)" "connected @echo"
assert_eq "ls" "$($MCPC ls 2>&1 | grep -c '@echo.*stdio.*live')" "1"
assert_eq "tools-list" "$($MCPC @echo tools-list 2>&1 | tr -s ' ')" "echo echo back its arguments
add add two numbers"
assert_eq "tools-get" "$($MCPC @echo tools-get echo 2>&1 | grep -c '"name":"echo"')" "1"
assert_eq "tools-call" "$($MCPC @echo tools-call echo msg:=42 2>&1)" '{"content":[{"type":"text","text":"echo-called"}],"isError":false}'
assert_eq "tools-call-args" "$(grep -c '"method":"tools/call".*"arguments":{"msg":42}' tests/mcpc-stdio.log)" "1"
assert_eq "ping" "$($MCPC @echo ping 2>&1)" "pong @echo"
rc=0; $MCPC @nosuch ping >/dev/null 2>&1 || rc=$?
assert_eq "ping-missing" "$rc" "1"

# protocol version flag
BA_HOME2="$PWD/tests/mc2.home"
assert_eq "protocol-version" "$(BA_HOME=$BA_HOME2 $MCPC connect cmd:"$PWD"/tests/echo-mcp.tmp.sh @old --protocol-version 2024-11-05 2>&1; grep -c '"protocolVersion":"2024-11-05"' tests/mcpc-stdio.log)" "connected @old
1"
rm -rf "$BA_HOME2"

# grep
assert_eq "grep" "$($MCPC grep numbers 2>&1 | tr -s ' ')" "echo add"

# JSON-RPC error surface
# (the `|| rc=$?` guard keeps `set -e` shells from aborting on the non-zero
# exit inside the command substitution)
es_rc=0
es_out=$($MCPC @echo tools-call nope 2>&1) || es_rc=$?
assert_eq "error-surface" "$es_out
rc=$es_rc" '{"code":-32601,"message":"tool not found"}
rc=1'

# big tools/list
awk 'BEGIN{for(i=0;i<20000;i++)printf "x"}' > tests/big-desc.txt
desc=$(cat tests/big-desc.txt)
python3 - "$desc" > tests/echo-big.tmp.sh <<'PY'
import sys
desc = sys.argv[1]
print(r'''#!/bin/sh
while IFS= read -r line; do
	method=$(printf '%s' "$line" | grep -o '"method":"[^"]*"' | head -1 | cut -d'"' -f4)
	id=$(printf '%s' "$line" | grep -o '"id":[0-9]*' | head -1 | cut -d: -f2)
	case "$method" in
	initialize)
		printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"big","version":"1.0"}}}\n' "$id"
		;;
	notifications/*)
		:
		;;
	tools/list)
		printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"t1","description":"%s"}]}}\n' "$id" ''' + repr(desc) + r'''
		;;
	*)
		printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
		;;
	esac
done
''')
PY
chmod +x tests/echo-big.tmp.sh
assert_eq "big-list" "$($MCPC connect cmd:"$PWD"/tests/echo-big.tmp.sh @big 2>&1 && $MCPC @big tools-list 2>&1 | wc -c | tr -d ' ')" "connected @big
20026"

# cache delete fallback
rm -f "$BA_HOME/mcpc/servers/big.json"
assert_eq "fallback" "$($MCPC @big tools-list 2>&1 | wc -c | tr -d ' ')" "20026"

# dead session drops out of ls
cat > tests/echo-die.tmp.sh <<'DIE'
#!/bin/sh
n=0
while IFS= read -r line; do
	n=$((n+1))
	method=$(printf '%s' "$line" | grep -o '"method":"[^"]*"' | head -1 | cut -d'"' -f4)
	id=$(printf '%s' "$line" | grep -o '"id":[0-9]*' | head -1 | cut -d: -f2)
	case "$method" in
	initialize)
		printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"die","version":"1.0"}}}\n' "$id"
		;;
	notifications/*)
		:
		;;
	tools/list)
		printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[]}}\n' "$id"
		;;
	*)
		printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
		;;
	esac
	if [ "$n" -ge 4 ]; then exit 0; fi
done
DIE
chmod +x tests/echo-die.tmp.sh
$MCPC connect cmd:"$PWD"/tests/echo-die.tmp.sh @doomed >/dev/null 2>&1
$MCPC @doomed tools-call nope >/dev/null 2>&1 || true
$MCPC @doomed tools-call nope >/dev/null 2>&1 || true
$MCPC @doomed tools-call nope >/dev/null 2>&1 || true
assert_eq "dead-session" "$($MCPC ls 2>&1 | grep -c '@doomed' || true)" "0"

# quoted session name in ls --json
$MCPC connect cmd:"$PWD"/tests/echo-mcp.tmp.sh "@we\"ird" >/dev/null 2>&1
assert_eq "quoted-name" "$($MCPC ls --json 2>&1 | grep -cF 'name":"we\"ird' || true)" "1"

# socket removal self-heal (restore_sessions brings @echo back live from cache;
# hitting "already live" proves the new daemon restored sessions correctly)
$MCPC connect cmd:"$PWD"/tests/echo-mcp.tmp.sh @base >/dev/null 2>&1
rm -f "$BA_HOME/mcpc/daemon.sock"
$MCPC ls >/dev/null 2>&1 || true
conn=$($MCPC connect cmd:"$PWD"/tests/echo-mcp.tmp.sh @echo 2>&1) || true
test "$(cat "$BA_HOME/mcpc/daemon.pid")" -gt 0 && ok=pidfile-ok || ok=pidfile-bad
assert_eq "self-heal" "$ok
$conn" "pidfile-ok
mcpc: connect @echo failed: session 'echo' already live"

# close keeps cache
assert_eq "close" "$($MCPC close @echo 2>&1; $MCPC @echo tools-list 2>&1 | head -1 | tr -s ' '; $MCPC @echo tools-call echo 2>&1 | grep -c 'no live session' || true)" "closed @echo
echo echo back its arguments
1"

# daemon stop
assert_eq "stop" "$($MCPC daemon stop 2>&1; ls "$BA_HOME/mcpc/daemon.sock" 2>&1 | grep -c 'No such' || true)" "daemon stopped
1"

# === HTTP transport ===================================================
ROOT=$PWD/tests/mc.e2e
WEB=$ROOT/www
PORT=$((21000 + $$ % 20000))
mkdir -p "$WEB/cgi-bin"
cp tests/http_cgi_mcp.cgi "$WEB/cgi-bin/mcp.cgi"
cp tests/http_cgi_mcp500.cgi "$WEB/cgi-bin/mcp500.cgi"
cp tests/http_cgi_mcpsse.cgi "$WEB/cgi-bin/mcpsse.cgi"
chmod +x "$WEB/cgi-bin/"*
python3 tests/http_server.py "$WEB" "$PORT" --daemon
for _ in $(seq 1 2400); do
    [ -f "$WEB/ready" ] && break
    sleep 0.05
done
if [ ! -f "$WEB/ready" ]; then
    echo "FAIL: http test server did not start"
    exit 1
fi
HTTPD_PID=$(cat "$WEB/httpd.pid" 2>/dev/null || echo "")
echo "--- proxy env ---"
env | grep -i proxy || echo "(none)"

assert_eq "http-connect" "$($MCPC connect http://127.0.0.1:$PORT/cgi-bin/mcp.cgi @web 2>&1)" "connected @web"
assert_eq "http-call" "$($MCPC @web tools-call web_search q=hello 2>&1)" '{"content":[{"type":"text","text":"web-result"}]}'
assert_eq "http-session-id" "$(grep -c 'sid=fixed-session-777' $WEB/requests.log)" "3"
assert_eq "http-500-notification" "$($MCPC connect http://127.0.0.1:$PORT/cgi-bin/mcp500.cgi @n500 2>&1 | grep -o 'connect @n500 failed'; echo rc=$?)" "connect @n500 failed
rc=0"
assert_eq "sse-connect" "$($MCPC connect http://127.0.0.1:$PORT/cgi-bin/mcpsse.cgi @sse 2>&1)" "connected @sse"
assert_eq "sse-call" "$($MCPC @sse tools-call sse_tool 2>&1)" '{"content":[{"type":"text","text":"sse-result"}]}'

HTTPD_PID=$(cat "$WEB/httpd.pid" 2>/dev/null || echo "")
kill "$HTTPD_PID" 2>/dev/null || true
wait "$HTTPD_PID" 2>/dev/null || true
rm -rf "$ROOT"

# restart implicit spawn
assert_eq "restart" "$($MCPC connect cmd:"$PWD"/tests/echo-mcp.tmp.sh @echo2 2>&1 && $MCPC daemon stop >/dev/null)" "connected @echo2"

if [ "$fail" -gt 0 ]; then
    echo "FAILED ($fail tests)"
    exit 1
fi
echo "ALL PASSED"
