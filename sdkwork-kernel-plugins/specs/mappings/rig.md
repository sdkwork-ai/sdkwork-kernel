# Rig Mapping

## Source

- Local path: `external/rig` (source reference for upstream inspection)
- Source-tree Rust crate: `external/rig/crates/rig-core` (`rig-core`)
- Upstream: `https://github.com/0xPlaygrounds/rig.git`

## SDKWork Surface

Rig maps first to a complete SDKWork typed plugin:

- `AgentDefinition` with explicit provider bindings
- `ModelProvider`
- `MemoryProvider`
- `KnowledgeProvider`
- `PlanningProvider`
- `PolicyProvider` for deterministic local conformance
- `AgentInstaller`
- `AgentConfigurationProvider`
- `ContextProvider` only where context assembly, ranking, trimming, or
  explanation is implemented

## Initial Registration Mode

`typed-local-provider`

Rig is Rust-native, so it is the first direct SDKWork Rust SPI adapter. The
SDKWork-owned implementation lives in
`agent-providers/crates/sdkwork-agent-provider-rig` and depends on
kernel SPI contracts, not on `sdkwork-agent-kernel` depending on Rig.

The Rig adapter exposes `rig_agent_definition()` as the executable standard
definition. It binds `provider.rig-rust`,
`provider.memory.rig-rust`,
`provider.knowledge.rig-rust`, `provider.planning.rig-rust`,
`provider.policy.rig-standard`, `adapter.rpc.agent-chat`,
`provider.agent.installer.rig-rust`, and
`provider.agent.configuration.rig-rust` by stable provider id.
The policy provider keeps the stable `provider.policy.rig-standard` id for
compatibility, while its provider manifest name is
`rig-local-conformance-policy` to make the current local approval-gate behavior
explicit.

Its `model_selection` uses the Rig model provider as the non-fallback default.
It does not declare a standalone tool provider or MCP tool surface; Rig agent
tool activity remains inside model execution until an independently invocable,
policy-controlled registry adapter exists. Its `memory_strategy` uses `provider.memory.rig-rust` as an
optional default for session and agent memory scopes.
`RigBackendConfig` is the current typed boundary between agent configuration and
model backend construction. It parses `runtime.rig.backend_mode`,
`llm.rig.provider_id`, and the `llm.rig.api_key` secret reference without
accepting raw API keys. `live` mode remains fail-closed until a host injects an
execution adapter. The `rig-core-adapter` feature supplies an official
`rig-core` OpenAI executor; model execution becomes available only after that
executor and the SDKWork `HostProvider` secret resolver are explicitly connected.
The Rust crate exposes this distinction through `RigBackendExecutionStatus`:
`fail_closed` reports the default local safety mode, while `live_pending`
reports that live configuration exists but execution still fails closed. The Rig
model catalog also publishes `sdkwork.backend.mode`,
`sdkwork.backend.execution_state`, and `sdkwork.backend.fail_closed` metadata so
hosts do not mistake configured live credentials for a connected runtime
backend.
`RigBackendConfig::bootstrap_plan()` is the secret-safe handoff point for a
future upstream Rig adapter. In `live` mode it records the selected provider id,
the required `llm.rig.api_key` secret-reference field, and the policy categories
needed for host secret resolution and model invocation, while the plan remains
`live_pending` and fail-closed when no executor is attached. An attached
executor reports `live` and is the only state in which model calls are allowed.
Safe summaries never echo raw secrets or secret reference values.
Rig model providers expose this bootstrap plan through a typed provider method
and mirror only secret-safe summary fields into model catalog metadata, such as
bootstrap state, selected provider id, required secret-reference field names,
policy categories, and safe summary text.

Its RPC chat entrypoint is exposed through `adapter.rpc.agent-chat`, a typed
SDKWork protocol adapter that maps transport-neutral RPC chat requests into the
kernel chat service. The adapter claims `protocol.map`; the Rig model provider
continues to own `model.chat`.

Rig retrieval, RAG, vector-store lookup, and wiki-like lookup map to SDKWork
`KnowledgeProvider`, not to memory or context. The current typed provider is a
SDKWork-owned local knowledge provider that exposes `knowledge.search`,
`knowledge.read`, and `knowledge.list` while keeping retrieval methods
provider-neutral. A user-owned knowledge base can implement the same SPI
directly; the Rig adapter can be used as one backend option rather than the
standard abstraction.

Rig source inspection shows native custom memory support through
`ConversationMemory`, `InMemoryConversationMemory`, `AgentBuilder::memory(...)`,
and optional `rig-memory` policy wrappers. SDKWork maps that surface to
`MemoryProvider` records with scope, owner context, trust level, and redaction
classification. The current typed provider is a SDKWork-owned local memory
provider that preserves these records and provides the adapter point for a
feature-gated live Rig `ConversationMemory` backend.

Rig source inspection also shows vector store and embedding-oriented retrieval
surfaces in `rig-core`. SDKWork maps those surfaces to `KnowledgeProvider`
requests and results. The optional `rig-core-adapter` Cargo feature consumes the
registry `rig-core` crate inside the Rig plugin crate only. Its OpenAI executor
resolves credentials through `HostProvider` and returns SDKWork-owned model
responses, so Rig types do not leak into `sdkwork-agent-kernel`. Non-vector RAG
approaches such as llm-wiki, keyword, structured, graph, or external lookup
also map to `knowledge.search` with the matching retrieval method.

Rig source inspection also shows first-class custom tool support through
`Tool`, `ToolSet`, dynamic tools, and `ToolServer`. The current adapter does not
claim this as a standalone SDKWork capability because no independently
invocable registry bridge is connected.

Rig core does not expose an SDKWork-equivalent first-class Agent Skill SPI.
Skill-like behavior in Rig-based systems should be modeled in SDKWork as a
`provider_family: skill` only when an adapter can discover and invoke stable
skills through tools, pipelines, workflows, or sub-agent compositions. The Rig
adapter therefore does not claim `skill.discover` or `skill.invoke` today.

## Capability Mapping

| Upstream area | SDKWork capability family |
| --- | --- |
| Model abstraction | `model.chat`; `model.tool_call` only after the model catalog and backend return typed tool-call output; `model.streaming` only after the adapter implements `ModelProvider::stream` |
| Tool composition | `agent.tool.*` observations within model execution; standalone `tool.invoke` is not declared |
| Conversation memory | `memory.query`, `memory.write`, `memory.delete`, `memory.export` |
| Retrieval/RAG | `knowledge.search`, `knowledge.read`, `knowledge.list` |
| Agent orchestration | `planning.*` |
| RPC chat adapter | `protocol.map` and `protocol.stream` through `adapter.rpc.agent-chat` |
| Installation | `agent.install`, `agent.uninstall`, `agent.upgrade` |
| Configuration | `agent.configure`, secret-ref validation |
| Context assembly | `context.*` only when ranking, trimming, or explaining selected context frames |
| Skill-like workflows | `skill.*` only through a future SDKWork skill adapter |

## Policy Boundaries

Model invocation with sensitive context, tool invocation, knowledge retrieval
over private documents, memory writes, and external sends must use SDKWork
policy.
The current typed policy provider is a local conformance approval gate: it
allows read-only and ordinary `model.invoke` requests so fail-closed backend
behavior remains observable, but requires approval and audit for tool calls,
sensitive model context, host secret reads, writes, installs, upgrades, and
other side-effectful, destructive, or privileged actions.
Rig adapters must not read secrets directly; they must consume SDKWork secret
references resolved through host providers.

## Event Mapping

Model, tool, memory, knowledge, planning, context, and policy activity should
map to `agent.model.*`, `agent.tool.*`, `agent.memory.*`,
`agent.knowledge.*`, `agent.step.*`, `agent.context.*`, and
`agent.policy.*`.

## Error Mapping

Unknown model maps to `capability_missing`. Provider setup failure maps to
`provider_unavailable`. Invocation failure maps to `provider_error`.

## Conformance

Implemented target: local-runtime profile with typed model, memory,
knowledge, planning, policy, installer, and configuration providers. Live
upstream Rig model execution is available through the feature-gated official
`rig-core` adapter only when deliberately configured. Standalone tool execution
is not declared, and the MCP surface exposes only implemented resources and
prompts. Memory and knowledge are executable through
SDKWork SPI today and are kept provider-neutral so they can be backed by Rig
`ConversationMemory`, Rig vector-store surfaces, or non-vector knowledge-base
implementations without changing kernel contracts.

## Status

Reference source is declared at `external/rig` for upstream inspection, while
the binding's Rust source-tree evidence points to
`external/rig/crates/rig-core`. SDKWork adapter code is implemented as a local plugin with
the latest registry `rig-core` pin `0.47.0` (verified 2026-11-23),
manifests, package lifecycle, configuration, memory SPI, knowledge SPI,
deployment snapshots, diagnostics, and conformance contract tests. Model
execution uses the official `rig-core` adapter when injected; otherwise the
state is reported as `live_pending`, not as `available`. Standalone tool
execution is absent until a stable tool registry adapter exists;
memory, knowledge, planning,
installer, and configuration contracts are executable through SDKWork SPI, while
policy is executable as a local approval gate rather than a production policy
engine.
