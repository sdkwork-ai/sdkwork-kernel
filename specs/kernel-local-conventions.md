# Kernel Local Conventions

- Version: 1.0
- Scope: sdkwork-kernel repository-specific execution rules that narrow SDKWORK standards
- Related: `AGENTS.md`, `../sdkwork-specs/AGENTS_SPEC.md`

This document contains repository-specific guidance derived from the previous `AGENTS.md` and complies with SDKWORK standards §9 by moving durable local rules out of `AGENTS.md`.

---

## 1. Project Structure & Module Organization

This repository defines the SDKWork kernel standard for agent and code-agent systems. Rust crates live at:

### L0 SPI Layer
- `sdkwork-agent-kernel/` - Agent SPI definitions (18 core provider families plus extension SPI: secret, sandbox, cancellation, rate_limit, model_stream, session_activity, a2a, backend_health, plugin, resilience, hooks, stream events, layered settings, plugin contributions, sandboxed execution coordination)
- `sdkwork-code-kernel/` - Code-agent SPI definitions

### L1 Provider Integration Layer
- `sdkwork-agent-provider-spi/` - Provider integration SPI + adapters

### L2 Transport Layer
- `sdkwork-agent-provider-transport-core/` - Core transport abstractions
- `sdkwork-agent-provider-transport-ipc/` - IPC transport
- `sdkwork-agent-provider-transport-node/` - Node.js SDK binding
- `sdkwork-agent-provider-transport-python/` - Python SDK binding
- `sdkwork-agent-provider-transport-rust/` - Rust native binding

### L3 Implementation Layer
- `agent-providers/crates/sdkwork-agent-provider-*/` - Per-framework implementations:
  - `sdkwork-agent-provider-codex` - Codex Agent Runtime
  - `sdkwork-agent-provider-claude-code` - Claude Code
  - `sdkwork-agent-provider-opencode` - OpenCode
  - `sdkwork-agent-provider-gemini-cli` - Gemini CLI
  - `sdkwork-agent-provider-openclaw` - OpenClaw Agent
  - `sdkwork-agent-provider-hermes` - Hermes Agent Runtime
  - `sdkwork-agent-provider-mimo-code` - MiMo Code
  - `sdkwork-agent-provider-rig` - Rig Agent Framework

### Operational Layer
- `sdkwork-agent-server/` - Operational HTTP server (`/internal/v3/api/intelligence/runtime/*`)
- `sdkwork-agent-client/` - Desktop/mobile bridge
- `sdkwork-agent-database/` - Runtime transient session/message/task state
- `sdkwork-agent-session/` - Session management
- `sdkwork-agent-api-bridge/` - API bridge utilities

### Plugin Layer
- `sdkwork-kernel-plugins/` - Plugin trait + provider-core + platform plugins

### SDK Layer
- `sdks/sdkwork-agent-internal-sdk/` - Generated `@sdkwork/agent-internal-sdk` TypeScript facade for internal runtime HTTP

### Cross-Cutting
- `specs/` - Cross-cutting contracts and schemas
- `external/` - Fixed-revision, read-only third-party source trees used for inspection, mapping, and approved L3 provider public-facade dependencies; never direct kernel-core dependencies

---

## 2. Kernel ↔ Agents Responsibility Boundary

SDKWork follows a Linux-kernel-style split. This repository (`sdkwork-kernel`) owns **SPI definitions, runtime mechanisms, and transient runtime state only**. Business persistence and application surfaces belong to the sibling `sdkwork-agents` repository.

### Kernel Responsibilities

| Concern | Owner | Location |
|---------|-------|----------|
| Agent SPI (core provider families: model, tool, policy, context, memory, knowledge, planning, host, protocol_adapter, mcp, skill, collaboration, telemetry, task_scheduling, agent_classification, message_query, agent_installer, agent_configuration; plus extension families: secret, sandbox, cancellation, rate_limit, model_stream, session_activity, a2a, backend_health, plugin, resilience/retry, kernel hooks, unified stream events) | kernel | `sdkwork-agent-kernel/src/` |
| Provider integration SPI + transports (Rust SDK in-process, Node/Python subprocess, IPC) | kernel | `sdkwork-agent-provider-spi/`, `sdkwork-agent-provider-transport-*/` |
| Per-framework provider implementations (pluggable, open-closed) | kernel | `agent-providers/crates/sdkwork-agent-provider-*/` |
| Runtime transient state (active sessions, streaming buffers, in-flight tasks, SSE cursors) | kernel | `sdkwork-agent-database/` (SessionRepository/MessageRepository/TaskRepository/EventRepository traits + sqlite/postgres/memory impls) |
| Operational HTTP server (`/internal/v3/api/intelligence/runtime/*`) | kernel | `sdkwork-agent-server/` |
| Client bridge (desktop/mobile local + hybrid + remote) | kernel | `sdkwork-agent-client/` |
| Agents-to-Sandbox ID mapping and lifecycle adapter | kernel | `sdkwork-agent-kernel/src/sandbox_runtime.rs` |

### Agents Responsibilities

| Concern | Owner | Location |
|---------|-------|----------|
| Business database (agent catalog, agent configuration profiles, long-term session archive, task history, scheduled job registry) | agents | `../sdkwork-agents/` |
| Agent classification catalog, app-api / backend-api / open-api + SDK generation | agents | `../sdkwork-agents/` |
| Integration of sdkwork-knowledge, sdkwork-drive, sdkwork-skills, sdkwork-prompts, sdkwork-memory | agents | `../sdkwork-agents/` |
| Memory provider implementations (permanent / user / growth-tier backends) | agents | `../sdkwork-agents/` (kernel defines `MemoryProvider` SPI + `MemoryTier`/`MemoryScope` model only) |
| `AgentWorkspace` identity, authorization, logical lifecycle and persistence | agents | `../sdkwork-agents/` |
| `AgentSession` durable business aggregate and opaque `runtimeLocationId` | agents | `../sdkwork-agents/` |

### Boundary Rules

- The kernel **MUST NOT** own business persistence (agent config catalog, long-term archives) or application HTTP surfaces (app-api/backend-api/open-api).
- The agents application **MUST NOT** depend on `sdkwork-agent-provider-*` crates directly; it consumes the kernel via `sdkwork-agent-internal-sdk` and the runtime facade.
- Cross-repository Runtime dependency is `sdkwork-agents -> sdkwork-kernel -> sdkwork-sandbox`. Sandbox **MUST NOT** depend on Kernel or Agents.
- Kernel maps authorized Agents IDs to `SandboxWorkspaceId` and `SandboxSessionId` through `sandbox_runtime::SandboxSessionLifecycleAdapter`; it does not copy Agents models or select a concrete Sandbox Provider.
- The root-exported legacy `SandboxProvider` is a one-shot host-command mechanism. New Runtime lifecycle integration consumes `sdkwork-sandbox` through the namespaced adapter and must not expand the legacy mechanism into a second lifecycle authority.
- `SandboxedExecutionCoordinator` runs an action inside a bound sandbox session lifecycle (`SandboxExecutionBinding` on `AgentExecutionRequest`, get -> start when `auto_start` -> action -> stop when `auto_stop`) through the kernel-side `SandboxedSessionPort`. It is fail-closed: a missing session refuses execution, and lifecycle failures propagate instead of being hidden.
- Layered settings resolution (`AgentSettingsDocument` + `AgentSettingsService`) merges per-scope layers with the agent SDK hierarchy (enterprise > user > project > local) under explicit `AgentSettingSources` selection; the kernel owns the resolution mechanism, not settings persistence.
- Plugins declare contribution points (`PluginContribution` of kind provider/tool/hook/stream/memory/skill) surfaced by `PluginRegistry` discovery; contribution ids must be unique across one registry, mirroring `sdkwork-kernel-plugins` `KernelPluginManifest` provider bindings at the kernel SPI level.

---

## 3. Build, Test, and Development Commands

### Rust Tests

```bash
# Agent kernel contracts
cargo test --manifest-path sdkwork-agent-kernel/Cargo.toml

# Code kernel contracts
cargo test --manifest-path sdkwork-code-kernel/Cargo.toml

# Runtime session/message/task persistence contracts
cargo test --manifest-path sdkwork-agent-database/Cargo.toml

# Operational server HTTP contracts
cargo test --manifest-path sdkwork-agent-server/Cargo.toml
```

### SDK Verification

```bash
pnpm --dir sdks/sdkwork-agent-internal-sdk/sdkwork-agent-internal-sdk-typescript verify
```

### Standards Verification

```bash
# Verify required specs, schemas, crates, and SDK workspace structure
node scripts/check-kernel-standards.mjs

# Run the full kernel audit remediation verification matrix
node scripts/verify-kernel-audit-remediation.mjs
```

### Topology Validation

```bash
# Install root @sdkwork/app-topology workspace dependency
pnpm install

# Validate specs/topology.spec.json against the shared topology schema
pnpm topology:validate

# Verify topology adoption contracts and retired vocabulary
pnpm test:topology
pnpm test:topology-baggage

# Start sdkwork-agent-server with the standalone.development profile and wait for /healthz
pnpm test:topology-smoke
```

### Development Stack

```bash
# Start the default standalone.development stack (agent server only)
pnpm dev

# Run the merge-ready verification aggregate
pnpm verify

# Run kernel standards, SDK workspace, and PNPM script checks
pnpm check
```

---

## 4. Coding Style & Naming Conventions

### Rust

- Use standard `cargo fmt` style
- Modules: `snake_case`
- Types: `PascalCase`
- Explicit contract names: `AgentRuntimeDiagnostics`

### TypeScript

- SDK packages use scoped names such as `@sdkwork/agent-internal-sdk`
- Source exports go through `src/index.ts`

### IDs and Manifests

- Provider and manifest IDs: stable, lowercase, dot-delimited
- Example: `agent.business.created` (represents kernel capabilities or events)

---

## 5. Testing Guidelines

### Contract Tests

- Prefer contract tests that document public behavior
- Rust integration tests: `tests/` directory with names ending in `_contracts.rs`
- SDK workspace checks: Node scripts under `scripts/` and `tools/validators/`

### Test-Driven Development

- When adding behavior, add the failing test first
- Verify the targeted test before broader checks

---

## 6. Commit & Pull Request Guidelines

### Commit Style

- Conventional Commit style
- Example: `feat(agent-kernel): add task scheduling SPI pause/resume contracts`
- Example: `refactor(agent-server): centralize problem error category mapping`
- Use a scoped subject
- Keep it imperative
- Keep unrelated changes out of the same commit

### Pull Requests

- Summarize the contract or subsystem changed
- List verification commands run
- Link relevant specs or issues
- Include screenshots only for visible UI changes

---

## 7. Security & Architecture Notes

### Dependency Restrictions

- Kernel core, provider SPI, provider-neutral transports, server, client, and database crates **MUST NOT** depend on:
  - React
  - Vite
  - Product UI
  - Provider-specific `external/` source trees
- L3 crates under `agent-providers/crates/sdkwork-agent-provider-*` **MAY** consume an `external/` source tree when all of the following hold:
  - the submodule is pinned by gitlink and remains read-only
  - the dependency is declared once through the root native workspace manifest
  - the provider consumes an upstream public facade, client, protocol, or SDK rather than private files, database schemas, caches, logs, or transcripts
  - provider-specific types remain inside L3 and are mapped into Kernel-neutral L0/L1 contracts
  - focused conformance tests and a static architecture guard cover the boundary
- The Codex provider uses `codex-app-server-client` and `codex-app-server-protocol`; direct access to Codex private state SQLite files, tables, PRAGMAs, or rollout files is forbidden.

### UI Architecture

- UI packages must use typed service adapters rather than raw kernel mutations

### Generated Artifacts

- Generated SDK contracts, schemas, and provider manifests should be updated deliberately and reviewed as compatibility surfaces

### HTTP API

- Agent runtime HTTP on `application.public-ingress` uses the SDKWork `internal-api` surface only (`/internal/v3/api/intelligence/runtime/*`)
- Authoritative OpenAPI lives under `apis/internal-api/`
- SDK family materialization runs through:
  1. `node sdks/materialize-agent-internal-api-openapi.mjs`
  2. `node scripts/check-agent-sdk-workspace.mjs`
- Retired application-local prefixes such as `/api/kernel/*` **MUST NOT** be remounted

---

## 8. References

- **SDKWORK Standards**: See `AGENTS.md` for the canonical spec index
- **Application Identity**: See `agent.app.config.json`
- **Local Specs**: See `specs/README.md` and `specs/AGENT_KERNEL_SPEC.md`
- **Documentation Canon**: See `docs/README.md`, `docs/product/prd/PRD.md`, `docs/architecture/tech/TECH_ARCHITECTURE.md`
