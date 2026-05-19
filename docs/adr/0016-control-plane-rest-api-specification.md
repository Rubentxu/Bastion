# ADR-0016: Control Plane REST API Specification

## Status

**Proposed** (2026-05-19)

## Context

ADR-0015 established the dual presentation layer architecture: MCP for AI-agent execution, REST for human-facing Control Plane. This ADR specifies the complete REST API surface — endpoints, methods, request/response shapes, authentication, error handling, and SSE event contracts.

The REST API serves two consumers:
1. **bastion-dashboard** (Leptos WASM UI embedded in the same binary)
2. **External integrations** (CI/CD, monitoring, custom tooling)

All endpoints map to BRN resources and enforce capability-based authorization per ADR-0015.

## Authentication

All endpoints require an API key via header:

```
Authorization: Bearer <api-key>
```

### Key Resolution

API keys are configured in gateway startup config (`--api-keys` or config file). Each key maps to a principal name and a `CapabilitySet`.

```json
{
  "api_keys": [
    { "key": "bcp-admin-xxx", "principal": "admin", "capabilities": ["ADMIN", "READONLY"] },
    { "key": "bcp-readonly-yyy", "principal": "viewer", "capabilities": ["READONLY"] }
  ]
}
```

### Response on Auth Failure

```json
HTTP 401 Unauthorized
{
  "error": "unauthorized",
  "message": "Invalid or missing API key"
}
```

```json
HTTP 403 Forbidden
{
  "error": "forbidden",
  "message": "Requires ADMIN capability",
  "required": ["ADMIN"],
  "actual": ["READONLY"]
}
```

## Common Patterns

### Pagination

List endpoints support cursor-based pagination:

```
GET /api/v1/sandboxes?limit=20&cursor=cursor_token
```

Response:
```json
{
  "items": [...],
  "next_cursor": "next_token_or_null",
  "has_more": true
}
```

### Errors

All errors follow RFC 7807 Problem Details:

```json
HTTP 4xx/5xx
{
  "type": "https://bastion.dev/errors/validation",
  "title": "Validation Failed",
  "status": 400,
  "detail": "Provider instance name must not be empty",
  "instance": "/api/v1/providers/instances"
}
```

Error types:
- `validation` (400) — Invalid input
- `unauthorized` (401) — Missing/invalid auth
- `forbidden` (403) — Insufficient capabilities
- `not_found` (404) — Resource does not exist
- `conflict` (409) — Duplicate resource
- `internal` (500) — Unexpected server error

### Timestamps

All timestamps are ISO 8601 UTC: `"2026-05-19T14:30:00Z"`

### BRN in Responses

Resources include their BRN for reference:

```json
{
  "brn": "brn:provider:instance:inst_abc123",
  "id": "inst_abc123",
  ...
}
```

---

## 1. Providers

### `GET /api/v1/providers`

List all registered provider types with aggregate status.

**Capability**: `READONLY`

**Response** `200`:
```json
{
  "items": [
    {
      "brn": "brn:provider:provider:podman",
      "type_id": "podman",
      "display_name": "Podman",
      "lifecycle_model": "Container",
      "capabilities": {
        "supports_snapshots": true,
        "supports_streaming": true,
        "supports_pause_resume": true,
        "supports_networking": true,
        "requires_kvm": false,
        "max_timeout_ms": 86400000,
        "max_memory_mb": 16384,
        "max_cpu_count": 16,
        "avg_startup_ms": 1500
      },
      "instance_count": 2,
      "healthy_instance_count": 1
    }
  ]
}
```

---

### `GET /api/v1/providers/:type_id`

Provider type detail.

**Capability**: `READONLY`

**Path params**: `type_id` (e.g., "podman", "docker", "firecracker")

**Response** `200`:
```json
{
  "brn": "brn:provider:provider:podman",
  "type_id": "podman",
  "display_name": "Podman",
  "description": "Podman container engine",
  "lifecycle_model": "Container",
  "capabilities": { "...same as above..." },
  "instances": [
    {
      "brn": "brn:provider:instance:inst_abc123",
      "id": "inst_abc123",
      "name": "podman-local",
      "status": "Active"
    }
  ]
}
```

**Error** `404`: Provider type not found.

---

### `GET /api/v1/providers/instances`

List all provider instances across all types.

**Capability**: `READONLY`

**Query params**: `?status=Active&type_id=podman`

**Response** `200`:
```json
{
  "items": [
    {
      "brn": "brn:provider:instance:inst_abc123",
      "id": "inst_abc123",
      "name": "podman-local",
      "type_id": "podman",
      "display_name": "Podman Local",
      "status": "Active",
      "created_at": "2026-05-19T10:00:00Z",
      "updated_at": "2026-05-19T14:30:00Z",
      "active_sandboxes": 3,
      "constraints": {
        "max_sandboxes": 10,
        "max_memory_mb": 16384,
        "max_cpu_count": 16
      }
    }
  ],
  "next_cursor": null,
  "has_more": false
}
```

---

### `GET /api/v1/providers/instances/:id`

Provider instance detail with config.

**Capability**: `READONLY`

**Response** `200`:
```json
{
  "brn": "brn:provider:instance:inst_abc123",
  "id": "inst_abc123",
  "name": "podman-local",
  "type_id": "podman",
  "display_name": "Podman Local",
  "description": "Local podman instance",
  "status": "Active",
  "created_at": "2026-05-19T10:00:00Z",
  "updated_at": "2026-05-19T14:30:00Z",
  "config": {
    "socket": "/run/podman/podman.sock",
    "image": "ubuntu:22.04",
    "worker_binary": "/usr/local/bin/bastion-worker",
    "mounts": [
      { "source": "/workspace", "target": "/workspace", "read_only": false }
    ],
    "network_mode": "bridge"
  },
  "constraints": {
    "max_sandboxes": 10,
    "max_memory_mb": 16384,
    "max_cpu_count": 16
  },
  "active_sandboxes": 3,
  "total_sandboxes_created": 157,
  "last_health_check": {
    "status": "Pass",
    "checked_at": "2026-05-19T14:25:00Z"
  }
}
```

---

### `POST /api/v1/providers/instances`

Register a new provider instance.

**Capability**: `ADMIN`

**Request**:
```json
{
  "name": "podman-local",
  "type_id": "podman",
  "display_name": "Podman Local",
  "description": "Local podman instance",
  "config": {
    "socket": "/run/podman/podman.sock",
    "image": "ubuntu:22.04",
    "worker_binary": "/usr/local/bin/bastion-worker",
    "mounts": [],
    "network_mode": "bridge"
  },
  "constraints": {
    "max_sandboxes": 10,
    "max_memory_mb": 16384,
    "max_cpu_count": 16
  }
}
```

**Response** `201`:
```json
{
  "brn": "brn:provider:instance:inst_new_uuid",
  "id": "inst_new_uuid",
  "name": "podman-local",
  "status": "Loading",
  "created_at": "2026-05-19T14:30:00Z"
}
```

**Errors**:
- `409` — Instance with same name already exists
- `400` — Invalid config (bad socket path, unknown type_id, etc.)

---

### `PUT /api/v1/providers/instances/:id`

Update provider instance config.

**Capability**: `ADMIN`

**Request**: Same shape as `POST` but all fields optional (partial update).

**Response** `200`: Updated instance (same as `GET` detail).

**Error** `409`: Name conflict with another instance.

---

### `DELETE /api/v1/providers/instances/:id`

Remove provider instance. Fails if sandboxes are active.

**Capability**: `ADMIN`

**Response** `204`: No content.

**Errors**:
- `409` — Cannot delete: 3 active sandboxes. Terminate them first.
- `404` — Instance not found.

---

### `PATCH /api/v1/providers/instances/:id/status`

Force status transition (for operational recovery).

**Capability**: `ADMIN`

**Request**:
```json
{
  "status": "Active"
}
```

**Valid transitions**: Any → `Active`, `Degraded`, `Offline`. `Failed` can be set from any state.

**Response** `200`: Updated instance status.

**Error** `422`: Invalid transition (e.g., `Offline` → `Active` without recovery).

---

## 2. Pool

### `GET /api/v1/pool`

Pool statistics.

**Capability**: `READONLY`

**Response** `200`:
```json
{
  "brn": "brn:infra:pool:default",
  "stats": {
    "active": 5,
    "idle": 8,
    "total": 13,
    "templates": [
      {
        "template_id": "ubuntu:22.04",
        "active": 3,
        "idle": 4,
        "total": 7,
        "min_idle": 2,
        "max_idle": 10,
        "max_total": 20
      },
      {
        "template_id": "eclipse-temurin:21-jdk",
        "active": 2,
        "idle": 4,
        "total": 6,
        "min_idle": 2,
        "max_idle": 8,
        "max_total": 15
      }
    ]
  },
  "config": {
    "idle_timeout_ms": 300000,
    "refill_interval_ms": 5000
  }
}
```

---

### `GET /api/v1/pool/config`

Pool configuration per template.

**Capability**: `READONLY`

**Response** `200`:
```json
{
  "global": {
    "idle_timeout_ms": 300000,
    "refill_interval_ms": 5000
  },
  "templates": {
    "ubuntu:22.04": {
      "min_idle": 2,
      "max_idle": 10,
      "max_total": 20
    },
    "eclipse-temurin:21-jdk": {
      "min_idle": 2,
      "max_idle": 8,
      "max_total": 15
    }
  }
}
```

---

### `PUT /api/v1/pool/config`

Update pool configuration.

**Capability**: `ADMIN`

**Request**:
```json
{
  "global": {
    "idle_timeout_ms": 600000
  },
  "templates": {
    "ubuntu:22.04": {
      "min_idle": 3,
      "max_idle": 15,
      "max_total": 30
    }
  }
}
```

All fields optional — partial update supported. Unchanged templates retain their config.

**Response** `200`: Updated config (same shape as `GET`).

---

### `POST /api/v1/pool/recover`

Trigger manual pool recovery (reconcile warm sandbox count with min_idle targets).

**Capability**: `ADMIN`

**Response** `200`:
```json
{
  "recovered": true,
  "actions": [
    { "template": "ubuntu:22.04", "created": 1, "removed": 0 },
    { "template": "eclipse-temurin:21-jdk", "created": 2, "removed": 1 }
  ]
}
```

---

### `GET /api/v1/pool/templates`

List pool templates with warm sandbox details.

**Capability**: `READONLY`

**Response** `200`:
```json
{
  "items": [
    {
      "template_id": "ubuntu:22.04",
      "warm_sandboxes": [
        {
          "sandbox_id": "sbx_warm_1",
          "created_at": "2026-05-19T14:00:00Z",
          "last_used_at": "2026-05-19T14:10:00Z",
          "idle_duration_ms": 1200000
        }
      ]
    }
  ]
}
```

---

## 3. Workers

### `GET /api/v1/workers`

List all connected workers.

**Capability**: `READONLY`

**Query params**: `?sandbox_id=sbx_123&status=connected`

**Response** `200`:
```json
{
  "items": [
    {
      "brn": "brn:infra:worker:wrk_abc123",
      "worker_id": "wrk_abc123",
      "sandbox_id": "sbx_xyz789",
      "status": "connected",
      "connected_at": "2026-05-19T13:00:00Z",
      "last_heartbeat_at": "2026-05-19T14:29:55Z",
      "circuit_breaker": "closed",
      "consecutive_failures": 0,
      "resource_usage": {
        "cpu_percent": 12.5,
        "memory_mb": 256,
        "disk_mb": 1024
      }
    }
  ]
}
```

---

### `GET /api/v1/workers/:id`

Worker detail with heartbeat history.

**Capability**: `READONLY`

**Response** `200`:
```json
{
  "brn": "brn:infra:worker:wrk_abc123",
  "worker_id": "wrk_abc123",
  "sandbox_id": "sbx_xyz789",
  "session_token": "tok_redacted",
  "status": "connected",
  "connected_at": "2026-05-19T13:00:00Z",
  "last_heartbeat_at": "2026-05-19T14:29:55Z",
  "circuit_breaker": "closed",
  "consecutive_failures": 0,
  "heartbeat_interval_ms": 5000,
  "resource_usage": {
    "cpu_percent": 12.5,
    "memory_mb": 256,
    "disk_mb": 1024,
    "network_rx_bytes": 4096,
    "network_tx_bytes": 1024
  },
  "heartbeat_history": [
    {
      "timestamp": "2026-05-19T14:29:55Z",
      "cpu_percent": 12.5,
      "memory_mb": 256
    },
    {
      "timestamp": "2026-05-19T14:29:50Z",
      "cpu_percent": 11.0,
      "memory_mb": 254
    }
  ]
}
```

---

### `DELETE /api/v1/workers/:id`

Force remove a worker (for dead/disconnected workers).

**Capability**: `ADMIN`

**Response** `204`: No content.

**Error** `409`: Worker is still connected. Disconnect first or wait for timeout.

---

## 4. Doctors

### `GET /api/v1/doctors`

List all registered doctors.

**Capability**: `READONLY`

**Response** `200`:
```json
{
  "items": [
    {
      "brn": "brn:catalog:doctor:podman-readiness",
      "id": "podman-readiness",
      "description": "Podman provider readiness checks",
      "check_count": 5,
      "last_result": {
        "status": "Pass",
        "ran_at": "2026-05-19T14:00:00Z",
        "duration_ms": 1200
      }
    }
  ]
}
```

---

### `GET /api/v1/doctors/:id`

Doctor detail with TOML source and individual checks.

**Capability**: `READONLY`

**Response** `200`:
```json
{
  "brn": "brn:catalog:doctor:podman-readiness",
  "id": "podman-readiness",
  "description": "Podman provider readiness checks",
  "toml_source": "[doctor]\nid = \"podman-readiness\"\n...",
  "checks": [
    {
      "name": "provider_alive",
      "type": "ProviderAlive",
      "description": "Verify podman socket connectivity"
    },
    {
      "name": "binary_available",
      "type": "BinaryAvailable",
      "description": "Check bastion-worker binary exists",
      "binary_name": "bastion-worker"
    },
    {
      "name": "image_available",
      "type": "ImageAvailable",
      "description": "Check base image is pulled",
      "image": "ubuntu:22.04"
    }
  ],
  "last_result": {
    "status": "Pass",
    "ran_at": "2026-05-19T14:00:00Z",
    "duration_ms": 1200,
    "check_results": [
      { "name": "provider_alive", "status": "Pass", "duration_ms": 100 },
      { "name": "binary_available", "status": "Pass", "duration_ms": 50 },
      { "name": "image_available", "status": "Pass", "duration_ms": 1050 }
    ]
  }
}
```

---

### `POST /api/v1/doctors/:id/run`

Run a doctor against a target.

**Capability**: `ADMIN`

**Request**:
```json
{
  "target": {
    "type": "provider_instance",
    "id": "inst_abc123"
  },
  "timeout_ms": 30000
}
```

Valid target types: `provider_instance`, `sandbox`, `system`

**Response** `200`:
```json
{
  "brn": "brn:catalog:doctor:podman-readiness",
  "run_id": "run_uuid",
  "status": "Pass",
  "started_at": "2026-05-19T14:30:00Z",
  "finished_at": "2026-05-19T14:30:01Z",
  "duration_ms": 1200,
  "target": {
    "type": "provider_instance",
    "id": "inst_abc123"
  },
  "check_results": [
    {
      "name": "provider_alive",
      "status": "Pass",
      "duration_ms": 100,
      "detail": "Podman socket responding at /run/podman/podman.sock",
      "remediation": null
    },
    {
      "name": "image_available",
      "status": "Fail",
      "duration_ms": 1050,
      "detail": "Image 'ubuntu:24.04' not found locally",
      "remediation": {
        "confidence": 0.95,
        "auto_fixable": true,
        "commands": ["podman pull ubuntu:24.04"],
        "manual_steps": [],
        "verify_after": ["podman image inspect ubuntu:24.04"]
      }
    }
  ],
  "summary": {
    "total": 5,
    "passed": 4,
    "failed": 1,
    "skipped": 0,
    "errors": 0
  }
}
```

---

### `GET /api/v1/doctors/:id/results`

Doctor run history.

**Capability**: `READONLY`

**Query params**: `?limit=20&status=Fail`

**Response** `200`:
```json
{
  "items": [
    {
      "run_id": "run_uuid",
      "status": "Pass",
      "ran_at": "2026-05-19T14:30:00Z",
      "duration_ms": 1200,
      "target_type": "provider_instance",
      "target_id": "inst_abc123",
      "summary": { "total": 5, "passed": 5, "failed": 0, "skipped": 0, "errors": 0 }
    }
  ],
  "next_cursor": null,
  "has_more": false
}
```

---

## 5. Catalog — Assertions

### `GET /api/v1/catalog/assertions`

**Capability**: `READONLY`

**Response** `200`:
```json
{
  "items": [
    {
      "brn": "brn:catalog:assertion:exit-code-zero",
      "id": "exit-code-zero",
      "description": "Verify command exits with code 0",
      "check_type": "ExitCode"
    }
  ]
}
```

### `GET /api/v1/catalog/assertions/:id`

**Capability**: `READONLY`

**Response** `200`:
```json
{
  "brn": "brn:catalog:assertion:exit-code-zero",
  "id": "exit-code-zero",
  "description": "Verify command exits with code 0",
  "toml_source": "[assertion]\nid = \"exit-code-zero\"\n...",
  "check": {
    "type": "ExitCode",
    "expected": 0
  }
}
```

### `POST /api/v1/catalog/assertions`

**Capability**: `ADMIN`

**Request**:
```json
{
  "id": "build-success",
  "description": "Build must succeed",
  "check": {
    "type": "ExitCode",
    "expected": 0
  }
}
```

**Response** `201`: Created assertion (same as `GET` detail).

### `PUT /api/v1/catalog/assertions/:id`

**Capability**: `ADMIN`

**Request**: Same as `POST`. Full replacement.

**Response** `200`: Updated assertion.

### `DELETE /api/v1/catalog/assertions/:id`

**Capability**: `ADMIN`

**Response** `204`: No content.

### `POST /api/v1/catalog/assertions/:id/run`

**Capability**: `ADMIN`

**Request**:
```json
{
  "experience_id": "exp_abc123"
}
```

**Response** `200`:
```json
{
  "assertion_id": "build-success",
  "experience_id": "exp_abc123",
  "passed": true,
  "check_type": "ExitCode",
  "expected": 0,
  "actual": 0,
  "evaluated_at": "2026-05-19T14:30:00Z"
}
```

---

## 6. Catalog — Advice

### `GET /api/v1/catalog/advice`

**Capability**: `READONLY`

**Response** `200`:
```json
{
  "items": [
    {
      "brn": "brn:catalog:advice:oom-restart",
      "id": "oom-restart",
      "description": "Restart sandbox on OOM",
      "severity": "Warning",
      "trigger": "ExperiencePattern",
      "enabled": true
    }
  ]
}
```

### `GET /api/v1/catalog/advice/:id`

**Capability**: `READONLY`

**Response** `200`: Full advice descriptor with TOML source.

### `PATCH /api/v1/catalog/advice/:id/toggle`

**Capability**: `ADMIN`

**Request**:
```json
{
  "enabled": false
}
```

**Response** `200`: Updated advice descriptor.

### `POST /api/v1/catalog/advice/suggest`

**Capability**: `READONLY`

**Request**:
```json
{
  "assertion_failures": ["build-success"],
  "doctor_failures": [],
  "experience_patterns": ["timeout-frequent"]
}
```

**Response** `200`:
```json
{
  "suggestions": [
    {
      "advice_id": "increase-timeout",
      "severity": "Hint",
      "message": "Consider increasing timeout — 3 of last 5 runs timed out",
      "confidence": 0.85,
      "context": {
        "pattern": "timeout-frequent",
        "occurrences": 3,
        "window_runs": 5
      }
    }
  ]
}
```

---

## 7. Catalog — Experience Records

### `GET /api/v1/catalog/experiences`

**Capability**: `READONLY`

**Query params**: `?trace_id=trace_abc&tool_name=sandbox_run&status=Failure&limit=20&cursor=token`

**Response** `200`:
```json
{
  "items": [
    {
      "brn": "brn:catalog:experience:exp_uuid",
      "id": "exp_uuid",
      "trace_id": "trace_abc",
      "tool_name": "sandbox_run",
      "sandbox_id": "sbx_xyz789",
      "status": "Success",
      "exit_code": 0,
      "started_at": "2026-05-19T14:00:00Z",
      "finished_at": "2026-05-19T14:00:05Z",
      "duration_ms": 5000
    }
  ],
  "next_cursor": "token",
  "has_more": true
}
```

### `GET /api/v1/catalog/experiences/:id`

**Capability**: `READONLY`

**Response** `200`:
```json
{
  "brn": "brn:catalog:experience:exp_uuid",
  "id": "exp_uuid",
  "trace_id": "trace_abc",
  "tool_name": "sandbox_run",
  "sandbox_id": "sbx_xyz789",
  "status": "Success",
  "exit_code": 0,
  "command": "cargo test",
  "started_at": "2026-05-19T14:00:00Z",
  "finished_at": "2026-05-19T14:00:05Z",
  "duration_ms": 5000,
  "stdout_summary": "running 42 tests...\ntest result: ok. 42 passed",
  "stderr_summary": "",
  "metadata": {
    "template": "eclipse-temurin:21-jdk-maven",
    "provider": "podman-local",
    "cpu_count": 4,
    "memory_mb": 4096
  }
}
```

---

## 8. Catalog — Enrichers

### `GET /api/v1/catalog/enrichers`

**Capability**: `READONLY`

**Response** `200`:
```json
{
  "items": [
    {
      "id": "maven-artifact-detector",
      "description": "Extract Maven artifact versions from build output",
      "extractor_count": 3,
      "rules_count": 5,
      "enabled": true
    }
  ]
}
```

### `GET /api/v1/catalog/enrichers/health`

**Capability**: `READONLY`

**Response** `200`:
```json
{
  "status": "healthy",
  "last_extraction_at": "2026-05-19T14:00:00Z",
  "total_facts_extracted": 1247,
  "active_enrichers": 3,
  "errors_last_24h": 0
}
```

### `GET /api/v1/catalog/enrichers/retention`

**Capability**: `READONLY`

**Response** `200`:
```json
{
  "config": {
    "max_age_days": 90,
    "max_records": 100000,
    "cleanup_interval_hours": 24
  },
  "stats": {
    "total_records": 45230,
    "oldest_record": "2026-02-18T10:00:00Z",
    "newest_record": "2026-05-19T14:30:00Z",
    "db_size_bytes": 12582912
  }
}
```

### `POST /api/v1/catalog/enrichers/retention/cleanup`

**Capability**: `ADMIN`

**Response** `200`:
```json
{
  "deleted": 1234,
  "remaining": 43996,
  "freed_bytes": 2097152
}
```

---

## 9. Config

### `GET /api/v1/config`

**Capability**: `READONLY`

**Response** `200`:
```json
{
  "brn": "brn:infra:config:gateway",
  "gateway": {
    "name": "bastion-gw-1",
    "grpc_port": 50052,
    "http_port": 8080,
    "worker_heartbeat_interval_ms": 5000,
    "worker_timeout_ms": 30000,
    "circuit_breaker_threshold": 5,
    "rate_limit_per_sandbox": 100,
    "default_sandbox_timeout_ms": 300000
  },
  "providers_path": ".bastion/providers/",
  "catalog_path": ".bastion/catalog/",
  "templates_path": ".bastion/templates/",
  "secrets": {
    "configured_keys": ["DOCKER_HUB_TOKEN", "GITHUB_TOKEN"],
    "resolved_count": 2,
    "unresolved": []
  }
}
```

### `PUT /api/v1/config`

**Capability**: `ADMIN`

**Request**: Partial update of gateway config fields.

```json
{
  "gateway": {
    "default_sandbox_timeout_ms": 600000,
    "rate_limit_per_sandbox": 200
  }
}
```

**Response** `200`: Updated config (same shape as `GET`).

### `GET /api/v1/config/history`

**Capability**: `READONLY`

**Query params**: `?limit=20`

**Response** `200`:
```json
{
  "items": [
    {
      "timestamp": "2026-05-19T14:30:00Z",
      "key": "gateway.default_sandbox_timeout_ms",
      "old_value": "300000",
      "new_value": "600000",
      "changed_by": "admin"
    }
  ],
  "next_cursor": null,
  "has_more": false
}
```

---

## 10. Secrets

### `GET /api/v1/secrets`

**Capability**: `READONLY`

**Response** `200`:
```json
{
  "items": [
    {
      "brn": "brn:infra:secret:DOCKER_HUB_TOKEN",
      "id": "DOCKER_HUB_TOKEN",
      "created_at": "2026-05-01T10:00:00Z",
      "updated_at": "2026-05-15T08:00:00Z"
    }
  ]
}
```

Values are **never** exposed in any response.

### `POST /api/v1/secrets`

**Capability**: `ADMIN`

**Request**:
```json
{
  "id": "DOCKER_HUB_TOKEN",
  "value": "dckr_pat_xxx..."
}
```

**Response** `201`:
```json
{
  "brn": "brn:infra:secret:DOCKER_HUB_TOKEN",
  "id": "DOCKER_HUB_TOKEN",
  "created_at": "2026-05-19T14:30:00Z"
}
```

**Error** `409`: Secret already exists. Use `PUT` to rotate.

### `PUT /api/v1/secrets/:id`

**Capability**: `ADMIN`

**Request**:
```json
{
  "value": "dckr_pat_yyy_new..."
}
```

**Response** `200`: Updated secret metadata (no value).

### `DELETE /api/v1/secrets/:id`

**Capability**: `ADMIN`

**Response** `204`: No content.

---

## 11. Metrics

### `GET /api/v1/metrics`

**Capability**: `READONLY`

**Query params**: `?format=json` (default) or `?format=prometheus`

**Response** `200` (JSON format):
```json
{
  "timestamp": "2026-05-19T14:30:00Z",
  "sandboxes": {
    "total": 13,
    "active": 5,
    "idle": 8,
    "by_status": {
      "Running": 5,
      "Pending": 1,
      "Stopped": 4,
      "Failed": 3
    },
    "by_provider": {
      "podman": 8,
      "firecracker": 3,
      "gvisor": 2
    }
  },
  "pool": {
    "hit_rate_percent": 78.5,
    "avg_checkout_ms": 150,
    "avg_warm_create_ms": 2500
  },
  "resources": {
    "total_cpu_percent": 45.2,
    "total_memory_mb": 8192,
    "total_disk_mb": 32768
  },
  "workers": {
    "connected": 5,
    "circuit_breaker_open": 0
  }
}
```

### `GET /api/v1/metrics/history`

**Capability**: `READONLY`

**Query params**: `?since=2026-05-19T00:00:00Z&interval=5m`

**Response** `200`:
```json
{
  "interval": "5m",
  "data_points": [
    {
      "timestamp": "2026-05-19T14:00:00Z",
      "active_sandboxes": 4,
      "pool_hit_rate": 75.0,
      "total_cpu_percent": 32.1,
      "total_memory_mb": 6144
    },
    {
      "timestamp": "2026-05-19T14:05:00Z",
      "active_sandboxes": 5,
      "pool_hit_rate": 78.5,
      "total_cpu_percent": 45.2,
      "total_memory_mb": 8192
    }
  ]
}
```

### `GET /api/v1/metrics/resources`

**Capability**: `READONLY`

**Query params**: `?sandbox_id=sbx_123` (optional, omits for all)

**Response** `200`:
```json
{
  "items": [
    {
      "sandbox_id": "sbx_xyz789",
      "provider": "podman-local",
      "cpu_percent": 12.5,
      "memory_mb": 256,
      "disk_mb": 1024,
      "network_rx_bytes": 4096,
      "network_tx_bytes": 1024,
      "measured_at": "2026-05-19T14:29:55Z"
    }
  ]
}
```

---

## 12. SSE Event Stream

### `GET /api/v1/events`

**Capability**: `READONLY`

**Headers**: `Accept: text/event-stream`

**Events**:

```
event: sandbox_created
data: {"sandbox_id":"sbx_new","template":"ubuntu:22.04","provider":"podman-local","purpose":"AdHocTest"}

event: sandbox_status_changed
data: {"sandbox_id":"sbx_new","old_status":"Pending","new_status":"Running"}

event: sandbox_terminated
data: {"sandbox_id":"sbx_old","reason":"timeout","duration_ms":300000}

event: metrics_update
data: {"active_sandboxes":5,"total_cpu_percent":45.2,"total_memory_mb":8192}

event: pool_event
data: {"template":"ubuntu:22.04","action":"checkout","idle_remaining":3}

event: worker_heartbeat
data: {"worker_id":"wrk_abc","sandbox_id":"sbx_xyz","cpu_percent":12.5,"memory_mb":256}

event: doctor_result
data: {"doctor_id":"podman-readiness","status":"Pass","target_type":"provider_instance","target_id":"inst_abc","duration_ms":1200}

event: config_changed
data: {"key":"gateway.default_sandbox_timeout_ms","old_value":"300000","new_value":"600000","changed_by":"admin"}

event: advice_triggered
data: {"advice_id":"oom-restart","severity":"Warning","sandbox_id":"sbx_xyz","message":"Sandbox exceeded memory limit"}

event: pipeline_started
data: {"pipeline":"build-and-test","project":"my-project","stages":4}

event: stage_completed
data: {"pipeline":"build-and-test","stage":"build","status":"Success","duration_ms":45000}

event: pipeline_completed
data: {"pipeline":"build-and-test","status":"Success","total_duration_ms":120000}
```

Clients send `Last-Event-ID` header to resume from last received event.

---

## Endpoint Summary

| Module | Endpoints | Methods |
|--------|-----------|---------|
| Providers | 8 | GET, POST, PUT, DELETE, PATCH |
| Pool | 5 | GET, PUT, POST |
| Workers | 3 | GET, DELETE |
| Doctors | 4 | GET, POST |
| Catalog: Assertions | 6 | GET, POST, PUT, DELETE |
| Catalog: Advice | 4 | GET, PATCH, POST |
| Catalog: Experience | 2 | GET |
| Catalog: Enrichers | 4 | GET, POST |
| Config | 3 | GET, PUT |
| Secrets | 4 | GET, POST, PUT, DELETE |
| Metrics | 3 | GET |
| SSE | 1 | GET (stream) |
| **Total** | **47 REST + 1 SSE** | |
