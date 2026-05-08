#!/usr/bin/env python3
"""Bastion MCP health check: initialize + initialized notification + sandbox_health over Streamable HTTP."""
import json
import sys
import urllib.request

HOST = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1"
PORT = sys.argv[2] if len(sys.argv) > 2 else "18765"
URL = f"http://{HOST}:{PORT}/"

def parse_sse(body):
    if not body or not body.strip():
        return None
    for line in body.splitlines():
        if line.startswith("data: "):
            payload = line[6:]
            if payload:
                return json.loads(payload)
    return None

def post_json(payload, headers, session_id=None):
    """POST JSON and return (response_headers, json_body or None)."""
    req_headers = dict(headers)
    if session_id:
        # Use lowercase header name - urllib normalizes headers to lowercase
        req_headers["mcp-session-id"] = session_id
    req = urllib.request.Request(
        URL,
        data=payload,
        headers=req_headers,
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=30) as response:
        resp_headers = dict(response.headers)
        body = response.read().decode()
        # Try SSE parsing first, fall back to plain JSON, return None if empty
        json_body = parse_sse(body)
        if json_body is None:
            try:
                json_body = json.loads(body) if body.strip() else None
            except json.JSONDecodeError:
                json_body = None
        return resp_headers, json_body

# Step 1: Send initialize
init_payload = json.dumps({
    "jsonrpc": "2.0",
    "id": 0,
    "method": "initialize",
    "params": {
        "protocolVersion": "2024-11-05",
        "capabilities": {},
        "clientInfo": {"name": "just-mcp-health", "version": "1.0.0"},
    }
}).encode()

resp_headers, init_resp = post_json(init_payload, {
    "Content-Type": "application/json",
    "Accept": "application/json, text/event-stream",
    "MCP-Protocol-Version": "2024-11-05",
})
print("=== INITIALIZE ===")
print(f"Response headers (keys): {list(resp_headers.keys())}")
print(f"mcp-session-id: {resp_headers.get('mcp-session-id')!r}")
print(f"Response body: {json.dumps(init_resp, indent=2)}")

session_id = resp_headers.get("mcp-session-id")
print(f"\nUsing session ID: {session_id!r}")

# Step 2: Send notifications/initialized
print("\n=== NOTIFICATIONS/INITIALIZED ===")
notif_payload = json.dumps({
    "jsonrpc": "2.0",
    "method": "notifications/initialized",
    "params": {"protocolVersion": init_resp.get("result", {}).get("protocolVersion", "2024-11-05")}
}).encode()
try:
    notif_resp = post_json(notif_payload, {
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
        "MCP-Protocol-Version": "2024-11-05",
    }, session_id)
    print(f"Notification response headers: {notif_resp[0]}")
    print(f"Notification response body: {notif_resp[1]}")
except urllib.error.HTTPError as e:
    print(f"Notification HTTP {e.code}: {e.reason}")
    print(f"Error body: {e.read().decode()}")

# Step 3: Call sandbox_health tool
print("\n=== SANDBOX_HEALTH ===")
health_payload = json.dumps({
    "jsonrpc": "2.0",
    "id": 1,
    "method": "tools/call",
    "params": {"name": "sandbox_health", "arguments": {}}
}).encode()
try:
    health_resp = post_json(health_payload, {
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
        "MCP-Protocol-Version": "2024-11-05",
    }, session_id)
    print(f"Response headers: {health_resp[0]}")
    print(f"Response body: {json.dumps(health_resp[1], indent=2)}")
except urllib.error.HTTPError as e:
    print(f"Health check HTTP {e.code}: {e.reason}")
    print(f"Error body: {e.read().decode()}")