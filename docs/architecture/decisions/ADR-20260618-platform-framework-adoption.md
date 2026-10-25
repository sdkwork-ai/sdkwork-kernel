# ADR-20260618-platform-framework-adoption

Status: accepted
Requirement: platform-framework-alignment
Owner: SDKWork kernel maintainers
Date: 2026-06-18
Specs: WEB_FRAMEWORK_SPEC.md, WEB_BACKEND_SPEC.md, DATABASE_SPEC.md, API_SPEC.md, DEPENDENCY_MANAGEMENT_SPEC.md, GITHUB_WORKFLOW_SPEC.md, RPC_SPEC.md, DEPLOYMENT_SPEC.md, ARCHITECTURE_DECISION_SPEC.md, QUALITY_GATE_SPEC.md

## Context

`sdkwork-kernel` is a kernel standards repository, not an SDKWork application root. It aligns with
`SDKWORK_WORKSPACE_SPEC.md` through the hybrid root dictionary model in
`ADR-20260612-sdkwork-kernel-root-dictionary.md` and passes `scripts/check-kernel-standards.mjs`.

Managed-agent business HTTP and persistence moved to `sdkwork-agents` per
`ADR-20260626-agents-application-layer-separation.md`. This repository now owns:

- Operational runtime HTTP (`sdkwork-agent-server`) on `internal-api`
- Runtime session persistence (`sdkwork-agent-database`)
- Internal route crates (`crates/sdkwork-routes-agent-internal-{manifest,api}`)

Platform alignment targets for the kernel:

1. **HTTP runtime** — `sdkwork-agent-server` uses Axum with ingress auth, `SdkWorkApiResponse` /
   `ProblemDetail` envelopes, and route manifest metadata from `sdkwork-routes-agent-internal-api`.
2. **Database runtime** — `sdkwork-agent-database` bootstraps PostgreSQL pools through
   `sdkwork-database-config` and `sdkwork-database-sqlx` when `postgres-sync` is enabled.

**Out of scope:**

- `sdkwork-discovery` — no first-party gRPC/RPC services in kernel (`RPC_SPEC.md`).
- Business app/backend/open HTTP — owned by `sdkwork-agents`.

**Integrated:**

- `sdkwork-utils-rust` — pagination, validation, and list-query helpers in server handlers.
- `agent.app.config.json` — topology/workflow packaging identity (`app.key: sdkwork-kernel`).

## Decision

Adopt platform frameworks for kernel runtime surfaces with executable verification gates.

### Phase 0 — alignment evidence

- Record adoption phases and verification gates in this ADR.
- Index authored OpenAPI under `apis/internal-api/authority-index.json`.
- Declare workspace `Cargo.toml` dependencies on `sdkwork-web-framework` and `sdkwork-database`.
- Enforce evidence through `tools/validators/kernel-standards/platform-integration.mjs`.

### Phase 1 — internal route boundary (complete)

- `crates/sdkwork-routes-agent-internal-manifest` — build-time route manifest from OpenAPI authority.
- `crates/sdkwork-routes-agent-internal-api` — served internal runtime router assembly.

### Phase 2 — runtime HTTP surface (complete)

- `sdkwork-agent-server` mounts `/internal/v3/api/intelligence/runtime/*` only.
- `sdkwork-agent-server/specs/AGENT_SERVER_HTTP_SURFACE.md` documents ingress auth, pagination,
  envelopes, SSE, and observability.

### Phase 3 — `sdkwork-database` integration (complete)

- `sdkwork-agent-database` `postgres-sync` uses `sdkwork-database-config` and `sdkwork-database-sqlx`.
- Runtime PostgreSQL resolves exclusively through the workspace-scoped
  `SDKWORK_DATABASE_*` profile; `agent_runtime` identifies table ownership only.

### Phase 4 — packaging (complete)

- `sdkwork.workflow.json` packages `sdkwork-agent-server`.
- `.github/workflows/package.yml` calls reusable `sdkwork-github-workflow` packaging workflow.

### Phase 5 — `sdkwork-utils` integration (complete)

- List handlers use `sdkwork_utils_rust::validated_offset_list_params`.
- `tools/validators/kernel-standards/platform-utils.mjs` and
  `scripts/dev/sdkwork-kernel-utils-standard.test.mjs` enforce the boundary.

## Alternatives

1. **Big-bang rewrite** — rejected; breaks contract suites and obscures review boundaries.
2. **Permanent local HTTP framework fork** — rejected; violates `WEB_FRAMEWORK_SPEC.md`.
3. **Phased adoption with executable gates** — selected.

## Consequences

Benefits:

- Kernel runtime HTTP and persistence align with platform configuration and observability standards.
- Validators prevent regression after Phase 0.

Costs:

- Cross-repo consumers compose `sdkwork-agents` gateway with `sdkwork-agent-server` operational router.

## Verification

- `node scripts/check-kernel-standards.mjs`
- `node scripts/check-agent-sdk-workspace.mjs`
- `node scripts/verify-kernel-audit-remediation.mjs`
- `cargo test --manifest-path sdkwork-agent-server/Cargo.toml`
- `cargo test --features postgres-sync --manifest-path sdkwork-agent-database/Cargo.toml`

## Supersedes / Superseded By

- Business HTTP route phases that referenced `sdkwork-agent-business` are superseded by
  `ADR-20260626-agents-application-layer-separation.md`.
