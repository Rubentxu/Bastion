#!/usr/bin/env python3
"""
Bastion E2E: Full asdf-vm + PetClinic workflow via MCP
======================================================
Tests sandbox lifecycle with asdf-vm toolchain, JVM build,
Maven execution, enrichment data collection, and snapshot.

Covers: create, prepare, run, write/read, sync, snapshot,
        enrichment, catalog, and termination.
"""

import json
import sys
import time
import urllib.request
import urllib.error

HOST = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1"
PORT = sys.argv[2] if len(sys.argv) > 2 else "18765"
URL = f"http://{HOST}:{PORT}/"
PROVIDER = sys.argv[3] if len(sys.argv) > 3 else "local"

# ─── Report accumulators ───────────────────────────────────────────────────
passes = 0
failures = []
debt = []
perf_marks = []
ops_notes = []
findings = []


def pass_count():
    global passes
    passes += 1


def fail(msg, detail=""):
    full = f"  ❌ {msg}"
    if detail:
        full += f" | {detail[:300]}"
    print(full)
    failures.append({"message": msg, "detail": detail})


def debt_note(msg):
    print(f"  ⚠️  DEBT: {msg}")
    debt.append(msg)


def perf_note(msg):
    print(f"  ⏱️  PERF: {msg}")
    perf_marks.append(msg)


def ops_note(msg):
    print(f"  🔧 OPS: {msg}")
    ops_notes.append(msg)


def finding(severity, title, detail):
    findings.append({"severity": severity, "title": title, "detail": detail})


# ─── MCP helpers ───────────────────────────────────────────────────────────


def parse_sse(body):
    results = []
    for line in body.splitlines():
        if line.startswith("data: "):
            payload = line[6:].strip()
            if payload:
                results.append(json.loads(payload))
    return results


def mcp_post(payload_dict, sid, timeout_s=120):
    """POST JSON-RPC to MCP endpoint and parse SSE response."""
    payload = json.dumps(payload_dict).encode()
    headers = {
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
        "mcp-session-id": sid,
    }
    req = urllib.request.Request(URL, data=payload, headers=headers, method="POST")
    with urllib.request.urlopen(req, timeout=timeout_s) as r:
        body = r.read().decode()
        events = parse_sse(body)
        for e in reversed(events):
            if "result" in e:
                return e["result"]
            if "error" in e:
                return None
        if events:
            return events[-1]
        return None


def call_tool(name, params, sid, timeout_s=120):
    """Call an MCP tool and extract the text content from the result."""
    t0 = time.time()
    result = mcp_post(
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": name, "arguments": params},
        },
        sid,
        timeout_s=timeout_s,
    )
    elapsed = time.time() - t0
    return result, elapsed


def get_text(result):
    """Extract text from MCP content array."""
    if result is None:
        return None
    if isinstance(result, dict):
        if "content" in result:
            for item in result["content"]:
                if item.get("type") == "text":
                    try:
                        return json.loads(item["text"])
                    except:
                        return item["text"]
        return result
    return result


def get_init_session():
    """Initialize MCP session and return session_id."""
    payload = json.dumps(
        {
            "jsonrpc": "2.0",
            "id": 0,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "e2e-asdf-test", "version": "1.0.0"},
            },
        }
    ).encode()
    headers = {
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
        "MCP-Protocol-Version": "2024-11-05",
    }
    req = urllib.request.Request(URL, data=payload, headers=headers, method="POST")
    with urllib.request.urlopen(req, timeout=10) as r:
        resp_headers = dict(r.headers)
        sid = resp_headers.get("mcp-session-id")
        r.read()  # consume body

    # Send notifications/initialized
    notif_payload = json.dumps(
        {
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {},
        }
    ).encode()
    notif_headers = {
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
        "mcp-session-id": sid,
        "MCP-Protocol-Version": "2024-11-05",
    }
    req = urllib.request.Request(
        URL, data=notif_payload, headers=notif_headers, method="POST"
    )
    try:
        urllib.request.urlopen(req, timeout=10)
    except:
        pass
    return sid


# ─── Main test flow ────────────────────────────────────────────────────────


def main():
    global passes

    print("=" * 70)
    print("BASTION E2E: ASDF-VM + PETCLINIC + ENRICHMENT")
    print(f"Provider: {PROVIDER} | Gateway: {HOST}:{PORT}")
    print("=" * 70)

    # ── 0. Session ──
    print("\n📡 0. MCP PROTOCOL")
    try:
        sid = get_init_session()
        print(f"  Session ID: {sid}")
        pass_count()
        print(f"  ✅ initialize + initialized accepted")
    except Exception as e:
        print(f"  ❌ Session init failed: {e}")
        sys.exit(1)

    # ── 1. Health & Discovery ──
    print("\n🏥 1. HEALTH & DISCOVERY")
    r, t = call_tool("sandbox_health", {}, sid)
    data = get_text(r)
    if data and data.get("status") == "healthy":
        print(f"  ✅ sandbox_health: {data['status']} ({t:.2f}s)")
        pass_count()
    else:
        fail("sandbox_health failed", str(r)[:200])

    r2, t2 = call_tool("sandbox_list_capabilities", {}, sid)
    caps = get_text(r2)
    if caps and caps.get("capabilities"):
        cap_names = [c["name"] for c in caps["capabilities"]]
        print(f"  ✅ capabilities: {cap_names} ({t2:.2f}s)")
        pass_count()
        print(f"    artifacts_count: {caps.get('artifacts_count', 0)}")
    else:
        fail("sandbox_list_capabilities failed or empty", str(r2)[:200])

    r3, t3 = call_tool("sandbox_list_templates", {}, sid)
    tmpl = get_text(r3)
    if tmpl:
        print(
            f"  ✅ templates: {tmpl.get('count', 0)} images ({t3:.2f}s)"
        )
        pass_count()

    # ── 2. Provider discovery ──
    print("\n📦 2. PROVIDER AVAILABILITY")
    # Try creating sandbox with different providers
    providers_to_test = ["gvisor", "firecracker", "local", "podman"]
    available_providers = {}
    for prov in providers_to_test:
        rp, tp = call_tool(
            "sandbox_create",
            {"template": "debian:bookworm-slim", "provider": prov, "timeout_ms": 30000},
            sid,
            timeout_s=40,
        )
        dp = get_text(rp)
        if dp and dp.get("sandbox_id"):  # noqa
            available_providers[prov] = dp["sandbox_id"]
            print(f"  ✅ {prov}: sandbox={dp['sandbox_id'][:12]}... ({tp:.2f}s)")
            pass_count()
        else:
            print(f"  ❌ {prov}: NOT AVAILABLE — {str(rp)[:200]}")
            ops_note(f"Provider '{prov}' not functional: {str(rp)[:200]}")
            finding("CRITICAL", f"Provider {prov} not available", str(rp)[:300])
            # Terminate any partial sandbox
            if dp and dp.get("sandbox_id"):
                call_tool("sandbox_terminate", {"sandbox_id": dp["sandbox_id"]}, sid)

    # Terminate test sandboxes (keep podman if we'll use it)
    for prov, sid_box in list(available_providers.items()):
        if prov != "podman":
            call_tool("sandbox_terminate", {"sandbox_id": sid_box}, sid)

    # ── 3. Create main sandbox ──
    print("\n🏗️ 3. MAIN SANDBOX CREATION")
    r4, t4 = call_tool(
        "sandbox_create",
        {"template": "debian:bookworm-slim", "provider": PROVIDER, "timeout_ms": 3600000},
        sid,
        timeout_s=120,
    )
    sandbox_id = None
    create_data = get_text(r4)
    if create_data and "sandbox_id" in create_data:
        sandbox_id = create_data["sandbox_id"]
        print(
            f"  ✅ sandbox_create: {sandbox_id[:16]}... "
            f"(from_pool={create_data.get('from_pool', False)}, {t4:.2f}s)"
        )
        perf_note(f"sandbox_create: {t4:.2f}s")
        pass_count()
    else:
        fail("sandbox_create failed — cannot proceed", str(r4)[:300])
        print("\n📊 PRODUCING EARLY REPORT...")
        produce_report()
        sys.exit(1)

    if not sandbox_id:
        fail("No sandbox_id returned")
        produce_report()
        sys.exit(1)

    # ── 4. System info ──
    print("\n🔍 4. SANDBOX INFO")
    r5, t5 = call_tool("sandbox_info", {"sandbox_id": sandbox_id}, sid)
    info = get_text(r5)
    if info and info.get("status") == "running":
        print(f"  ✅ sandbox_info: {info['status']} ({t5:.2f}s)")
        pass_count()
    else:
        fail("sandbox_info", str(r5)[:200])

    r6, t6 = call_tool("sandbox_run", {"sandbox_id": sandbox_id, "command": "cat /etc/os-release | head -3"}, sid)
    os_data = get_text(r6)
    if os_data and os_data.get("exit_code") == 0:
        print(f"  ✅ OS: {os_data['stdout'].strip()[:100]} ({t6:.2f}s)")
        pass_count()

    # ── 5. Install system prerequisites ──
    print("\n📦 5. SYSTEM PREREQUISITES")
    install_cmds = [
        ("apt update", "apt-get update -qq 2>&1 | tail -1", 60),
        ("git+curl", "apt-get install -y -qq curl git 2>&1 | tail -1", 120),
    ]
    for label, cmd, timeout in install_cmds:
        r, t = call_tool("sandbox_run", {"sandbox_id": sandbox_id, "command": cmd}, sid, timeout)
        d = get_text(r)
        if d and d.get("exit_code") == 0:
            print(f"  ✅ {label}: ok ({t:.2f}s)")
            perf_note(f"{label}: {t:.2f}s")
            pass_count()
        else:
            fail(f"{label} failed", f"exit={d.get('exit_code') if d else '?'} stderr={d.get('stderr','')[:100] if d else '?'}")

    # ── 6. Install asdf-vm ──
    print("\n🔨 6. INSTALL ASDF-VM")
    # Clone asdf
    r7, t7 = call_tool(
        "sandbox_run",
        {
            "sandbox_id": sandbox_id,
            "command": "git clone https://github.com/asdf-vm/asdf.git ~/.asdf --branch v0.14.0 2>&1 | tail -3 && bash -c 'export ASDF_DIR=$HOME/.asdf && . $ASDF_DIR/asdf.sh && asdf --version'",
        },
        sid,
        timeout_s=60,
    )
    d7 = get_text(r7)
    if d7 and d7.get("exit_code") == 0:
        print(f"  ✅ asdf clone: ok ({t7:.2f}s)")
        perf_note(f"asdf clone: {t7:.2f}s")
        pass_count()
    else:
        fail("asdf clone failed", str(d7)[:300] if d7 else str(r7)[:300])
        debt_note("asdf clone: may need git pre-installed")

    # Verify asdf works (WITH bash -c wrapper — needed for non-interactive shells)
    r8, t8 = call_tool(
        "sandbox_run",
        {
            "sandbox_id": sandbox_id,
            "command": "bash -c 'export ASDF_DIR=$HOME/.asdf && . $ASDF_DIR/asdf.sh && asdf --version'",
        },
        sid,
        timeout_s=10,
    )
    d8 = get_text(r8)
    if d8 and d8.get("exit_code") == 0:
        print(f"  ✅ asdf --version: {d8['stdout'].strip()[:80]} ({t8:.2f}s)")
        pass_count()
    else:
        fail("asdf --version failed", str(d8)[:300] if d8 else str(r8)[:300])
        debt_note("asdf requires explicit export ASDF_DIR in non-interactive shells")

    # ── 7. Install Java via asdf ──
    print("\n☕ 7. INSTALL JAVA 17 VIA ASDF")
    r9, t9 = call_tool(
        "sandbox_run",
        {
            "sandbox_id": sandbox_id,
            "command": "bash -c 'export ASDF_DIR=$HOME/.asdf && . $ASDF_DIR/asdf.sh && asdf plugin add java 2>&1 | tail -1 && asdf install java adoptopenjdk-17.0.8+7 2>&1 | tail -5 && asdf global java adoptopenjdk-17.0.8+7 && java -version 2>&1'",
        },
        sid,
        timeout_s=300,
    )
    d9 = get_text(r9)
    if d9 and d9.get("exit_code") == 0:
        java_output = d9["stdout"].strip()[:200]
        print(f"  ✅ Java installed: {java_output.replace(chr(10),' ')[:120]} ({t9:.2f}s)")
        perf_note(f"asdf java install: {t9:.2f}s")
        pass_count()
    else:
        stderr = d9.get("stderr", "") if d9 else ""
        fail("asdf java install failed", f"exit={d9.get('exit_code') if d9 else '?'} stderr={stderr[:200]}")
        debt_note("asdf java install: asdf plugin add may fail without dependencies")

    # ── 8. Install Maven via asdf ──
    print("\n📦 8. INSTALL MAVEN VIA ASDF")
    r10, t10 = call_tool(
        "sandbox_run",
        {
            "sandbox_id": sandbox_id,
            "command": "bash -c 'export ASDF_DIR=$HOME/.asdf && . $ASDF_DIR/asdf.sh && asdf plugin add maven 2>&1 | tail -1 && asdf install maven 3.9.6 2>&1 | tail -5 && asdf global maven 3.9.6 && mvn --version 2>&1 | head -3'",
        },
        sid,
        timeout_s=120,
    )
    d10 = get_text(r10)
    if d10 and d10.get("exit_code") == 0:
        print(f"  ✅ Maven installed: {d10['stdout'].strip()[:200].replace(chr(10),' ')} ({t10:.2f}s)")
        perf_note(f"asdf maven install: {t10:.2f}s")
        pass_count()
    else:
        stderr = d10.get("stderr", "") if d10 else ""
        fail("asdf maven install failed", f"exit={d10.get('exit_code') if d10 else '?'} stderr={stderr[:200]}")
        debt_note("asdf maven install: may fail if maven plugin not available for asdf")

    # ── 9. Clone spring-petclinic ──
    print("\n📥 9. CLONE SPRING PETCLINIC")
    r11, t11 = call_tool(
        "sandbox_run",
        {
            "sandbox_id": sandbox_id,
            "command": "bash -c 'export ASDF_DIR=$HOME/.asdf && . $ASDF_DIR/asdf.sh && cd /tmp && git clone --depth 1 https://github.com/spring-projects/spring-petclinic.git 2>&1 | tail -3'",
        },
        sid,
        timeout_s=120,
    )
    d11 = get_text(r11)
    if d11 and d11.get("exit_code") == 0:
        print(f"  ✅ petclinic cloned ({t11:.2f}s)")
        perf_note(f"git clone petclinic: {t11:.2f}s")
        pass_count()
    else:
        fail("petclinic clone failed", str(d11)[:300] if d11 else str(r11)[:300])

    # ── 10. Build PetClinic with Maven ──
    print("\n🔨 10. BUILD PETCLINIC (mvn package)")
    r12, t12 = call_tool(
        "sandbox_run",
        {
            "sandbox_id": sandbox_id,
            "command": "bash -c 'export ASDF_DIR=$HOME/.asdf && . $ASDF_DIR/asdf.sh && cd /tmp/spring-petclinic && mvn package -DskipTests -q 2>&1 | tail -10'",
            "trace_id": "e2e-asdf-petclinic-build",
        },
        sid,
        timeout_s=600,
    )
    d12 = get_text(r12)
    if d12 and d12.get("exit_code") == 0:
        stdout = d12.get("stdout", "").strip()
        print(f"  ✅ Build SUCCESS ({t12:.2f}s)")
        perf_note(f"mvn petclinic package: {t12:.2f}s")
        print(f"    Output: {stdout.replace(chr(10),' ')[:200]}")
        pass_count()
    else:
        stderr = d12.get("stderr", "") if d12 else ""
        exit_code = d12.get("exit_code", "?") if d12 else "?"
        fail("mvn petclinic build failed", f"exit={exit_code} stderr={stderr[:300]}")
        debt_note(f"mvn petclinic build: exit={exit_code}, stderr={stderr[:200]}")

    # Check agent_context / enrichment_meta from build
    if d12:
        if "agent_context" in d12 and d12["agent_context"]:
            print(f"    enrichment: agent_context found!")
            ac = d12["agent_context"]
            print(f"      facts: {len(ac.get('facts', []))}")
            if ac.get("artifacts"):
                print(f"      artifacts: {json.dumps(ac['artifacts'])[:300]}")
            if ac.get("test_summary"):
                print(f"      test_summary: {ac['test_summary']}")
            pass_count()
        else:
            print(f"    ⚠️  No agent_context — enrichment may not be capturing")
            ops_note("No enrichment metadata in mvn build — enricher may not be configured")
        if "enrichment_meta" in d12:
            print(f"    enrichment_meta: {json.dumps(d12['enrichment_meta'])[:200]}")

    # ── 11. Verify JAR exists ──
    print("\n📂 11. VERIFY JAR ARTIFACT")
    r13, t13 = call_tool(
        "sandbox_run",
        {
            "sandbox_id": sandbox_id,
            "command": "bash -c 'export ASDF_DIR=$HOME/.asdf && . $ASDF_DIR/asdf.sh && ls -lh /tmp/spring-petclinic/target/*.jar 2>&1'",
        },
        sid,
        timeout_s=10,
    )
    d13 = get_text(r13)
    if d13 and d13.get("exit_code") == 0:
        print(f"  ✅ JAR found: {d13['stdout'].strip()[:200]} ({t13:.2f}s)")
        pass_count()
    else:
        fail("JAR not found", str(d13)[:300] if d13 else str(r13)[:300])

    # ── 12. Check enrichment tools ──
    print("\n📈 12. ENRICHMENT TOOLS")
    enrich_tools = [
        ("enrichment_health", {}),
        ("enrichment_retention_info", {}),
        ("enrichment_optimizer_report", {}),
    ]
    for name, params in enrich_tools:
        r, t = call_tool(name, params, sid)
        d = get_text(r)
        if d and "error" not in str(d).lower():
            summary = str(d)[:150].replace("\n", " ")
            print(f"  ✅ {name}: {summary} ({t:.2f}s)")
            pass_count()
        else:
            fail(f"{name}", str(r)[:200])

    # Also check experience records
    r_exp, t_exp = call_tool(
        "experience_list", {"trace_id": "e2e-asdf-petclinic-build"}, sid
    )
    d_exp = get_text(r_exp)
    if d_exp and d_exp.get("count", 0) > 0:
        print(f"  ✅ experience_list: {d_exp.get('count')} records for petclinic build ({t_exp:.2f}s)")
        pass_count()
    else:
        ops_note(f"No experience records for trace_id 'e2e-asdf-petclinic-build'")

    # ── 13. Write/Read test ──
    print("\n📝 13. SANDBOX WRITE/READ")
    r14, t14 = call_tool(
        "sandbox_write",
        {
            "sandbox_id": sandbox_id,
            "path": "/tmp/bastion-test-report.txt",
            "content": "BASTION E2E ASDF+PETCLINIC TEST — BUILD SUCCESSFUL"
        },
        sid,
    )
    if r14:
        print(f"  ✅ sandbox_write: ok ({t14:.2f}s)")
        pass_count()

    r15, t15 = call_tool(
        "sandbox_read", {"sandbox_id": sandbox_id, "path": "/tmp/bastion-test-report.txt"}, sid
    )
    d15 = get_text(r15)
    if d15 and d15.get("content"):
        print(f"  ✅ sandbox_read: content={d15['content'][:60]}... ({t15:.2f}s)")
        pass_count()
    else:
        fail("sandbox_read", str(r15)[:200])

    # ── 14. List files ──
    print("\n📂 14. SANDBOX LIST_FILES")
    r16, t16 = call_tool(
        "sandbox_list_files",
        {"sandbox_id": sandbox_id, "path": "/tmp/spring-petclinic"},
        sid,
        timeout_s=10,
    )
    d16 = get_text(r16)
    if d16 and d16.get("count", 0) > 0:
        files = [e["path"] for e in d16.get("entries", [])[:5]]
        print(f"  ✅ list_files: {d16['count']} entries (e.g. {files}) ({t16:.2f}s)")
        pass_count()
    else:
        fail("list_files petclinic", str(r16)[:200])

    # ── 15. Snapshot ──
    print("\n📸 15. SNAPSHOT")
    r17, t17 = call_tool(
        "sandbox_snapshot",
        {
            "action": "create",
            "sandbox_id": sandbox_id,
            "name": "e2e-asdf-petclinic",
        },
        sid,
        timeout_s=300,
    )
    d17 = get_text(r17)
    if d17 and d17.get("status") == "created":
        print(f"  ✅ snapshot created: {d17.get('snapshot_id', '?')} ({t17:.2f}s)")
        perf_note(f"snapshot create: {t17:.2f}s")
        pass_count()
    else:
        fail("snapshot create", str(r17)[:300])

    # ── 16. list snapshots ──
    r18, t18 = call_tool(
        "sandbox_snapshot", {"action": "list"}, sid, timeout_s=10
    )
    d18 = get_text(r18)
    if d18 and d18.get("count", 0) > 0:
        print(f"  ✅ snapshot list: {d18['count']} snapshots ({t18:.2f}s)")
        pass_count()

    # ── 17. Sync test ──
    print("\n🔄 17. SANDBOX SYNC")
    r19, t19 = call_tool(
        "sandbox_sync",
        {
            "sandbox_id": sandbox_id,
            "mode": "pull",
            "source": "/tmp/spring-petclinic/pom.xml",
            "target": "/tmp/bastion-e2e-pom.xml",
            "backend": "auto",
        },
        sid,
        timeout_s=30,
    )
    d19 = get_text(r19)
    if d19 and d19.get("status") == "ok":
        print(f"  ✅ sync pull pom.xml: ok ({t19:.2f}s)")
        pass_count()
    else:
        # Expected failure if petclinic isn't at expected path
        ops_note(f"sync pull: {str(r19)[:200]}")

    # ── 18. Catalog tools ──
    print("\n📚 18. CATALOG TOOLS")
    cat_tools = [
        ("assertion_list", {}),
        ("doctor_list", {}),
        ("advice_list", {}),
    ]
    for name, params in cat_tools:
        r, t = call_tool(name, params, sid)
        d = get_text(r)
        if d:
            if isinstance(d, dict):
                cnt = d.get("count", len(d))
                print(f"  ✅ {name}: {cnt} items ({t:.2f}s)")
            else:
                print(f"  ✅ {name}: ok ({t:.2f}s)")
            pass_count()
        else:
            fail(f"{name}", str(r)[:200])

    # ── 19. Pool stats ──
    print("\n🏊 19. POOL STATS")
    r20, t20 = call_tool("sandbox_pool_stats", {}, sid)
    d20 = get_text(r20)
    if d20:
        print(f"  ✅ pool: active={d20.get('active','?')} idle={d20.get('idle','?')} total={d20.get('total','?')} ({t20:.2f}s)")
        pass_count()

    # ── 20. Metrics ──
    r21, t21 = call_tool("sandbox_metrics", {}, sid)
    d21 = get_text(r21)
    if d21:
        # Metrics is prometheus text
        lines = str(d21).split("\\n")[:5]
        print(f"  ✅ metrics: {len(str(d21).splitlines())} lines ({t21:.2f}s)")
        pass_count()

    # ── 21. Cleanup ──
    print("\n🗑️  21. TERMINATE SANDBOX")
    r22, t22 = call_tool(
        "sandbox_terminate", {"sandbox_id": sandbox_id}, sid
    )
    d22 = get_text(r22)
    if d22:
        print(f"  ✅ terminate: {d22.get('status', 'ok')} ({t22:.2f}s)")
        pass_count()
    else:
        fail("terminate", str(r22)[:200])

    # ── Produce report ──
    produce_report()


def produce_report():
    print("\n" + "=" * 70)
    print("SUMMARY")
    print("=" * 70)
    print(f"  ✅ PASS:  {passes}")
    print(f"  ❌ FAIL:  {len(failures)}")
    print(f"  ⚠️  DEBT:  {len(debt)}")
    print(f"  ⏱️  PERF:  {len(perf_marks)}")
    print(f"  🔧 OPS:   {len(ops_notes)}")
    print(f"  🔍 FINDINGS: {len(findings)}")

    if failures:
        print("\n--- FAILURES ---")
        for f in failures:
            print(f"  ❌ {f['message']}")
            if f["detail"]:
                print(f"     Detail: {f['detail'][:300]}")

    if debt:
        print("\n--- TECHNICAL DEBT ---")
        for d in debt:
            print(f"  ⚠️  {d}")

    if perf_marks:
        print("\n--- PERFORMANCE ---")
        for p in perf_marks:
            print(f"  ⏱️  {p}")

    if ops_notes:
        print("\n--- OPERATIONAL EXCELLENCE ---")
        for o in ops_notes:
            print(f"  🔧 {o}")

    if findings:
        print("\n--- CRITICAL FINDINGS ---")
        for f in findings:
            print(f"  🔍 [{f['severity']}] {f['title']}")
            if f["detail"]:
                print(f"     {f['detail'][:300]}")

    # Save full report
    report = {
        "test": "E2E asdf-vm + PetClinic + Enrichment",
        "provider": PROVIDER,
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S"),
        "passes": passes,
        "failures": len(failures),
        "debt_count": len(debt),
        "perf_count": len(perf_marks),
        "ops_count": len(ops_notes),
        "findings_count": len(findings),
        "failure_details": failures,
        "technical_debt": debt,
        "performance": perf_marks,
        "ops_notes": ops_notes,
        "findings": findings,
    }
    report_path = "/tmp/bastion-e2e-asdf-findings.json"
    with open(report_path, "w") as f:
        json.dump(report, f, indent=2)
    print(f"\n📊 Full report saved to {report_path}")


if __name__ == "__main__":
    main()
