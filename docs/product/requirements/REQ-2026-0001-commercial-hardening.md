# REQ-2026-0001: Commercial Hardening (P4)

```yaml
id: REQ-2026-0001
title: Complete kernel commercial hardening for P4 release
owner: SDKWork kernel maintainers
status: in-progress
source: platform
problem: Kernel mechanism paths are under active verification, but enterprise rollout is not approved until artifact publishing, strict commercial release verification, live runtime PostgreSQL evidence, Hermes-specific staging gateway proof, managed HA data-plane controls, immutable secrets/images, MiMo Code facade/live SDK proof, and IM agents-only consumption are evidenced on the release revision.
goals:
  - Publish kernel server binaries to artifact registry with checksum/SBOM evidence
  - Keep the default merge pipeline credential-free while making commercial release verification fail closed on missing live dependencies
  - Require `pnpm verify:commercial` to validate live runtime PostgreSQL, staging SDK credentials, and Hermes-specific staging gateway proof before release promotion
  - Require production deployments to use managed HA Postgres and managed HA Redis, not bundled single-node reference manifests
  - Require production deployments to inject secrets externally, use a dedicated metrics credential, apply exact NetworkPolicy egress, and deploy immutable image digests
  - Require a dedicated cursor-signing credential that is isolated from ingress, JWT, and metrics secrets and is rotated through the target secret manager
  - Require target-environment load, memory, cancellation, failover, restore, and graceful-shutdown evidence before commercial promotion
  - Complete MiMo Code agents facade registration and staging live SDK proof
  - Route IM PC agent surfaces exclusively through sdkwork-agents SDK
non_goals:
  - P5 ZeroCloud, gRPC client, dynamic plugins, discovery
  - Product business features in sdkwork-kernel
users:
  - SRE / release engineer
  - Platform engineer
  - IM PC and BirdCoder product teams
acceptance_criteria:
  - Release pipeline builds target-scoped linux-x64 tar.gz and windows-x64 zip server packages, and validates package-scoped checksum/SBOM evidence from agent.app.config.json
  - Release validation rejects legacy crate-scoped `sdkwork-agent-server` evidence that is not tied to manifest package ids
  - Default merge verification remains credential-free, while `pnpm verify:commercial` runs `scripts/verify-kernel-audit-remediation.mjs --commercial-release`
  - Commercial release verification fails closed when `SDKWORK_DATABASE_URL` is missing, staging SDK credentials are not available, or `SDKWORK_KERNEL_STAGING_HERMES_GATEWAY=1` is not set
  - Commercial release verification forces `SDKWORK_KERNEL_STAGING_LIVE_SDK=1` and `SDKWORK_KERNEL_STAGING_REQUIRE_CREDENTIALS=1` for the staging SDK gate
  - Commercial release verification runs `scripts/provider-transport-workers/hermes-gateway-staging.mjs` for the Hermes TUI gateway JSON-RPC proof
  - Release packaging invokes `pnpm verify:commercial` before building target artifacts, so missing live evidence cannot produce a promotable package
  - Tool discovery exposes only registered provider capabilities and every tool invocation passes through kernel policy evaluation before side effects
  - Session `timeoutMs` is range-validated, persisted, and propagated into typed provider requests
  - Synchronous model, tool, message-turn, and SSE provider calls use one bounded blocking admission executor and cannot create unbounded Tokio blocking workers
  - Provider admission exports fixed-series execution/wait capacity, active, waiting, rejection-reason, and acquisition-latency metrics with cancellation-safe lifecycle accounting
  - Provider admission bounds queued requests independently from active executions and fails queue-full or admission-timeout requests with retryable provider-unavailable errors
  - Persistence operations share one bounded blocking admission executor with independent active and wait capacities, admission timeout, fixed-series saturation metrics, and standard service-unavailable rejection semantics
  - SSE event-stream admission is acquired before session persistence lookup, broadcast subscription, and durable replay so rejected streams cannot consume database or replay-memory capacity
  - Provider-capable HTTP paths acquire a typed admission lease before bounded session-history hydration, and hydration cannot be called without that lease
  - Managed Node/Python stdio unary, streaming, and health calls enforce hard process deadlines; timeout kills and reaps the request-scoped child and prevents worker reuse
  - Provider blocking admission is configurable through `SDKWORK_PROVIDER_MAX_CONCURRENCY` with a strict `1..=1024` startup bound
  - SQLite/PostgreSQL permission execution stores atomically create permission/task/run/step/operation/event rows and apply allow, deny, expiry, completion, and failure transitions with lease fencing
  - Permission-required tool calls persist only authenticated AES-256-GCM ciphertext, bind all execution/provider/revision identities as AAD, and crypto-erase payload material on terminal paths
  - Bounded permission workers preserve the original tool-call id, re-evaluate current policy and provider/descriptor/policy revisions, and fail closed on identity, expiry, digest, or authentication mismatch
  - Task cancellation is idempotent, emits one cancellation event, and rejects completed/failed terminal states
  - Task cancellation state change and cancellation event commit atomically in SQLite, PostgreSQL, and memory implementations
  - Session updates cannot overwrite database-owned message counts, owner identity, or creation time from stale rows, and unknown-session updates fail closed
  - Task submission is not considered complete until a durable scheduler/run/step worker, retry/recovery, fencing, and real in-flight cancellation are implemented
  - TypeScript and Python provider transport workers never synthesize successful `tool_invoke`, `skill_invoke`, or unknown-operation results in any profile; production additionally fails closed for unavailable live session/model execution
  - Rust IPC/Node/Python provider transport stubs and injected transports fail closed in production when mock fallback is disabled
  - Production runbooks require managed HA Postgres and managed HA Redis; `deployments/kubernetes/postgres-redis.yaml` is documented as local/staging reference only
  - Production runbooks reject checked-in/default credentials, metrics-token reuse, mutable image tags, broad database egress, and unbounded deployment claims
  - bindings/agent-providers/mimo-code/provider-binding.manifest.json validates and MiMo Code registers in sdkwork-agents-runtime-facade
  - sdkwork-im has zero direct dependency on sdkwork-agent-provider-* crates
non_functional_requirements:
  security: none beyond SECURITY_SPEC.md and production mock policy
  privacy: none beyond root standards
  performance: none beyond root standards
  reliability: commercial release verification uses live runtime PostgreSQL plus Hermes-specific staging gateway proof, production uses managed HA Postgres and managed HA Redis, and artifact rollback, restore/failover, load, memory, and NetworkPolicy evidence is documented per RELEASE_SPEC.md and DEPLOYMENT_SPEC.md
affected_surfaces:
  - api
  - sdk
  - backend
trace:
  specs:
    - REQUIREMENTS_SPEC.md
    - RELEASE_SPEC.md
    - QUALITY_GATE_SPEC.md
    - SUPPLY_CHAIN_SECURITY_SPEC.md
  components:
    - agent-providers/crates/sdkwork-agent-provider-mimo-code
    - sdkwork-agent-server
verification:
  - node scripts/check-agent-provider-bindings.mjs
  - pnpm verify
  - pnpm verify:commercial
  - node --test tests/kernel_deployment_release.test.mjs
  - SDKWORK_PACKAGE_ID=linux-x64-cloud-server-tar-gz node scripts/release/package-kernel-artifact.mjs
  - SDKWORK_PACKAGE_ID=<package-id> node scripts/release/generate-kernel-sbom.mjs
  - SDKWORK_PACKAGE_ID=<package-id> node scripts/release/generate-kernel-checksums.mjs
  - SDKWORK_PACKAGE_ID=<package-id> node scripts/release/validate-release-artifacts.mjs
  - node scripts/provider-transport-workers/generic-ts-sdk-worker.test.mjs
  - node scripts/provider-transport-workers/generic-python-sdk-worker.test.mjs
  - node scripts/provider-transport-workers/hermes-gateway-staging.test.mjs
  - SDKWORK_KERNEL_STAGING_HERMES_GATEWAY=1 node scripts/provider-transport-workers/hermes-gateway-staging.mjs
  - cargo test --manifest-path sdkwork-agent-provider-transport-ipc/Cargo.toml
  - cargo test --manifest-path sdkwork-agent-provider-transport-node/Cargo.toml
  - cargo test --manifest-path sdkwork-agent-provider-transport-python/Cargo.toml
  - cross-repo sdkwork-agents-runtime-facade and sdkwork-im dependency audit
```

Parent PRD: [PRD.md](../prd/PRD.md) · Readiness shard: [PRD-03-commercial-readiness-baseline.md](../prd/PRD-03-commercial-readiness-baseline.md)

## Rollout Items

1. CI publishing of kernel server binaries to artifact registry with signed checksums.
2. Target-scoped package evidence: lifecycle package builds the declared tar.gz/zip package, SBOM/checksum/signing policy live under `dist/release/<package-id>/`, and release validation rejects evidence that is not tied to manifest package ids.
3. Commercial release gate via `pnpm verify:commercial`; it requires `SDKWORK_DATABASE_URL`, `SDKWORK_KERNEL_STAGING_LIVE_SDK=1`, `SDKWORK_KERNEL_STAGING_REQUIRE_CREDENTIALS=1`, and `SDKWORK_KERNEL_STAGING_HERMES_GATEWAY=1`.
4. Staging-backed live SDK invoke gate via `kernel-staging-live-sdk.yml` (`workflow_dispatch`, credential-gated), plus Hermes TUI gateway JSON-RPC proof via `hermes-gateway-staging.mjs`.
5. Production data-plane handoff to managed HA Postgres and managed HA Redis; bundled Kubernetes Postgres/Redis remains local/staging only.
6. Complete MiMo Code agents facade registration and staging live SDK proof.
7. IM PC agent module exclusively via `sdkwork-agents` SDK.
8. Multi-region runtime DB failover runbook (product-owned; kernel documents session recovery SPI only).
9. Implement and prove the durable runtime task worker, distributed session lease/fencing, and cross-node cancellation channel; task persistence alone is not execution evidence.
