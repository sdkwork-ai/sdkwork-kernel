> Migrated from `docs/quality/sdkwork-standards-alignment-20260612.md` on 2026-06-24.
> Owner: SDKWork maintainers

# SDKWork Standards Alignment Quality Gate Evidence

Date: 2026-06-12
Owner: SDKWork kernel maintainers
Status: complete

## 2026-07 UI workspace retirement

In-repo `sdkwork-kernel-ui/` was removed from `sdkwork-kernel`. Product UI shells live in
application repositories and consume runtime HTTP through `@sdkwork/agent-internal-sdk` or
`sdkwork-agent-client`. Verification commands in this document that reference
`check-kernel-ui-architecture.mjs`, `ui-packages.mjs`, or `sdkwork-kernel-ui/` are historical evidence
only.

## Scope

This evidence covers the repository-root SDKWork standards alignment for:

- root dictionary representation under `SDKWORK_WORKSPACE_SPEC.md`.
- component specs and SDK family metadata under `COMPONENT_SPEC.md`, `SDK_SPEC.md`, and
  `SDK_WORKSPACE_GENERATION_SPEC.md`.
- architecture-decision evidence under `ARCHITECTURE_DECISION_SPEC.md`.
- quality gate and test evidence under `QUALITY_GATE_SPEC.md` and `TEST_SPEC.md`.
- generated SDK output boundary enforcement.

Specs: QUALITY_GATE_SPEC.md, ARCHITECTURE_DECISION_SPEC.md, SDKWORK_WORKSPACE_SPEC.md, COMPONENT_SPEC.md, SDK_SPEC.md, SDK_WORKSPACE_GENERATION_SPEC.md, TEST_SPEC.md

## Definition Of Ready

- Task source: user requested continued alignment to the sibling `../sdkwork-specs` standards.
- Repository root: `<workspace-root>/sdkwork-kernel`.
- Application identity: no root `sdkwork.app.config.json`; this is a kernel standards repository,
  not an application root.
- Relevant specs: `SOUL.md`, `SDKWORK_WORKSPACE_SPEC.md`, `COMPONENT_SPEC.md`, `SDK_SPEC.md`,
  `SDK_WORKSPACE_GENERATION_SPEC.md`, `ARCHITECTURE_DECISION_SPEC.md`, `QUALITY_GATE_SPEC.md`,
  `TEST_SPEC.md`, `CODE_STYLE_SPEC.md`, `NAMING_SPEC.md`, and `TYPESCRIPT_CODE_SPEC.md` for Node
  validation scripts.
- Non-goal: no physical migration of mature component roots into `crates/`, `apps/`, or `plugins/`
  during this phase.
- Generated boundary: no generated output was hand-edited.

## Definition Of Done

- Root dictionary is represented by tracked directories and `README.md` placeholders.
- Component specs declare canonical SDKWork metadata required for their owner boundary.
- SDK family component specs declare `contracts.sdkDependencies`, `contracts.dependencyApiExports`,
  and the expected `component.surface`.
- The layout-preservation decision is recorded in
  `docs/architecture/decisions/ADR-20260612-sdkwork-kernel-root-dictionary.md`.
- This quality evidence records commands, outcomes, gaps, and residual risk.
- Repository tests and `scripts/check-kernel-standards.mjs` require both evidence files.

## Verification Commands And Outcomes

TDD RED evidence for this phase:

- `node --test tests\kernel_workspace_structure.test.mjs`
- Outcome: `7 tests pass`, `1 expected fail`.
- Failure reason: the new standards-alignment ADR did not exist yet.

Baseline verification carried into this phase:

- `node --test tests\*.test.mjs`
- Outcome: `7 tests pass` before adding the lifecycle evidence test.
- `node scripts\check-kernel-standards.mjs`
- Outcome: `Kernel standards conformance check passed.`
- `node scripts\check-agent-sdk-workspace.mjs`
- Outcome: `Agent SDK workspace check passed.`

Final verification for this phase:

- `node --test tests\*.test.mjs`
- Outcome: `8 tests pass`, `0 fail`.
- `node scripts\check-kernel-standards.mjs`
- Outcome: `Kernel standards conformance check passed.`
- `node scripts\check-agent-sdk-workspace.mjs`
- Outcome: `Agent SDK workspace check passed.`

Follow-up validator extraction:

- `scripts/check-kernel-standards.mjs` is now a thin command entrypoint.
- Reusable kernel standards validation logic lives in
  `tools/validators/kernel-standards/check-kernel-standards.mjs`.
- Component spec discovery, canonical spec path validation, SDK metadata validation, and SDK surface
  inference now live in `tools/validators/kernel-standards/component-specs.mjs`.
- Root dictionary, `.sdkwork`, stale dictionary scans, and ADR/quality evidence checks now live in
  `tools/validators/kernel-standards/workspace-evidence.mjs`.
- Agent knowledge, memory, RAG/provider SPI, database, SQL, and SDK operation contract checks now
  live in `tools/validators/kernel-standards/agent-knowledge-memory-contracts.mjs`.
- Required kernel specs, schemas, Rust crate files, kernel plugin structure, component identity, and
  code-kernel dependency checks now live in
  `tools/validators/kernel-standards/kernel-contracts.mjs`.
- Kernel UI architecture command validation and UI package manifest/export checks were retired with the
  `sdkwork-kernel-ui` workspace. `ui-packages.mjs` and its wiring in `check-kernel-standards.mjs` are
  historical evidence only.
- The tool README lives at `tools/validators/kernel-standards/README.md` and documents the canonical
  command `node scripts/check-kernel-standards.mjs`.
- `scripts/check-agent-sdk-workspace.mjs` is now a thin command entrypoint.
- Reusable agent SDK workspace validation logic lives in
  `tools/validators/agent-sdk-workspace/check-agent-sdk-workspace.mjs`.
- SDK generator canonical path, deprecated path, SDK_SPEC generator rule, and sdkgen report checks
  now live in `tools/validators/agent-sdk-workspace/sdkgen-standard-checks.mjs`.
- SDK family README, SDK manifest, TypeScript package metadata, generated
  metadata ownership boundary, and component SDK metadata checks now live in
  `tools/validators/agent-sdk-workspace/sdk-family-metadata-checks.mjs`.
- OpenAPI authority and sdkgen input checks, ownership extension validation, forbidden prefix
  scanning, request-id guardrails, and unsupported agent RAG lifecycle operation guards now live in
  `tools/validators/agent-sdk-workspace/openapi-checks.mjs`.
- The agent SDK workspace validator README lives at `tools/validators/agent-sdk-workspace/README.md`
  and documents the canonical command `node scripts/check-agent-sdk-workspace.mjs`.
- Narrow verification after extraction:
  `node --test tests\kernel_workspace_structure.test.mjs` -> `9 tests pass`.
- Narrow verification after component-spec module split:
  `node --test tests\kernel_workspace_structure.test.mjs` -> `10 tests pass`.
- Narrow verification after workspace-evidence module split:
  `node --test tests\kernel_workspace_structure.test.mjs` -> `11 tests pass`.
- Narrow verification after agent knowledge/memory contract module split:
  `node --test tests\kernel_workspace_structure.test.mjs` -> `12 tests pass`.
- Narrow verification after kernel-contract module split:
  `node --test tests\kernel_workspace_structure.test.mjs` -> `13 tests pass`.
- Narrow verification after UI package module split:
  `node --test tests\kernel_workspace_structure.test.mjs` -> `14 tests pass`.
- Narrow verification after agent SDK workspace validator extraction:
  `node --test tests\kernel_workspace_structure.test.mjs` -> `15 tests pass`.
- Narrow verification after sdkgen standard module split:
  `node --test tests\kernel_workspace_structure.test.mjs` -> `16 tests pass`.
- Narrow verification after SDK family metadata module split:
  `node --test tests\kernel_workspace_structure.test.mjs` -> `17 tests pass`.
- Narrow verification after OpenAPI module split:
  `node --test tests\kernel_workspace_structure.test.mjs` -> `18 tests pass`.
- Full verification after extraction:
  `node --test tests\*.test.mjs` -> `9 tests pass`, `0 fail`.
- Full verification after extraction:
  `node scripts\check-kernel-standards.mjs` -> `Kernel standards conformance check passed.`
- Full verification after extraction:
- Full verification after extraction:
  `node scripts\check-agent-sdk-workspace.mjs` -> `Agent SDK workspace check passed.`
- Full verification after workspace-evidence module split:
  `node --test tests\*.test.mjs` -> `11 tests pass`, `0 fail`.
- Full verification after workspace-evidence module split:
  `node scripts\check-kernel-standards.mjs` -> `Kernel standards conformance check passed.`
- Full verification after workspace-evidence module split:
- Full verification after workspace-evidence module split:
  `node scripts\check-agent-sdk-workspace.mjs` -> `Agent SDK workspace check passed.`
- Full verification after agent knowledge/memory contract module split:
  `node --test tests\*.test.mjs` -> `12 tests pass`, `0 fail`.
- Full verification after agent knowledge/memory contract module split:
  `node scripts\check-kernel-standards.mjs` -> `Kernel standards conformance check passed.`
- Full verification after agent knowledge/memory contract module split:
- Full verification after agent knowledge/memory contract module split:
  `node scripts\check-agent-sdk-workspace.mjs` -> `Agent SDK workspace check passed.`
- Full verification after kernel-contract module split:
  `node --test tests\*.test.mjs` -> `13 tests pass`, `0 fail`.
- Full verification after kernel-contract module split:
  `node scripts\check-kernel-standards.mjs` -> `Kernel standards conformance check passed.`
- Full verification after kernel-contract module split:
- Full verification after kernel-contract module split:
  `node scripts\check-agent-sdk-workspace.mjs` -> `Agent SDK workspace check passed.`
- Full verification after UI package module split:
  `node --test tests\*.test.mjs` -> `14 tests pass`, `0 fail`.
- Full verification after UI package module split:
  `node scripts\check-kernel-standards.mjs` -> `Kernel standards conformance check passed.`
- Full verification after UI package module split:
- Full verification after UI package module split:
  `node scripts\check-agent-sdk-workspace.mjs` -> `Agent SDK workspace check passed.`
- Full verification after agent SDK workspace validator extraction:
  `node --test tests\*.test.mjs` -> `15 tests pass`, `0 fail`.
- Full verification after agent SDK workspace validator extraction:
  `node scripts\check-kernel-standards.mjs` -> `Kernel standards conformance check passed.`
- Full verification after agent SDK workspace validator extraction:
- Full verification after agent SDK workspace validator extraction:
  `node scripts\check-agent-sdk-workspace.mjs` -> `Agent SDK workspace check passed.`
- Full verification after sdkgen standard module split:
  `node --test tests\*.test.mjs` -> `16 tests pass`, `0 fail`.
- Full verification after sdkgen standard module split:
  `node scripts\check-kernel-standards.mjs` -> `Kernel standards conformance check passed.`
- Full verification after sdkgen standard module split:
- Full verification after sdkgen standard module split:
  `node scripts\check-agent-sdk-workspace.mjs` -> `Agent SDK workspace check passed.`
- Full verification after SDK family metadata module split:
  `node --test tests\*.test.mjs` -> `17 tests pass`, `0 fail`.
- Full verification after SDK family metadata module split:
  `node scripts\check-kernel-standards.mjs` -> `Kernel standards conformance check passed.`
- Full verification after SDK family metadata module split:
- Full verification after SDK family metadata module split:
  `node scripts\check-agent-sdk-workspace.mjs` -> `Agent SDK workspace check passed.`
- Full verification after OpenAPI module split:
  `node --test tests\*.test.mjs` -> `18 tests pass`, `0 fail`.
- Full verification after OpenAPI module split:
  `node scripts\check-kernel-standards.mjs` -> `Kernel standards conformance check passed.`
- Full verification after OpenAPI module split:
- Full verification after OpenAPI module split:
  `node scripts\check-agent-sdk-workspace.mjs` -> `Agent SDK workspace check passed.`

## Standards Scans

SDK metadata scan:

- 25 component specs were scanned after SDK metadata hardening.
- `issueCount: 0`.
- Authored SDK-family/component contracts use explicit `sdkDependencies` and
  `dependencyApiExports`.

SDK surface scan:

- SDK component specs with required SDK surfaces: 6.
- SDK specs missing required `component.surface`: 0.
- Open/custom SDK families map to `open-api`.
- App SDK families map to `app-api`.
- Backend SDK families map to `backend-admin`.

## Agent Implementation Type Follow-up

> **2026-06 separation:** `sdkwork-agent-business` moved to `sdkwork-agents`. Historical verification
> below records the pre-separation closeout; active kernel gates use internal runtime API only.

The agent implementation type contract is recorded as follow-up architecture and quality evidence
because it changed the managed-agent domain model, explicit database storage, OpenAPI contracts, and
generated SDK surfaces.

Decision and plan evidence:

- `docs/architecture/decisions/ADR-20260612-agent-implementation-type.md`
- `docs/archive/superpowers/plans/2026-06-12-agent-implementation-type.md`
- Agent implementation type evidence is enforced by tools/validators/kernel-standards/workspace-evidence.mjs.

Verification evidence:

- `node --test tests/*.test.mjs` -> exit 0; `19 tests pass`, `0 fail`.
- `cargo fmt --manifest-path sdkwork-agent-business/Cargo.toml` -> exit 0.
- `cargo test --manifest-path sdkwork-agent-business/Cargo.toml` -> exit 0; core and contract
  suites passed with 0 failures.
- `cargo test --features http-axum --manifest-path sdkwork-agent-business/Cargo.toml` -> exit 0;
  HTTP contract suite passed with 0 failures.
- `cargo test --features postgres-sync --manifest-path sdkwork-agent-business/Cargo.toml` -> exit 0;
  persistence contract suite passed with 0 failures.
- `node sdks/materialize-agent-v3-openapi-boundaries.mjs` -> exit 0; OpenAPI boundaries
  materialized.
- `node sdks/workspace-agent-sdkgen.mjs --mode dry-run` -> exit 0; app, backend, and open SDK
  outputs reported `hasChanges: false`.
- `node scripts/check-agent-sdk-workspace.mjs` -> exit 0; agent SDK workspace check passed.
- `node scripts/check-kernel-standards.mjs` -> exit 0; kernel standards conformance check passed.

Generated SDK output remains generator-owned. The authored OpenAPI contracts were updated first, SDK
boundaries were materialized, and TypeScript SDK output was regenerated through the SDK generator.

## Generated TypeScript API surface validator split

- `tools/validators/agent-sdk-workspace/generated-typescript-api-surface-checks.mjs` now owns
  generated TypeScript API surface checks for agent memory and knowledge resources.
- `tools/validators/agent-sdk-workspace/check-agent-sdk-workspace.mjs` now imports
  `validateGeneratedAgentApi` and remains the workspace orchestration gate.
- RED: `node --test tests\kernel_workspace_structure.test.mjs` -> exit 1; 17 tests passed and 3
  failed because the README, quality evidence, and standards evidence did not yet require the new
  focused module.
- GREEN: `node --test tests\kernel_workspace_structure.test.mjs` -> exit 0; 20 tests pass after the
  focused module split.
- Final verification: `node scripts\check-agent-sdk-workspace.mjs` -> exit 0; agent SDK workspace
  check passed.
- Final verification: `node scripts\check-kernel-standards.mjs` -> exit 0; kernel standards
  conformance check passed.
- Final verification: `node --test tests\*.test.mjs` -> exit 0; 20 tests pass and 0 failed.
- Final verification: `node scripts\check-agent-sdk-workspace.mjs` -> exit 0;
  10 UI packages passed architecture checks.
- Final verification: direct whitespace scan over the touched validator, evidence, and test files ->
  exit 0.
- The quality evidence requirement is enforced by
  `tools/validators/kernel-standards/workspace-evidence.mjs`.

## Component Surface Applicability Follow-up

- 19 non-SDK component specs document why component.surface is not required through
  `component.surfaceNotRequiredReason`.
- SDK family and generated SDK package component specs still declare required SDK surfaces:
  `open-api`, `app-api`, and `backend-admin`.
- `tools/validators/kernel-standards/component-specs.mjs` now rejects any non-SDK component spec
  that omits both `component.surface` and a specific `surfaceNotRequiredReason`.
- RED: `node --test tests\kernel_workspace_structure.test.mjs` -> exit 1; the new component spec
  assertion failed first on `sdkwork-agent-business/specs/component.spec.json` because no
  `surfaceNotRequiredReason` existed.
- GREEN: `node --test tests\kernel_workspace_structure.test.mjs` -> exit 0; 20 tests pass after
  the 19 non-SDK component specs and validator rule were updated.
- Final verification: `node scripts\check-kernel-standards.mjs` -> exit 0; kernel standards
  conformance check passed.
- Final verification: `node --test tests\*.test.mjs` -> exit 0; 20 tests pass and 0 failed.
- Final verification: `node scripts\check-agent-sdk-workspace.mjs` -> exit 0; agent SDK workspace
  check passed.
- Final verification: `node scripts\check-agent-sdk-workspace.mjs` -> exit 0;
  10 UI packages passed architecture checks.

## Component Dependency Policy Follow-up

- All component specs explicitly declare contracts.sdkDependencies and contracts.dependencyApiExports.
- Non-SDK components that do not consume dependency SDKs or re-export dependency APIs use explicit
  empty arrays (`[]`) instead of relying on missing fields.
- `tools/validators/kernel-standards/component-specs.mjs` now rejects every component spec that
  omits either dependency policy array.
- RED: `node --test tests\kernel_workspace_structure.test.mjs` -> exit 1; the new dependency policy
  assertion failed first on `sdkwork-agent-business/specs/component.spec.json` because
  `contracts.sdkDependencies` was missing.
- GREEN: component dependency policy gap scan -> exit 0:

```json
{
  "gapCount": 0,
  "gaps": []
}
```

- GREEN: `node --test tests\kernel_workspace_structure.test.mjs` -> exit 0; 20 tests pass after
  explicit empty arrays were added to non-SDK component specs.
- Final verification: `node scripts\check-kernel-standards.mjs` -> exit 0; kernel standards
  conformance check passed.
- Final verification: `node --test tests\*.test.mjs` -> exit 0; 20 tests pass and 0 failed.
- Final verification: `node scripts\check-agent-sdk-workspace.mjs` -> exit 0; agent SDK workspace
  check passed.
- Final verification: `node scripts\check-agent-sdk-workspace.mjs` -> exit 0;
  10 UI packages passed architecture checks.
- Final verification: component dependency policy whitespace scan -> exit 0 across 29 files.

## Component Dependency API Surface Follow-up

- All component specs explicitly declare contracts.dependencyApiSurfaces.
- Components that do not serve, proxy, or require dependency-owned HTTP APIs use an explicit empty
  array (`[]`) instead of relying on a missing field.
- `contracts.dependencyApiSurfaces` entries are object runtime declarations, not legacy strings.
  The kernel-owned internal-api route/server components keep `[]` because the internal-api is their
  own surface, not a dependency-owned surface. `sdkwork-agent-client` records its remote runtime
  dependency as an `external-service` object with `apiAuthority`, `apiPrefix`, base URL source, and
  `coverage: "verified"` evidence from `SseChatClient::runtime_url`.
- `tools/validators/kernel-standards/component-specs.mjs` now rejects every component spec that
  omits `contracts.dependencyApiSurfaces`.
- RED: `node --test tests\kernel_workspace_structure.test.mjs` -> exit 1; the new dependency API
  surface object assertion failed first on
  `crates\sdkwork-routes-agent-internal-api\specs\component.spec.json`.
- GREEN: component dependency API surface gap scan -> exit 0:

```json
{
  "componentSpecCount": 26,
  "gapCount": 0,
  "gaps": []
}
```

- GREEN: `node --test tests\kernel_workspace_structure.test.mjs` -> exit 0; 24 tests pass after
  explicit empty arrays and object runtime surface declarations were aligned.

## Cargo Workspace Dependency Follow-up

- Root `Cargo.toml` owns SDKWork sibling path declarations under `[workspace.dependencies]`.
- Member `Cargo.toml` files consume SDKWork crates through `workspace = true`; features and optional
  flags remain on the member dependency entry when needed.
- This avoids dependency path drift across provider crates, route crates, server/client crates, and
  plugin crates.
- RED: `node --test tests\kernel_workspace_structure.test.mjs` -> exit 1; the member Cargo
  dependency assertion failed first on `sdkwork-agent-api-bridge\Cargo.toml`.
- GREEN: member Cargo dependency scan -> exit 0:

```text
no member sdkwork path dependencies
```

- GREEN: `cargo metadata --format-version 1 --no-deps` -> exit 0.
- GREEN: `cargo fmt --check` -> exit 0.
- GREEN: `node ../sdkwork-specs/tools/verify-repo.mjs --root .` -> exit 0.
- GREEN: `pnpm check` -> exit 0.

## Component SDK Route Manifest Applicability Follow-up

- SDK family root component specs explicitly declare contracts.routeManifest as null.
- This keeps `contracts.routeManifest` explicit for SDK family roots while preserving the
  `COMPONENT_SPEC.md` rule that only Rust HTTP route crate components use route manifest paths.
- `tools/validators/kernel-standards/component-specs.mjs` now rejects SDK family root component specs
  that omit `contracts.routeManifest`, or that set it to a non-null non-string value.
- RED: `node --test tests\kernel_workspace_structure.test.mjs` -> exit 1; the SDK family route
  manifest assertion failed first on `sdks\sdkwork-agent-app-sdk\specs\component.spec.json`.
- GREEN: component contract field gap scan -> exit 0:

```json
{
  "componentSpecCount": 25,
  "gapCount": 0,
  "gaps": []
}
```

- GREEN: `node --test tests\kernel_workspace_structure.test.mjs` -> exit 0; 20 tests pass after
  `contracts.routeManifest: null` was added to the three SDK family root component specs.
- Final verification: `node scripts\check-kernel-standards.mjs` -> exit 0; kernel standards
  conformance check passed.
- Final verification: `node --test tests\*.test.mjs` -> exit 0; 20 tests pass and 0 failed.
- Final verification: `node scripts\check-agent-sdk-workspace.mjs` -> exit 0; agent SDK workspace
  check passed.
- Final verification: `node scripts\check-agent-sdk-workspace.mjs` -> exit 0;
  10 UI packages passed architecture checks.

## Generated Output Boundary

No generated output was hand-edited.

No manual edits were made under generated SDK transport output such as `generated/server-openapi`.
SDK metadata remains in family-level and component-level metadata files, not in generated
`sdkwork-sdk.json`, generated `package.json`, generated `sdk-manifest.json`, generator reports, or
generated source metadata blocks.

## Residual Risks

- The worktree contains pre-existing and earlier-phase changes. No destructive cleanup or unrelated
  revert was performed.

## Kernel Audit Remediation Follow-up (2026-06-17)

Scope: close audit findings from the SDKWork kernel application system review against
`sdkwork-specs` (security, contracts, standards coverage, UI services, optimistic concurrency).

### Security and trust boundary (P0)

- Tenant reconciliation: `reconcile_resource_tenant_with_subject_header()` enforces matching
  resource `tenant_id` and subject tenant headers before policy extraction.
- Trust boundary spec: `sdkwork-agent-business/specs/AGENT_BUSINESS_HTTP_TRUST_BOUNDARY.md`.
- Trusted request context: `AgentRequestContext::from_gateway_subject_headers()` and
  `RequestScope::from_trusted_extension()`; legacy header path delegates to the trusted builder.
- HTTP negative/positive contract tests in `http_axum_contracts.rs` (tenant mismatch 403, match 200).

### Functional and concurrency (P1)

- PostgreSQL memory get paths implemented; `postgres-sync` contract tests in
  `tests/agent_postgres_sync_contracts.rs`.
- `AgentBusinessIdGenerator`: removed panicking `Default`; explicit `new_default()`.
- Optimistic concurrency: `ensure_expected_version()` requires `expectedVersion` on mutations;
  HTTP and service contract tests updated (including `agent_memory_contracts.rs`).
- HTTP service lock: `postgres-sync` uses `std::sync::Mutex` with `tokio::task::spawn_blocking`
  in `with_service_mut`; default `http-axum` path uses `tokio::sync::Mutex`.

### Standards coverage (P1)

- Six agent runtime crates received `specs/component.spec.json` (and README where missing):
  `sdkwork-agent-api-bridge`, `sdkwork-agent-client`, `sdkwork-agent-database`,
  `sdkwork-agent-server`, `sdkwork-agent-session`.
- Eight adapter crates under `sdkwork-kernel-plugins/crates/` have component specs, READMEs, and `AGENTS.md`.
- `tools/validators/kernel-standards/kernel-contracts.mjs` requires the six runtime crates.

### Kernel UI (P1) — retired 2026-07

In-repo `sdkwork-kernel-ui/` was removed. Runtime HTTP contracts remain on `internal-api`;
product applications own UI implementation and consume `@sdkwork/agent-internal-sdk`.
Historical implementation notes (auth provider surface, session bootstrap panel, i18n baseline,
contract tests) applied to the retired workspace only.

### Managed agent business (P1) — retired 2026-06

In-repo `sdkwork-agent-business/` and business API/SDK families were removed per
`ADR-20260626-agents-application-layer-separation.md`. Managed-agent CRUD, catalog persistence,
and app/backend/open HTTP surfaces now live in `sdkwork-agents`. Kernel validators index
`apis/internal-api/authority-index.json` only.

### Verification commands and outcomes (2026-06-17)

- `node scripts/check-kernel-standards.mjs` -> exit 0; kernel standards conformance check passed.
- `node scripts/check-agent-sdk-workspace.mjs` -> exit 0; agent SDK workspace check passed.
- `node --test tests/*.test.mjs` -> exit 0; 20 tests pass.
- `cargo test --manifest-path sdkwork-agent-server/Cargo.toml` -> exit 0; runtime HTTP contract suites pass.
- `cargo test --doc --manifest-path sdkwork-agent-kernel/Cargo.toml` -> exit 0; 2 doctests pass.
- `cargo test --manifest-path sdkwork-agent-kernel/Cargo.toml` -> exit 0.
- `cargo test --manifest-path sdkwork-code-kernel/Cargo.toml` -> exit 0.
- `node scripts/verify-kernel-audit-remediation.mjs` -> exit 0; full audit verification matrix passes.
- `.github/workflows/kernel-verification.yml` -> audit remediation job + PostgreSQL live contract job.

### Audit remediation status (2026-06-17 closeout)

All P0/P1 audit items tracked in this document are implemented and verified locally.
CI runs the audit matrix on every push/PR to `main` and executes the live PostgreSQL memory
contract against a service container. Live Postgres memory relation inserts bind `TIMESTAMP`
columns with `time::PrimitiveDateTime` (not `OffsetDateTime`, which only accepts
`TIMESTAMPTZ`) and `REAL` scores with native `f32`. Product shells consume runtime HTTP through
`@sdkwork/agent-internal-sdk` and `sdkwork-agent-client`.

Enterprise IdP OAuth redirect flows remain future product work outside this audit scope.

## Platform Framework Adoption Follow-up (2026-06-18)

Scope: kernel runtime alignment against `WEB_FRAMEWORK_SPEC.md` and `DATABASE_SPEC.md`.

Decision and evidence:

- `docs/architecture/decisions/ADR-20260618-platform-framework-adoption.md`
- `docs/architecture/decisions/ADR-20260626-agents-application-layer-separation.md`
- `apis/internal-api/authority-index.json`
- Workspace `Cargo.toml` declares `sdkwork-web-*` and `sdkwork-database-*` dependencies
- `tools/validators/kernel-standards/platform-integration.mjs` enforces Phase 0 evidence

Out of kernel scope (documented, not implemented in this repository):

- `sdkwork-discovery` — no first-party gRPC/RPC services in kernel
- `sdkwork.app.config.json` — kernel is a standards repository, not an application root

Phase 5 utils:

- `sdkwork-utils-rust` workspace dependency; runtime session/message/task lists now use bounded cursor-only keyset pagination, superseding the historical `validated_offset_list_params` path
- `sdkwork-agent-database` postgres pool bootstrap through `sdkwork-database-sqlx`
- `tools/validators/kernel-standards/platform-utils.mjs` and `scripts/dev/sdkwork-kernel-utils-standard.test.mjs`
- `pnpm test:utils-standard` in root `package.json`

Phase 4 packaging:

- `sdkwork.workflow.json` and `.github/workflows/package.yml` for `sdkwork-agent-server` release artifacts
- `deployments/` topology-linked profiles remain future work for production rollout

Verification evidence (current):

- `node scripts/check-kernel-standards.mjs` -> exit 0
- `node scripts/verify-kernel-audit-remediation.mjs` -> exit 0
- `cargo test --manifest-path sdkwork-agent-server/Cargo.toml` -> exit 0
