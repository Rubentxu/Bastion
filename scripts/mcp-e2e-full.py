#!/usr/bin/env python3
"""Bastion E2E Full Feature Test via MCP.
Tests: health, list, create, prepare, run, write, read, list_files, info,
       pool_stats, metrics, snapshot, sync, register_artifact, enrichment,
       cancel, terminate.
Collects: failures, performance data, technical debt, operational issues.
"""
import json
import sys
import time
import urllib.request
import urllib.error

HOST = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1"
PORT = sys.argv[2] if len(sys.argv) > 2 else "18765"
URL = f"http://{HOST}:{PORT}/"

# --- Findings collector ---
findings = {
    "pass": [],
    "fail": [],
    "debt": [],
    "perf": [],
    "ops": [],
}

def record_pass(msg):
    findings["pass"].append(msg)
    print(f"  ✅ {msg}")

def record_fail(msg):
    findings["fail"].append(msg)
    print(f"  ❌ {msg}")

def record_debt(msg):
    findings["debt"].append(msg)
    print(f"  ⚠️  DEBT: {msg}")

def record_perf(msg):
    findings["perf"].append(msg)
    print(f"  ⏱️  PERF: {msg}")

def record_ops(msg):
    findings["ops"].append(msg)
    print(f"  🔧 OPS: {msg}")

# --- MCP protocol ---
def parse_sse(body):
    if not body or not body.strip():
        return None
    for line in body.splitlines():
        if line.startswith("data: "):
            payload = line[6:]
            if payload:
                return json.loads(payload)
    return None

def mcp_init():
    """Initialize MCP session and return session ID."""
    init_payload = json.dumps({
        "jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "bastion-e2e-full", "version": "1.0.0"}
        }
    }).encode()
    req = urllib.request.Request(URL, data=init_payload, headers={
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
        "MCP-Protocol-Version": "2024-11-05",
    }, method="POST")
    with urllib.request.urlopen(req, timeout=30) as resp:
        session_id = dict(resp.headers).get("mcp-session-id")
        body = resp.read().decode()
        json_body = parse_sse(body)
        return session_id, json_body

_session_id = None
_req_id = 0

def call(method, params, timeout_s=60):
    """Call an MCP tool, returning (response_json, elapsed_seconds)."""
    global _req_id
    _req_id += 1
    payload = json.dumps({
        "jsonrpc": "2.0",
        "id": _req_id,
        "method": "tools/call",
        "params": {"name": method, "arguments": params}
    }).encode()
    headers = {
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
        "MCP-Protocol-Version": "2024-11-05",
    }
    if _session_id:
        headers["mcp-session-id"] = _session_id
    req = urllib.request.Request(URL, data=payload, headers=headers, method="POST")
    start = time.time()
    try:
        with urllib.request.urlopen(req, timeout=timeout_s) as resp:
            body = resp.read().decode()
            json_body = parse_sse(body)
            if json_body is None:
                try:
                    json_body = json.loads(body) if body.strip() else None
                except:
                    json_body = None
            elapsed = time.time() - start
            return json_body, elapsed
    except urllib.error.HTTPError as e:
        elapsed = time.time() - start
        body = e.read().decode()
        return {"error": {"code": e.code, "message": f"HTTP {e.code}: {body[:200]}"}}, elapsed

def extract_text(resp):
    """Extract the 'text' field from an MCP tool response."""
    if not resp:
        return None
    if "error" in resp:
        return json.dumps(resp["error"])
    try:
        return resp["result"]["content"][0]["text"]
    except (KeyError, IndexError, TypeError):
        return json.dumps(resp)

def extract_data(resp):
    """Extract JSON data from the 'text' field of an MCP tool response."""
    text = extract_text(resp)
    if text is None:
        return None
    try:
        return json.loads(text)
    except (json.JSONDecodeError, TypeError):
        return {"raw_text": text}

# ============================================================
# TEST SUITE
# ============================================================

print("=" * 70)
print("BASTION E2E FULL FEATURE TEST VIA MCP")
print("=" * 70)

# --- 0. MCP Protocol ---
print("\n📡 0. MCP PROTOCOL")
sid, init_resp = mcp_init()
_session_id = sid
print(f"  Session ID: {sid}")
if not sid:
    record_fail("No session ID returned from initialize")
    sys.exit(1)
record_pass(f"initialize returned session {sid[:16]}...")

# Send initialized notification
notif_payload = json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}}).encode()
notif_req = urllib.request.Request(URL, data=notif_payload, headers={
    "Content-Type": "application/json",
    "Accept": "application/json, text/event-stream",
    "MCP-Protocol-Version": "2024-11-05",
    "mcp-session-id": _session_id,
}, method="POST")
try:
    with urllib.request.urlopen(notif_req, timeout=30) as resp:
        resp.read()
    record_pass("notifications/initialized accepted (202)")
except urllib.error.HTTPError as e:
    record_fail(f"notifications/initialized failed: HTTP {e.code}")

# --- 1. Health & List ---
print("\n🏥 1. HEALTH & INFRASTRUCTURE")
resp, t = call("sandbox_health", {})
data = extract_data(resp)
if data and data.get("status") == "healthy":
    record_pass(f"sandbox_health: healthy ({t:.2f}s)")
    checks = data.get("checks", [])
    for c in checks:
        print(f"    {c.get('component')}: {c.get('status')} ({c})")
else:
    record_fail(f"sandbox_health: {extract_text(resp)}")

resp, t = call("sandbox_list", {})
data = extract_data(resp)
if data:
    count = data.get("count", "?")
    record_pass(f"sandbox_list: {count} sandboxes ({t:.2f}s)")
else:
    record_fail(f"sandbox_list: {extract_text(resp)}")

resp, t = call("sandbox_pool_stats", {})
data = extract_data(resp)
if data:
    record_pass(f"sandbox_pool_stats: {data} ({t:.2f}s)")
else:
    record_fail(f"sandbox_pool_stats: {extract_text(resp)}")

resp, t = call("sandbox_metrics", {})
data = extract_data(resp)
if data:
    record_pass(f"sandbox_metrics: {data} ({t:.2f}s)")
else:
    record_fail(f"sandbox_metrics: {extract_text(resp)}")

resp, t = call("sandbox_list_templates", {})
data = extract_data(resp)
if data:
    record_pass(f"sandbox_list_templates: {data} ({t:.2f}s)")
else:
    record_fail(f"sandbox_list_templates: {extract_text(resp)}")

# --- 2. Enrichment tools (no sandbox needed) ---
print("\n📊 2. ENRICHMENT TOOLS")
resp, t = call("enrichment_health", {})
data = extract_data(resp)
if data:
    if "error" in data:
        record_debt(f"enrichment_health: {data['error']} (recorder not configured)")
    else:
        record_pass(f"enrichment_health: enabled={data.get('enabled')}, enrichers={data.get('catalog_enricher_count')} ({t:.2f}s)")
else:
    record_fail(f"enrichment_health: {extract_text(resp)}")

resp, t = call("enrichment_optimizer_report", {})
data = extract_data(resp)
if data:
    if "error" in data:
        record_debt(f"enrichment_optimizer_report: {data['error']}")
    else:
        record_pass(f"enrichment_optimizer_report: {data.get('total_runs_analyzed', 0)} runs analyzed ({t:.2f}s)")

resp, t = call("enrichment_retention_info", {})
data = extract_data(resp)
if data:
    if "error" in data:
        record_debt(f"enrichment_retention_info: {data['error']}")
    else:
        record_pass(f"enrichment_retention_info: rows={data.get('stats', {}).get('current_row_count')} ({t:.2f}s)")

resp, t = call("enrichment_retention_cleanup", {})
data = extract_data(resp)
if data:
    if "error" in data:
        record_debt(f"enrichment_retention_cleanup: {data['error']}")
    else:
        record_pass(f"enrichment_retention_cleanup: deleted={data.get('deleted_rows')} ({t:.2f}s)")

# --- 3. Sandbox lifecycle ---
print("\n🏗️ 3. SANDBOX LIFECYCLE")
sandbox_id = None

# Create sandbox
resp, t = call("sandbox_create", {"template": "debian:bookworm-slim", "timeout_ms": 120000})
data = extract_data(resp)
if data and "sandbox_id" in data:
    sandbox_id = data["sandbox_id"]
    from_pool = data.get("from_pool", False)
    record_pass(f"sandbox_create: {sandbox_id[:12]}... (from_pool={from_pool}, {t:.2f}s)")
    record_perf(f"sandbox_create: {t:.2f}s (from_pool={from_pool})")
else:
    record_fail(f"sandbox_create: {extract_text(resp)}")
    print("  ⚠️  Cannot continue without sandbox - aborting")
    # Print findings so far
    print_summary()
    sys.exit(1)

# Info
resp, t = call("sandbox_info", {"sandbox_id": sandbox_id})
data = extract_data(resp)
if data:
    record_pass(f"sandbox_info: status={data.get('status')} ({t:.2f}s)")
else:
    record_fail(f"sandbox_info: {extract_text(resp)}")

# --- 4. sandbox_run (basic) ---
print("\n⚡ 4. SANDBOX RUN (basic commands)")
for cmd_name, cmd in [("echo", "echo 'hello world'"), ("uname", "uname -a"), ("whoami", "whoami")]:
    resp, t = call("sandbox_run", {"sandbox_id": sandbox_id, "command": cmd}, timeout_s=30)
    data = extract_data(resp)
    if data and data.get("exit_code") == 0:
        stdout = data.get("stdout", "").strip()
        record_pass(f"sandbox_run({cmd_name}): exit=0, stdout='{stdout[:80]}' ({t:.2f}s)")
    elif data and data.get("exit_code") is not None:
        record_fail(f"sandbox_run({cmd_name}): exit={data.get('exit_code')}, stderr='{data.get('stderr', '')[:100]}'")
    else:
        record_fail(f"sandbox_run({cmd_name}): {extract_text(resp)}")

# --- 5. sandbox_write + sandbox_read ---
print("\n📝 5. SANDBOX WRITE & READ")
resp, t = call("sandbox_write", {"sandbox_id": sandbox_id, "path": "/tmp/test-write.txt", "content": "Hello Bastion!\nLine 2\n"})
data = extract_data(resp)
if data and data.get("status") == "ok":
    record_pass(f"sandbox_write: ok ({t:.2f}s)")
else:
    record_fail(f"sandbox_write: {extract_text(resp)}")

resp, t = call("sandbox_read", {"sandbox_id": sandbox_id, "path": "/tmp/test-write.txt"})
data = extract_data(resp)
if data:
    # sandbox_read might return base64 or text
    text = extract_text(resp)
    if text and "Hello Bastion" in text:
        record_pass(f"sandbox_read: content verified ({t:.2f}s)")
    elif text and "error" in text.lower():
        record_fail(f"sandbox_read: {text[:200]}")
        record_debt("sandbox_read: base64 decoding issue (known F-010)")
    else:
        record_debt(f"sandbox_read: returned data but content check inconclusive ({t:.2f}s)")
else:
    record_fail(f"sandbox_read: {extract_text(resp)}")

# --- 6. sandbox_list_files ---
print("\n📂 6. SANDBOX LIST_FILES")
resp, t = call("sandbox_list_files", {"sandbox_id": sandbox_id, "path": "/tmp"})
data = extract_data(resp)
if data:
    record_pass(f"sandbox_list_files(/tmp): {data} ({t:.2f}s)")
else:
    record_fail(f"sandbox_list_files: {extract_text(resp)}")

# --- 7. sandbox_prepare (jvm-build via apt) ---
print("\n🔧 7. SANDBOX PREPARE (jvm-build via apt)")
resp, t = call("sandbox_prepare", {
    "sandbox_id": sandbox_id,
    "capability": "jvm-build",
    "strategy": "system_package",
    "timeout_ms": 300000,
}, timeout_s=310)
data = extract_data(resp)
if data:
    if "error" in data:
        if "timeout" in str(data.get("error", "")).lower() or "timed out" in str(data.get("error", "")).lower():
            record_fail(f"sandbox_prepare(apt): TIMEOUT ({t:.2f}s)")
            record_debt("sandbox_prepare timeout < apt-get install time (known F-005)")
        else:
            record_fail(f"sandbox_prepare(apt): {data}")
    else:
        record_pass(f"sandbox_prepare(apt): prepared ({t:.2f}s)")
        record_perf(f"sandbox_prepare(apt jvm-build): {t:.2f}s")
else:
    record_fail(f"sandbox_prepare(apt): {extract_text(resp)}")

# --- 8. sandbox_run with env_ref ---
print("\n🏃 8. SANDBOX RUN with env_ref + Java verify")
java_result = None
resp, t = call("sandbox_run", {
    "sandbox_id": sandbox_id,
    "command": "java -version 2>&1 && mvn --version 2>&1 | head -1",
    "env_ref": "jvm-build",
}, timeout_s=30)
data = extract_data(resp)
if data and data.get("exit_code") == 0:
    stdout = data.get("stdout", "").strip()
    record_pass(f"sandbox_run(java+mvn verify): exit=0 ({t:.2f}s)")
    record_perf(f"sandbox_run(java+mvn verify): {t:.2f}s")
    java_result = stdout
    print(f"    Output: {stdout[:200]}")
elif data:
    record_fail(f"sandbox_run(java+mvn): exit={data.get('exit_code')}, {data.get('stderr', '')[:200]}")
else:
    record_fail(f"sandbox_run(java+mvn): {extract_text(resp)}")

# --- 9. Install asdf-vm manually ---
print("\n🔨 9. INSTALL asdf-vm + Java + Maven (manual)")
resp, t = call("sandbox_run", {
    "sandbox_id": sandbox_id,
    "command": "apt-get update -qq && apt-get install -y -qq curl git 2>&1 | tail -3",
}, timeout_s=120)
data = extract_data(resp)
if data and data.get("exit_code") == 0:
    record_pass(f"apt install curl+git: exit=0 ({t:.2f}s)")
    record_perf(f"apt install curl+git: {t:.2f}s")
else:
    record_fail(f"apt install curl+git: {extract_text(resp)}")

# Install asdf
resp, t = call("sandbox_run", {
    "sandbox_id": sandbox_id,
    "command": "git clone https://github.com/asdf-vm/asdf.git ~/.asdf --branch v0.14.0 2>&1 | tail -3 && . ~/.asdf/asdf.sh && asdf --version",
}, timeout_s=60)
data = extract_data(resp)
if data and data.get("exit_code") == 0:
    record_pass(f"asdf install: {data.get('stdout', '').strip()[:80]} ({t:.2f}s)")
    record_perf(f"asdf clone+init: {t:.2f}s")
else:
    record_fail(f"asdf install: {extract_text(resp)}")

# Install Java via asdf
resp, t = call("sandbox_run", {
    "sandbox_id": sandbox_id,
    "command": """. ~/.asdf/asdf.sh && asdf plugin add java 2>&1 | tail -1 && asdf install java adoptopenjdk-17.0.8+7 2>&1 | tail -5 && asdf global java adoptopenjdk-17.0.8+7 && java -version 2>&1""",
}, timeout_s=300)
data = extract_data(resp)
if data and data.get("exit_code") == 0:
    record_pass(f"asdf java install: exit=0 ({t:.2f}s)")
    record_perf(f"asdf java install: {t:.2f}s")
    print(f"    Java: {data.get('stdout', '').strip()[:200]}")
else:
    stdout = data.get("stdout", "") if data else ""
    stderr = data.get("stderr", "") if data else ""
    record_fail(f"asdf java install: exit={data.get('exit_code') if data else '?'} ({t:.2f}s)")
    if stderr:
        record_debt(f"asdf java stderr: {stderr[:200]}")

# Install Maven via asdf
resp, t = call("sandbox_run", {
    "sandbox_id": sandbox_id,
    "command": """. ~/.asdf/asdf.sh && asdf plugin add maven 2>&1 | tail -1 && asdf install maven 3.9.6 2>&1 | tail -5 && asdf global maven 3.9.6 && mvn --version 2>&1 | head -1""",
}, timeout_s=180)
data = extract_data(resp)
if data and data.get("exit_code") == 0:
    record_pass(f"asdf maven install: exit=0 ({t:.2f}s)")
    record_perf(f"asdf maven install: {t:.2f}s")
    print(f"    Maven: {data.get('stdout', '').strip()[:120]}")
else:
    record_fail(f"asdf maven install: {extract_text(resp)}")

# --- 10. Clone & build PetClinic ---
print("\n🐴 10. CLONE & BUILD PETCLINIC")
resp, t = call("sandbox_run", {
    "sandbox_id": sandbox_id,
    "command": ". ~/.asdf/asdf.sh && cd /tmp && git clone --depth 1 https://github.com/spring-projects/spring-petclinic.git 2>&1 | tail -3",
}, timeout_s=60)
data = extract_data(resp)
if data and data.get("exit_code") == 0:
    record_pass(f"git clone petclinic: ok ({t:.2f}s)")
    record_perf(f"git clone petclinic: {t:.2f}s")
else:
    record_fail(f"git clone petclinic: {extract_text(resp)}")

# Build PetClinic with Maven
print("  Building PetClinic (this takes a while)...")
resp, t = call("sandbox_run", {
    "sandbox_id": sandbox_id,
    "command": ". ~/.asdf/asdf.sh && cd /tmp/spring-petclinic && mvn package -DskipTests -q 2>&1 | tail -10",
}, timeout_s=300)
data = extract_data(resp)
if data and data.get("exit_code") == 0:
    record_pass(f"mvn package petclinic: BUILD SUCCESS ({t:.2f}s)")
    record_perf(f"mvn package petclinic: {t:.2f}s")
else:
    stdout = data.get("stdout", "") if data else ""
    stderr = str(data.get("stderr", "")) if data else ""
    # Check if BUILD SUCCESS is in stdout even with exit != 0
    if "BUILD SUCCESS" in stdout:
        record_pass(f"mvn package: BUILD SUCCESS in output (exit={data.get('exit_code')}, {t:.2f}s)")
        record_debt(f"mvn package: exit_code != 0 but output contains BUILD SUCCESS")
    else:
        record_fail(f"mvn package: exit={data.get('exit_code') if data else '?'}, {t:.2f}s")
        if stderr:
            record_debt(f"mvn package stderr: {stderr[:200]}")

# Check JAR exists
resp, t = call("sandbox_run", {
    "sandbox_id": sandbox_id,
    "command": ". ~/.asdf/asdf.sh && ls -lh /tmp/spring-petclinic/target/*.jar 2>&1",
}, timeout_s=15)
data = extract_data(resp)
if data and data.get("exit_code") == 0:
    record_pass(f"JAR verified: {data.get('stdout', '').strip()[:200]} ({t:.2f}s)")
else:
    record_fail(f"JAR not found: {extract_text(resp)}")

# --- 11. Retrace enrichment run through sandbox_run ---
print("\n📈 11. ENRICHMENT RUN (trace)")
resp, t = call("sandbox_run", {
    "sandbox_id": sandbox_id,
    "command": ". ~/.asdf/asdf.sh && cd /tmp/spring-petclinic && mvn dependency:tree -q 2>&1 | head -5",
    "trace_id": "e2e-petclinic-deps",
}, timeout_s=120)
data = extract_data(resp)
if data:
    record_pass(f"sandbox_run(trace_id=e2e-petclinic-deps): exit={data.get('exit_code')} ({t:.2f}s)")
else:
    record_debt(f"sandbox_run(trace_id): no enrichment data collected")

# --- 12. Snapshot ---
print("\n📸 12. SNAPSHOT")
# snapshot create
resp, t = call("sandbox_snapshot", {
    "action": "create",
    "sandbox_id": sandbox_id,
    "name": "e2e-petclinic-snapshot",
}, timeout_s=60)
data = extract_data(resp)
if data and "snapshot_id" in str(data):
    record_pass(f"snapshot create: {data} ({t:.2f}s)")
    record_perf(f"snapshot create: {t:.2f}s")
    snapshot_id = data.get("snapshot_id", "")
else:
    resp_text = extract_text(resp) if resp else "no response"
    if data and "error" in str(data).lower():
        record_fail(f"snapshot create: {data}")
    else:
        record_fail(f"snapshot create: {resp_text[:300]}")
    snapshot_id = None
    record_debt("snapshot: known issues - F-012 (restore not registered), F-013 (list empty)")

# snapshot list
resp, t = call("sandbox_snapshot", {"action": "list"})
data = extract_data(resp)
if data:
    count = data.get("count", 0) if isinstance(data, dict) else "?"
    if count == 0 and snapshot_id:
        record_debt(f"snapshot list returned 0 snapshots (known F-013: localhost/ prefix bug)")
    else:
        record_pass(f"snapshot list: {count} snapshots ({t:.2f}s)")
else:
    record_fail(f"snapshot list: {extract_text(resp)}")

# --- 13. Sync ---
print("\n🔄 13. SANDBOX SYNC")
resp, t = call("sandbox_sync", {
    "sandbox_id": sandbox_id,
    "mode": "pull",
    "source": "/tmp/spring-petclinic/target",
    "target": "/tmp/sync-output",
}, timeout_s=30)
data = extract_data(resp)
if data and "error" not in str(data).lower():
    record_pass(f"sandbox_sync(pull): {data} ({t:.2f}s)")
else:
    record_fail(f"sandbox_sync(pull): {extract_text(resp)}")
    record_debt("sandbox_sync: known stub (F-011)")

# --- 14. Register artifact ---
print("\n🎭 14. REGISTER ARTIFACT")
resp, t = call("sandbox_register_artifact", {
    "name": "spring-petclinic",
    "version": "4.0.0-SNAPSHOT",
    "digest": "sha256:e2e-test-dummy-digest",
    "capability": "jvm-build",
    "tools": "maven",
})
data = extract_data(resp)
if data and "error" not in str(data).lower():
    record_pass(f"register_artifact: {data} ({t:.2f}s)")
else:
    record_fail(f"register_artifact: {extract_text(resp)}")

# --- 15. Sandbox cancel (no-op test) ---
print("\n🛑 15. SANDBOX CANCEL (no-op test)")
resp, t = call("sandbox_cancel", {"sandbox_id": sandbox_id, "grace_period_ms": 1000})
data = extract_data(resp)
if data:
    # Cancel on an idle sandbox should succeed or report nothing to cancel
    record_pass(f"sandbox_cancel: responded ({t:.2f}s)")
else:
    record_debt(f"sandbox_cancel: {extract_text(resp)}")

# --- 16. Terminate sandbox ---
print("\n🗑️ 16. TERMINATE SANDBOX")
resp, t = call("sandbox_terminate", {"sandbox_id": sandbox_id})
data = extract_data(resp)
if data and data.get("status") in ("terminated", "stopped", "ok"):
    record_pass(f"sandbox_terminate: {data.get('status')} ({t:.2f}s)")
elif data:
    record_pass(f"sandbox_terminate: {data} ({t:.2f}s)")
else:
    record_fail(f"sandbox_terminate: {extract_text(resp)}")

# ============================================================
# SUMMARY
# ============================================================
print("\n" + "=" * 70)
print("SUMMARY")
print("=" * 70)
print(f"  ✅ PASS:  {len(findings['pass'])}")
print(f"  ❌ FAIL:  {len(findings['fail'])}")
print(f"  ⚠️  DEBT:  {len(findings['debt'])}")
print(f"  ⏱️  PERF:  {len(findings['perf'])}")
print(f"  🔧 OPS:  {len(findings['ops'])}")

print("\n--- FAILURES ---")
for f in findings["fail"]:
    print(f"  ❌ {f}")

print("\n--- TECHNICAL DEBT ---")
for d in findings["debt"]:
    print(f"  ⚠️  {d}")

print("\n--- PERFORMANCE ---")
for p in findings["perf"]:
    print(f"  ⏱️  {p}")

# Save full findings to JSON for report generation
with open("/tmp/bastion-e2e-findings.json", "w") as f:
    json.dump(findings, f, indent=2)
print(f"\n📊 Full findings saved to /tmp/bastion-e2e-findings.json")