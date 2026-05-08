#!/usr/bin/env python3
"""E2E test for Bastion MCP sandbox operations."""
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
    req_headers = dict(headers)
    if session_id:
        req_headers["mcp-session-id"] = session_id
    req = urllib.request.Request(URL, data=payload, headers=req_headers, method="POST")
    with urllib.request.urlopen(req, timeout=60) as response:
        resp_headers = dict(response.headers)
        body = response.read().decode()
        json_body = parse_sse(body)
        if json_body is None:
            try:
                json_body = json.loads(body) if body.strip() else None
            except json.JSONDecodeError:
                json_body = None
        return resp_headers, json_body

def rpc(method, params, id_, session_id):
    payload = json.dumps({"jsonrpc": "2.0", "id": id_, "method": method, "params": params}).encode()
    return post_json(payload, {
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
        "MCP-Protocol-Version": "2024-11-05",
    }, session_id)

# Initialize
_, init_resp = rpc("initialize", {
    "protocolVersion": "2024-11-05",
    "capabilities": {},
    "clientInfo": {"name": "bastion-e2e", "version": "1.0.0"},
}, 0, None)
session_id = None  # Will be set after initialize

# Get session ID from initialize response (need to do a fresh request to capture headers)
# Re-do initialize with header capture
init_payload = json.dumps({
    "jsonrpc": "2.0", "id": 0, "method": "initialize",
    "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "bastion-e2e", "version": "1.0.0"}}
}).encode()
req = urllib.request.Request(URL, data=init_payload, headers={
    "Content-Type": "application/json",
    "Accept": "application/json, text/event-stream",
    "MCP-Protocol-Version": "2024-11-05",
}, method="POST")
with urllib.request.urlopen(req, timeout=60) as response:
    session_id = dict(response.headers).get("mcp-session-id")
print(f"Session ID: {session_id}")

# Send initialized notification
notif_payload = json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}}).encode()
try:
    post_json(notif_payload, {
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
        "MCP-Protocol-Version": "2024-11-05",
    }, session_id)
    print("notifications/initialized: OK")
except urllib.error.HTTPError as e:
    print(f"notifications/initialized: HTTP {e.code} {e.reason}")

# Test: sandbox_health
print("\n=== sandbox_health ===")
_, resp = rpc("tools/call", {"name": "sandbox_health", "arguments": {}}, 1, session_id)
print(json.dumps(resp, indent=2) if resp else "No response")

# Test: sandbox_list
print("\n=== sandbox_list ===")
_, resp = rpc("tools/call", {"name": "sandbox_list", "arguments": {}}, 2, session_id)
print(json.dumps(resp, indent=2) if resp else "No response")

# Test: sandbox_create
print("\n=== sandbox_create ===")
_, resp = rpc("tools/call", {
    "name": "sandbox_create",
    "arguments": {"template": "debian:bookworm-slim", "timeout_ms": 60000}
}, 3, session_id)
print(json.dumps(resp, indent=2) if resp else "No response")
sandbox_id = None
if resp and "result" in resp:
    try:
        content = resp["result"]["content"][0]["text"]
        data = json.loads(content)
        sandbox_id = data.get("sandbox_id")
        print(f"Created sandbox: {sandbox_id}")
    except (KeyError, json.JSONDecodeError) as e:
        print(f"Failed to parse: {e}")

# Test: sandbox_run (if we have a sandbox)
if sandbox_id:
    print(f"\n=== sandbox_run in {sandbox_id} ===")
    _, resp = rpc("tools/call", {
        "name": "sandbox_run",
        "arguments": {"sandbox_id": sandbox_id, "command": "echo 'Hello from bastion!'", "timeout_ms": 10000}
    }, 4, session_id)
    print(json.dumps(resp, indent=2) if resp else "No response")

    # Test: sandbox_write and sandbox_read
    print(f"\n=== sandbox_write in {sandbox_id} ===")
    _, resp = rpc("tools/call", {
        "name": "sandbox_write",
        "arguments": {"sandbox_id": sandbox_id, "path": "/tmp/test.txt", "content": "Hello Bastion!\n"}
    }, 5, session_id)
    print(json.dumps(resp, indent=2) if resp else "No response")

    print(f"\n=== sandbox_read in {sandbox_id} ===")
    _, resp = rpc("tools/call", {
        "name": "sandbox_read",
        "arguments": {"sandbox_id": sandbox_id, "path": "/tmp/test.txt"}
    }, 6, session_id)
    print(json.dumps(resp, indent=2) if resp else "No response")

    # Cleanup
    print(f"\n=== sandbox_terminate {sandbox_id} ===")
    _, resp = rpc("tools/call", {
        "name": "sandbox_terminate",
        "arguments": {"sandbox_id": sandbox_id}
    }, 7, session_id)
    print(json.dumps(resp, indent=2) if resp else "No response")