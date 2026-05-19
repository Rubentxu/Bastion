# Bastion

Agent-first sandbox orchestration platform. AI agents interact via MCP; humans manage via REST dashboard.

## Language

**Sandbox**:
An isolated execution environment (container, VM, WASM module) managed by a Provider.
_Avoid_: container, VM, environment (when referring to the abstract concept)

**Provider**:
A backend that creates and runs Sandboxes (Podman, Docker, Firecracker, gVisor, Local, Wasm, Kubernetes, Lambda).
_Avoid_: driver, runtime, backend

**Provider Instance**:
A concrete, configured deployment of a Provider Type (e.g., "podman-local", "firecracker-vm-1").
_Avoid_: provider config, node

**Provider Type**:
The category of a Provider (podman, docker, firecracker, etc.) with associated Capabilities.
_Avoid_: provider kind, provider flavor

**Project**:
A directory containing `.bastion/` with `project.toml`, pipeline definitions, and runtime state. Groups Sandboxes by purpose.
_Avoid_: workspace, repo

**Pool**:
A set of warm, pre-created Sandboxes per Template, ready for immediate checkout.
_Avoid_: cache, pre-warm

**Worker**:
A process inside a Sandbox that executes commands and reports heartbeat/resource data back to the Gateway.
_Avoid_: agent, sidecar

**Doctor**:
A named health-check definition (TOML) that runs checks against a Provider or Sandbox.
_Avoid_: health check, diagnostic, probe

**Assertion**:
A named validation rule (TOML) evaluated against an Experience Record.
_Avoid_: validator, check, test

**Advice**:
A named guidance rule (TOML) triggered by assertion failures or experience patterns.
_Avoid_: recommendation, suggestion

**Experience Record**:
A structured log of a tool invocation (command, result, timing) stored for assertion evaluation and enrichment.
_Avoid_: execution log, trace, history

**Enricher**:
A named data-extraction rule (YAML) that derives Facts from Experience Records.
_Avoid_: extractor, analyzer

**Template**:
A container image or VM rootfs used to create Sandboxes, optionally registered with capabilities.
_Avoid_: image, base image

**Artifact**:
A versioned, verifiable, capability-providing file (OCI image, WASM module, rootfs tar, etc.).
_Avoid_: asset, package, layer

**Pipeline**:
A sequence of stages executed in Sandboxes, defined in `.bastion/pipelines/*.toml`.
_Avoid_: workflow, job, CI

**Capability**:
A named feature set (e.g., "jvm-build", "node-build") that can be prepared in a Sandbox.
_Avoid_: feature, skill

**BRN (Bastion Resource Name)**:
Uniform identifier for any resource: `brn:{namespace}:{type}[/{sub}]:{id}`.
_Avoid_: ARN, URI, resource ID

**Control Plane**:
The set of admin/management features (pool config, provider management, doctor runs, etc.) exposed via REST API.
_Avoid_: admin panel, management console

**Presentation Layer**:
A binding that exposes domain operations to consumers (MCP for AI agents, REST for dashboard).
_Avoid_: API, interface, facade

## Relationships

- A **Project** contains zero or more **Sandboxes**
- A **Sandbox** belongs to exactly one **Provider Instance**
- A **Provider Instance** belongs to exactly one **Provider Type**
- A **Pool** manages warm **Sandboxes** per **Template**
- A **Worker** runs inside a **Sandbox** and reports to the **Gateway**
- A **Doctor** runs **Checks** against a **Provider** or **Sandbox**
- An **Assertion** is evaluated against an **Experience Record**
- **Advice** is triggered by failed **Assertions** or **Experience** patterns
- An **Enricher** derives **Facts** from **Experience Records**
- A **Pipeline** executes **Stages** in **Sandboxes**
- A **Template** can have multiple **Artifacts**
- A **Capability** is prepared in a **Sandbox** using **Artifacts**

## Bounded Contexts

- **PROJECT**: Project aggregate, PipelineDef, SandboxPurpose, Template
- **SANDBOX**: Sandbox aggregate, StateMachine, Provider ports, Command execution
- **CATALOG**: ExperienceRecord, AssertionCheck, AdviceDescriptor, DoctorDescriptor, Enricher
- **INFRASTRUCTURE**: Pool, Worker, Provider registry, Config, Secrets
- **GATEWAY**: MCP server, REST API, RegistryService, Auth

## Example dialogue

> **Dev:** "When an AI agent creates a **Sandbox**, does it go through the **Pool**?"
> **Domain expert:** "Yes — `sandbox_create` checks the **Pool** for a warm **Sandbox** matching the **Template**. If none exists, it creates one and registers a **Worker**."

> **Dev:** "Can a **Doctor** check if a **Provider Instance** is alive?"
> **Domain expert:** "Yes — the `ProviderAlive` check verifies connectivity. But only `doctor_run` is exposed via MCP; individual checks are Control Plane territory."

## Flagged ambiguities

- "provider" was used to mean both **Provider Type** (podman) and **Provider Instance** (podman-local) — resolved: these are distinct concepts.
- "template" was used to mean both **Template** (container image) and **Pipeline Stage Template** — resolved: **Template** refers to the Sandbox image; Pipeline stages use **PipelineStage**.
- "check" was used to mean both **Doctor Check** (individual) and **Doctor** (named group of checks) — resolved: **Doctor** is the named definition; **Check** is one evaluation within.
