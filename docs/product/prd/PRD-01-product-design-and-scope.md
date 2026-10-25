# SDKWork Kernel — Product Design And Scope

Status: active
Owner: SDKWork kernel maintainers
Application: sdkwork-kernel
Updated: 2026-07-28
Parent: [PRD.md](PRD.md)
Specs: [REQUIREMENTS_SPEC.md](../../../../sdkwork-specs/REQUIREMENTS_SPEC.md)

## 1. Product Positioning

SDKWork Kernel is the **shared intelligence mechanism layer** for the SDKWork
product family. It is not an end-user application. It provides:

- Agent runtime semantics (session, model, tool, skill, policy, events).
- Multi-framework provider integration (Codex, Claude Code, Gemini CLI, OpenCode,
  OpenClaw, Hermes, Rig, and future frameworks).
- Operational hosted runtime and internal API for sibling applications.
- Dual-mode runtime coordination for single-machine non-cluster operation and
  multi-node cluster operation through the same agent contracts.
- Code-agent SPI for BirdCoder-class products.
- Client bridge for local/hybrid/remote agent sessions on desktop and mobile hosts.

Products consume kernel capabilities through **`sdkwork-agents`** (HTTP/SDK and
runtime facade), not by depending on `sdkwork-agent-provider-*` crates directly.
Kernel consumes execution-environment lifecycle through `sdkwork-sandbox`; it
does not implement a second Sandbox lifecycle or Workspace registry.

## 2. Target Users

| Persona | Primary need |
| --- | --- |
| Platform engineer | Add a framework once; all products inherit via agents facade |
| Product engineer | Bootstrap engines/runtime without per-vendor wiring |
| Agents application team | Compose kernel server router with business routes |
| SRE / release engineer | Deploy with topology profiles and fail-closed defaults |
| Runtime operator | Manage agent placement, capacity, drain, recovery, and controlled single/cluster transitions |
| Security / compliance | Audit transport health, mock policy, ingress, secrets |

## 3. Product Goals

### 3.1 Mechanism goals

- Single agent object model across all integrated frameworks.
- Negotiated capability routing with explicit transport health and driver resolution.
- Fail-closed when required capabilities cannot be served in production.
- Stable internal runtime API independent of upstream SDK churn.
- Extensible provider onboarding: binding manifest + provider crate + agents facade hook.
- One agent, session, message, task, event, and error model across single and
  cluster coordination modes.
- Unified runtime inventory and capability-aware agent placement across nodes
  and processes when cluster mode is enabled.

### 3.2 Non-functional goals

| Attribute | Requirement |
| --- | --- |
| Cohesion | Kernel aggregates runtime + provider mechanisms; no product business logic |
| Coupling | Products depend on agents application layer, not provider crates |
| Performance | In-process Rust path preferred; subprocess transports only when required |
| Security | Mock/stub blocked in production; ingress auth on hosted runtime |
| Maintainability | One crate per framework; no duplicate plugin/adapter pairs |
| Operability | Topology-driven env; verifiable binding catalog and standards gates |
| Mode parity | Single and cluster modes preserve API and SDK behavior; differences are capability-declared runtime mechanisms |

## 4. In-Scope Capabilities (Kernel-Owned)

| Domain | Deliverables |
| --- | --- |
| Agent SPI | `sdkwork-agent-kernel`, execution loop specs |
| Provider integration | `sdkwork-agent-provider-spi`, transport crates, `agent-providers/crates/*` |
| Binding catalog | `bindings/agent-providers/*/provider-binding.manifest.json` |
| Hosted runtime | `sdkwork-agent-server`, session DB, internal API |
| Runtime coordination | Local single-mode coordination plus cluster node/process inventory, placement, routing, leases, recovery, and drain mechanisms |
| Client bridge | `sdkwork-agent-client`, builtin bridge plugins |
| Code kernel | `sdkwork-code-kernel` |
| Platform plugins | Drive, knowledgebase kernel plugins |
| Sandbox lifecycle adaptation | Agents ID validation/mapping, `SandboxSessionLifecyclePort` invocation and opaque runtime-binding projection |
| Release | `agent.app.config.json`, topology profiles, workflow manifest |

## 5. Out-of-Scope (Sibling Repositories)

| Domain | Owner |
| --- | --- |
| Managed agents CRUD, marketplace | `sdkwork-agents` |
| Open/app/backend agent HTTP + SDK families | `sdkwork-agents` |
| Code-engine runtime facade for products | `sdkwork-agents-runtime-facade` |
| IM messaging, social graph | `sdkwork-im` |
| RTC media runtime | `sdkwork-rtc` |
| Identity, tenant IAM | `sdkwork-iam` |
| `SandboxSession`, Workspace Attachment, Sandbox allocation and Sandbox Provider SPI | `sdkwork-sandbox` |
| Desired agent deployment catalog, replica policy, and configuration profiles | `sdkwork-agents` |
| Multi-region active-active agent execution in the first distributed release | Deferred product scope |

## 6. Core Design Principles

1. **Kernel object models are immutable for vendor fields** — external metadata lives in bindings and diagnostics.
2. **Mechanism vs policy** — kernel provides drivers and transports; agents owns business policy.
3. **Official SDK first** — raw HTTP bypass only when binding declares OpenAPI authority.
4. **Health-aware selection** — transports attach only after `prepare()` succeeds.
5. **Products fail closed** — missing provider capability is an explicit error, not a silent stub.
6. **Open extension** — new framework = manifest + provider crate + conformance tests.

## 7. Key Product Objects

| Object | Description |
| --- | --- |
| `AgentSession` | Existing Kernel execution-session mechanism; it is not the Agents-owned persistent `AgentSession` business aggregate and does not replace `SandboxSession` |
| `SandboxSessionRuntimeProjection` | Kernel view of Sandbox lifecycle state; maps `SandboxRuntimeBindingId` to opaque Agents `runtimeLocationId` without exposing Provider-private metadata |
| `ProviderBinding` | Catalog manifest (`binding.*`) |
| `CapabilityDriver` | Registered handler for `sdk.*` capability ids |
| `TransportHost` | Language/runtime probe (`typescript_node`, `rust_native`, …) |
| `SdkRuntimeRouter` | Routes runtime requests to healthy negotiated transports |
| `KernelEvent` | Telemetry/audit envelope; may project to product events |
| `CodeSession` | Code-kernel workspace/task context for BirdCoder |
| `RuntimeNode` | Observable node identity and capacity used by cluster coordination |
| `RuntimeSlot` | One effective agent runtime instance with version, capability, lifecycle, and health |
| `SessionRuntimeLease` | Fenced ownership that preserves one valid session executor during cluster routing and recovery |

## 8. User Scenarios

Dual-mode runtime scenarios are defined in
[PRD-05-distributed-agent-runtime.md](PRD-05-distributed-agent-runtime.md#9-user-scenarios).

Canonical step-by-step scenarios: [PRD.md §5](PRD.md#5-user-scenarios).

## 9. Evaluation Criteria

Success metrics and release gates: [PRD-03 §4](PRD-03-commercial-readiness-baseline.md#4-success-metrics).
