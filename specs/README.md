# SDKWork Kernel Specifications

This directory contains the SDKWork Agent and Code Kernel standards. The Agent
Kernel defines the general runtime and provider foundation. The Code Kernel
builds on that foundation with software-engineering-specific SPI for
workspaces, VCS, patches, terminal execution, verification, language
intelligence, review, artifacts, and the code-agent runtime registry.

Runtime connectivity profiles for local dev and deployment live in
[`topology.spec.json`](./topology.spec.json) with env files under
`../etc/topology/`. See [`../docs/architecture/tech/TECH-topology-standard.md`](../docs/architecture/tech/TECH-topology-standard.md).

## Platform Framework Alignment

| Framework | Status | Integration point |
| --- | --- | --- |
| `sdkwork-web-framework` | **Integrated** | Workspace `Cargo.toml` deps; internal route crates under `crates/sdkwork-routes-agent-internal-*`. Managed agents HTTP surfaces are owned by `sdkwork-agents`. Verified by `platform-integration.mjs`. |
| `sdkwork-database` | **Integrated** | Workspace deps (`sdkwork-database-config`, `sdkwork-database-sqlx`); `sdkwork-agent-database` `postgres-sync` bootstraps runtime session pools. Managed-store persistence is owned by `sdkwork-agents`. Verified by `platform-integration.mjs`. |
| `sdkwork-utils` | **Integrated** | Workspace dep `sdkwork-utils-rust`; product shells may consume `@sdkwork/utils`. Verified by `scripts/dev/sdkwork-kernel-utils-standard.test.mjs`. |
| `sdkwork-discovery` | **Deferred** | Kernel has no first-party gRPC/RPC services. Adopt when an RPC surface ships per `RPC_SPEC.md` and `RUST_RPC_SPEC.md`. |
| PNPM script surface | **Integrated** | Root `package.json` exposes `dev`, `build`, `test`, `check`, `verify`, `clean` via `scripts/sdkwork-command.mjs`. Verified by `tools/validators/kernel-standards/platform-pnpm-scripts.mjs`. |

Sibling checkout and release refs are declared in `agent.workflow.json`
(`sdkwork-web-framework`, `sdkwork-database`, `sdkwork-utils`).

## Extension Provider And Orchestration Standard Set

These specs extend the 18 core provider families with production-hardening and
multi-agent primitives. They are normative for commercial deployment profiles.

| Spec | Responsibility |
| --- | --- |
| [`SANDBOX_PROVIDER_SPEC.md`](./SANDBOX_PROVIDER_SPEC.md) | Platform sandbox isolation for host/tool execution (Landlock, seatbelt, restricted token) |
| [`SECRET_PROVIDER_SPEC.md`](./SECRET_PROVIDER_SPEC.md) | Secret resolve, rotation, audit; complements `HostProvider::resolve_secret` |
| [`BACKEND_HEALTH_MONITOR_SPEC.md`](./BACKEND_HEALTH_MONITOR_SPEC.md) | Continuous SDK backend health, degradation, and recovery |
| [`MULTI_AGENT_ORCHESTRATION_SPEC.md`](./MULTI_AGENT_ORCHESTRATION_SPEC.md) | Task graphs, dependency ordering, parallel execution, result aggregation |
| [`A2A_PROTOCOL_ADAPTER_SPEC.md`](./A2A_PROTOCOL_ADAPTER_SPEC.md) | Agent-to-Agent protocol object mapping (`AgentCard`, task, message) |

Extension traits implemented in `sdkwork-agent-kernel` also include
`RateLimitProvider`, `CancellationProvider`, and `ModelStreamProvider` — cataloged
in [`AGENT_KERNEL_SPEC.md`](./AGENT_KERNEL_SPEC.md) §3.4.

Ecosystem and gap tracking (product layer): [`../docs/product/prd/PRD-04-ecosystem-architecture.md`](../docs/product/prd/PRD-04-ecosystem-architecture.md),
[`../docs/architecture/tech/TECH-03-spi-implementation-gap-tracker.md`](../docs/architecture/tech/TECH-03-spi-implementation-gap-tracker.md).

## Cross-Cutting Standard Set

| Spec | Responsibility |
| --- | --- |
| [`SDK_SPEC.md`](./SDK_SPEC.md) | SDK generation source of truth, canonical `sdkwork-sdk-generator` location, generated output boundaries, regeneration contract, and conformance rules |
| [`AGENT_PROVIDER_INTEGRATION_SPEC.md`](./AGENT_PROVIDER_INTEGRATION_SPEC.md) | External agent framework integration SPI, transports, drivers, registry |
| [`AGENT_PROVIDER_BINDING_SPEC.md`](./AGENT_PROVIDER_BINDING_SPEC.md) | Provider binding manifests, integration sources, catalog layout |
| [`AGENT_SDK_SPI_SPEC.md`](./AGENT_SDK_SPI_SPEC.md) | Deprecated alias → `AGENT_PROVIDER_INTEGRATION_SPEC.md` |
| [`AGENT_SDK_BINDING_SPEC.md`](./AGENT_SDK_BINDING_SPEC.md) | Deprecated alias → `AGENT_PROVIDER_BINDING_SPEC.md` |
| [`KERNEL_PLUGIN_SPEC.md`](./KERNEL_PLUGIN_SPEC.md) | Kernel plugin identity, manifests, contribution points, provider and adapter loading, lifecycle, security, dependencies, conformance, distribution, and plugin naming policy |

## Agent Kernel Standard Set

| Spec | Responsibility |
| --- | --- |
| [`AGENT_KERNEL_SPEC.md`](./AGENT_KERNEL_SPEC.md) | Root Agent Kernel object model, architecture principles, lifecycle, provider families, events, errors, compatibility, and conformance overview |
| [`AGENT_MANIFEST_SPEC.md`](./AGENT_MANIFEST_SPEC.md) | Agent manifest, agent card, provider manifest, capability manifest, discovery, negotiation, compatibility, deprecation, and schema validation |
| [`AGENT_INSTALLATION_CONFIGURATION_SPEC.md`](./AGENT_INSTALLATION_CONFIGURATION_SPEC.md) | Agent installer, uninstall, upgrade, configuration sections, login auth, LLM API key secret references, policy categories, events, and conformance |
| [`AGENT_RUNTIME_SPEC.md`](./AGENT_RUNTIME_SPEC.md) | Runtime bootstrap, provider registration, capability negotiation, runtime API, lifecycle, cancellation, timeout, persistence, and resume |
| [`AGENT_MODEL_PROVIDER_SPI_SPEC.md`](./AGENT_MODEL_PROVIDER_SPI_SPEC.md) | Model provider operations, model catalog descriptors, request-level model selection, model request/response, streaming, tool-call output, structured output, safety, usage, cancellation, and errors |
| [`AGENT_MCP_PROVIDER_SPI_SPEC.md`](./AGENT_MCP_PROVIDER_SPI_SPEC.md) | MCP server descriptors, tools, resources, prompts, typed provider registration, policy, health, and conformance |
| [`AGENT_SKILL_PROVIDER_SPI_SPEC.md`](./AGENT_SKILL_PROVIDER_SPI_SPEC.md) | Agent skill discovery, descriptors, invocation, cancellation, policy, health, and conformance |
| [`AGENT_COLLABORATION_SPI_SPEC.md`](./AGENT_COLLABORATION_SPI_SPEC.md) | Agent discovery, handoff, delegation, and multi-agent collaboration SPI |
| [`AGENT_KNOWLEDGE_PROVIDER_SPI_SPEC.md`](./AGENT_KNOWLEDGE_PROVIDER_SPI_SPEC.md) | Provider-neutral knowledge retrieval, non-vector RAG methods, document read/list, provenance, policy, runtime registration, and conformance |
| [`AGENT_TOOL_PROVIDER_SPI_SPEC.md`](./AGENT_TOOL_PROVIDER_SPI_SPEC.md) | Tool discovery, descriptors, schema, authorization, invocation, streaming, cancellation, result normalization, MCP mapping, and conformance |
| [`AGENT_CONTEXT_MEMORY_SPEC.md`](./AGENT_CONTEXT_MEMORY_SPEC.md) | Context frames, trust/provenance, memory records, scope, retention, redaction, delete/export, and privacy-sensitive behavior |
| [`AGENT_PLANNING_EXECUTION_SPEC.md`](./AGENT_PLANNING_EXECUTION_SPEC.md) | Plans, actions, observations, execution loop, approval gates, retry, revision, pause/resume, and conformance |
| [`AGENT_HOST_PROVIDER_SPI_SPEC.md`](./AGENT_HOST_PROVIDER_SPI_SPEC.md) | Filesystem, process, network, secrets, storage, time, environment, executor providers, sandbox policy, and deterministic fakes |
| [`AGENT_PROTOCOL_ADAPTER_SPEC.md`](./AGENT_PROTOCOL_ADAPTER_SPEC.md) | MCP, A2A, HTTP/RPC, IPC, Tauri, WebSocket, kernel UI client adapters, object mapping, auth, streaming, and trace propagation |
| [`AGENT_SECURITY_POLICY_SPEC.md`](./AGENT_SECURITY_POLICY_SPEC.md) | Policy categories, decisions, untrusted context, prompt-injection boundaries, secret handling, sandbox controls, audit, and risk controls |
| [`AGENT_EVENT_TELEMETRY_SPEC.md`](./AGENT_EVENT_TELEMETRY_SPEC.md) | Event envelope, event families, streaming, replay, trace context, metrics, logs, audit export, and redaction |
| [`KERNEL_PRODUCT_PROJECTION_SPEC.md`](./KERNEL_PRODUCT_PROJECTION_SPEC.md) | KernelEvent → BirdCoder `coding_session_event` projection mapping for product surfaces |
| [`AGENT_UI_CONTRACT_SPEC.md`](./AGENT_UI_CONTRACT_SPEC.md) | Typed UI client surface, capability-driven rendering, permission response, event timeline, diagnostics, and TypeScript package boundary |
| [`AGENT_CONFORMANCE_SPEC.md`](./AGENT_CONFORMANCE_SPEC.md) | Conformance profiles, manifest/runtime/provider/adapter/security/UI tests, and reporting |

## Code Kernel Standard Set

| Spec | Responsibility |
| --- | --- |
| [`CODE_KERNEL_SPEC.md`](./CODE_KERNEL_SPEC.md) | Code-agent object model, provider SPI families, typed runtime registry, runtime diagnostics, side-effect-free conformance reports, session/task state, code event helpers, protocol object mapping, knowledge providers, safety assessment, capability metadata, event/artifact expectations, and conformance |

## Machine-Readable Schemas

| Schema | Manifest |
| --- | --- |
| [`schemas/agent-manifest.schema.json`](./schemas/agent-manifest.schema.json) | `AgentManifest` |
| [`schemas/agent-definition.schema.json`](./schemas/agent-definition.schema.json) | `AgentDefinition` |
| [`schemas/agent-package-manifest.schema.json`](./schemas/agent-package-manifest.schema.json) | `AgentPackageManifest` |
| [`schemas/agent-configuration-spec.schema.json`](./schemas/agent-configuration-spec.schema.json) | `AgentConfigurationSpec` |
| [`schemas/agent-configuration-profile.schema.json`](./schemas/agent-configuration-profile.schema.json) | `AgentConfigurationProfile` |
| [`schemas/agent-configuration-migration.schema.json`](./schemas/agent-configuration-migration.schema.json) | `AgentConfigurationUpgradePlan` |
| [`schemas/agent-card.schema.json`](./schemas/agent-card.schema.json) | `AgentCard` |
| [`schemas/provider-manifest.schema.json`](./schemas/provider-manifest.schema.json) | `ProviderManifest` |
| [`schemas/capability-manifest.schema.json`](./schemas/capability-manifest.schema.json) | `CapabilityManifest` |
| [`schemas/agent-runtime-diagnostics.schema.json`](./schemas/agent-runtime-diagnostics.schema.json) | `AgentRuntimeDiagnostics` |
| [`schemas/kernel-conformance-report.schema.json`](./schemas/kernel-conformance-report.schema.json) | `KernelConformanceReport` |
| [`schemas/kernel-plugin-manifest.schema.json`](./schemas/kernel-plugin-manifest.schema.json) | `KernelPluginManifest` |
| [`schemas/agent-sdk-binding.schema.json`](./schemas/agent-sdk-binding.schema.json) | `AgentSdkBindingManifest` |
| [`schemas/code-capability-manifest.schema.json`](./schemas/code-capability-manifest.schema.json) | `CodeKernelCapabilityManifest` |
| [`schemas/code-runtime-diagnostics.schema.json`](./schemas/code-runtime-diagnostics.schema.json) | `CodeKernelRuntimeDiagnostics` |
| [`schemas/code-conformance-report.schema.json`](./schemas/code-conformance-report.schema.json) | `CodeConformanceReport` |

Schemas use JSON Schema Draft 2020-12 and are standard-candidate validation
artifacts. They should be tightened as concrete Rust and TypeScript
implementations mature.

## Reading Order

1. Start with [`../README.md`](../README.md) for the industry-level kernel
   positioning.
2. Read [`../sdkwork-agent-kernel/README.md`](../sdkwork-agent-kernel/README.md)
   for the Agent Kernel implementation boundary.
3. Read [`AGENT_KERNEL_SPEC.md`](./AGENT_KERNEL_SPEC.md).
4. Read [`AGENT_MANIFEST_SPEC.md`](./AGENT_MANIFEST_SPEC.md) before implementing
   agents, providers, adapters, or registries.
5. Read [`AGENT_INSTALLATION_CONFIGURATION_SPEC.md`](./AGENT_INSTALLATION_CONFIGURATION_SPEC.md)
   before implementing agent package installers or agent configuration screens.
6. Read [`CODE_KERNEL_SPEC.md`](./CODE_KERNEL_SPEC.md) before implementing
   code-agent workspace, patch, terminal, verification, language, review, or
   artifact providers.
7. Read the provider, runtime, security, telemetry, UI, and conformance specs
   relevant to the implementation profile.

## Standard Closure Checklist

- [ ] Every provider has a manifest.
- [ ] Every executable agent has an `AgentDefinition` that binds model, tool,
      context, memory, policy, and other SPI provider families by provider id
      when those families are required or selected by default.
- [ ] Every code provider has a typed SPI registration path or is explicitly
      manifest-only.
- [ ] Every installable agent has installer, uninstall, upgrade, and
      configuration contracts.
- [ ] Every runtime can produce a capability manifest.
- [ ] Every runtime can produce diagnostics for typed providers,
      manifest-only providers, provider health, degraded capabilities, and
      missing standard provider families.
- [ ] Every runtime can produce standard `runtime-manifest` and
      `runtime-local` conformance reports from diagnostics.
- [ ] Every runtime that claims model catalog support exposes
      `model.catalog`, `ModelDescriptor`, and request-level `model_id`
      selection through the Model provider SPI.
- [ ] Every runtime that claims MCP support exposes `mcp.tools`,
      `mcp.resources`, or `mcp.prompts` through the MCP provider SPI.
- [ ] Every runtime that claims Agent Skills support exposes `skill.discover`
      and `skill.invoke` through the Agent Skill provider SPI.
- [ ] Every runtime that claims RAG or knowledge retrieval support exposes
      `knowledge.search`, `knowledge.read`, and/or `knowledge.list` through the
      Knowledge provider SPI rather than binding directly to a vector store.
- [ ] Every code runtime can produce diagnostics for typed providers,
      manifest-only providers, health, and missing standard families.
- [ ] Every code runtime can produce deterministic manifest and local-runtime
      conformance reports.
- [ ] Every generic kernel and code manifest/report contract has a
      machine-readable schema.
- [ ] Every plugin compatibility claim is backed by `KernelPluginManifest`,
      declared provider/adapter contributions, and a conformance profile.
- [ ] Every protected action flows through policy.
- [ ] Every side-effectful code operation can build a standard
      `PolicyRequest`.
- [ ] Every long-running operation is event-backed and cancellable where
      supported.
- [ ] Every adapter maps external protocol objects to SDKWork kernel objects.
- [ ] Every code object required by UI/IPC/RPC maps to a shared protocol
      envelope with namespaced metadata.
- [x] Every UI package uses typed clients/service adapters.
- [x] Runtime HTTP uses canonical internal-api paths only (retired `/api/kernel/*` guarded).
- [x] Production deployment reference assets exist under `deployments/` (Docker, Kubernetes, runbook).
- [x] Release supply-chain evidence includes SBOM and checksum validation scripts.
- [ ] Every compatibility claim is backed by a conformance profile.
- [ ] Extension providers (sandbox, secret, backend health) are wired when security profile requires them.
- [ ] A2A adapter conformance passes when `protocol_adapters` includes `a2a`.
- [ ] Orchestration primitives are exercised when collaboration capability is declared.
