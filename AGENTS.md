# Repository Guidelines

<!-- SDKWORK-AGENTS-GENERATED: v1 -->

## SDKWORK Soul

Read `../sdkwork-specs/SOUL.md` before executing tasks in this root. Follow specs before memory, dictionary before context, stop on ambiguity, and evidence before completion.

## SDKWORK Standards

Canonical SDKWORK specs path from this root:

- `../sdkwork-specs/README.md`
- `../sdkwork-specs/SOUL.md`
- `../sdkwork-specs/AGENTS_SPEC.md`
- `../sdkwork-specs/CODE_STYLE_SPEC.md`
- `../sdkwork-specs/NAMING_SPEC.md`
- `../sdkwork-specs/PNPM_SCRIPT_SPEC.md`
- `../sdkwork-specs/GITHUB_WORKFLOW_SPEC.md`
- `../sdkwork-specs/SOURCE_CONFIG_SPEC.md`

Do not copy root standard text into this repository. If these relative paths do not resolve, stop and report the broken workspace layout.

## Application Identity

`sdkwork.app.config.json` is present at this repository root (`app.key: sdkwork-kernel`). Topology and workflow metadata are linked through `metadata.topologySpec` and `metadata.workflowManifest`.

## Local Dictionary Structure

- `AGENTS.md`: local agent entrypoint and relative SDKWORK spec index.
- `CLAUDE.md`: Claude Code compatibility shim that points to `AGENTS.md` and must not duplicate rules.
- `GEMINI.md`: Gemini CLI compatibility shim that points to `AGENTS.md` and must not duplicate rules.
- `CODEX.md`: Codex compatibility shim that points to `AGENTS.md` and must not duplicate rules.
- `sdkwork.app.config.json`: application identity, topology, and workflow metadata for this repository root (`app.key: sdkwork-kernel`).
- `.sdkwork/`: reserved local dictionary folder; create only for local skills, plugins, manifests, or AI workspace metadata.
- `specs/`: local application/component contracts and narrowing rules.
- `specs/kernel-local-conventions.md`: repository-specific structure, ownership boundary, command, testing, and security notes moved out of legacy agent guidance.
- `sdks/`: SDK families, OpenAPI authorities, route manifests, and generated SDK artifacts.
- `etc/`: deployable-root source configuration for concrete environment, runtime, topology, and deployment values.
- Local directories to inspect first when relevant: `docs/`, `external/`, `scripts/`, `sdks/`, `agent-providers/`, `sdkwork-kernel-plugins/`, `sdkwork-agent-kernel/`, `sdkwork-agent-database/`, `sdkwork-code-kernel/`, `specs/`.

## Documentation Canon

- [docs/README.md](docs/README.md)
- [docs/product/prd/PRD.md](docs/product/prd/PRD.md)
- [docs/architecture/tech/TECH_ARCHITECTURE.md](docs/architecture/tech/TECH_ARCHITECTURE.md)

## Spec Resolution Order

1. Read this `AGENTS.md` and any nearer component-level `AGENTS.md`.
2. Read `sdkwork.app.config.json` when present.
3. Read local `specs/README.md` and `specs/component.spec.json` when present.
4. Read local `.sdkwork/README.md`, `.sdkwork/skills/`, and `.sdkwork/plugins/` when relevant.
5. Read `../sdkwork-specs/README.md` and the task-specific root specs.
6. Inspect implementation files only after the relevant dictionary entries are clear.

Use dynamic progressive loading: read the nearest `AGENTS.md`, app identity only when relevant, local specs only when relevant, root `sdkwork-specs/README.md`, task-specific specs, then implementation files. Do not eagerly load all language, runtime, UI, deployment, or SDK specs for unrelated work.

## Required Specs By Task Type

- Agent/workflow changes: `../sdkwork-specs/SOUL.md`, `../sdkwork-specs/AGENTS_SPEC.md`, `../sdkwork-specs/SDKWORK_WORKSPACE_SPEC.md`; include `../sdkwork-specs/GITHUB_WORKFLOW_SPEC.md` when GitHub packaging is touched.
- Any code change: `../sdkwork-specs/CODE_STYLE_SPEC.md`, `../sdkwork-specs/NAMING_SPEC.md`, plus only the touched language/framework spec.
- Build scripts, dev runners, dependency preparation, and root command changes: `../sdkwork-specs/CODE_STYLE_SPEC.md`, `../sdkwork-specs/TYPESCRIPT_CODE_SPEC.md`, and `../sdkwork-specs/PNPM_SCRIPT_SPEC.md`.
- Release packaging workflow changes: `../sdkwork-specs/GITHUB_WORKFLOW_SPEC.md`, `../sdkwork-specs/APP_MANIFEST_SPEC.md`, `../sdkwork-specs/RELEASE_SPEC.md`, and `../sdkwork-specs/SUPPLY_CHAIN_SECURITY_SPEC.md`.
- Rust code: `../sdkwork-specs/RUST_CODE_SPEC.md` and `../sdkwork-specs/RUST_RPC_SPEC.md` when RPC is touched.
- Java/Spring code: `../sdkwork-specs/JAVA_CODE_SPEC.md` and `../sdkwork-specs/WEB_BACKEND_SPEC.md` when HTTP backend behavior is touched.
- TypeScript/Node code: `../sdkwork-specs/TYPESCRIPT_CODE_SPEC.md`.
- Frontend/UI code: `../sdkwork-specs/FRONTEND_CODE_SPEC.md`, `../sdkwork-specs/FRONTEND_SPEC.md`, `../sdkwork-specs/UI_ARCHITECTURE_SPEC.md`, and exactly one detailed UI architecture spec.
- Source configuration or `etc/` changes: `../sdkwork-specs/SOURCE_CONFIG_SPEC.md`, plus the task-matrix dependencies in `../sdkwork-specs/README.md`.
- API, SDK, database, runtime, security, and deployment changes must follow the task matrix in `../sdkwork-specs/README.md`.

Language-specific specs are on-demand; do not load Rust, Java, TypeScript, and frontend specs for unrelated tasks.

## Int64 Wire Contract (API_SPEC §13.6)

- OpenAPI `int64` fields and parameters `MUST` be `type: string`, `format: int64`,
  a decimal `pattern` such as `^-?[0-9]+$`, and `x-sdkwork-int64-string: true`.
  `type: integer, format: int64` is a contract violation: generated TypeScript
  SDKs then emit `number`, and browsers silently round ids past
  `Number.MAX_SAFE_INTEGER` (2^53), replaying wrong ids into lookups.
- Rust response DTOs `MUST` serialize `i64` wire fields with
  `#[serde(with = "sdkwork_utils_rust::serde_int64")]` (or `::option`); request
  boundaries parse inbound strings with the same helper.
- Generated TypeScript SDKs keep `int64` as `string`; frontend code `MUST NOT`
  convert ids/snowflake ids/sequence ids to `number` for storage, comparison,
  or submission.
- Verification: `node <sdkwork-specs>/tools/check-api-operation-patterns.mjs --workspace .`

## Code Style Rules

Read `../sdkwork-specs/CODE_STYLE_SPEC.md` and `../sdkwork-specs/NAMING_SPEC.md` before code changes.

Load language specs only when touched: Rust uses `RUST_CODE_SPEC.md`, Java/Spring uses `JAVA_CODE_SPEC.md`, TypeScript/Node uses `TYPESCRIPT_CODE_SPEC.md`, and frontend/UI uses `FRONTEND_CODE_SPEC.md`.


Build scripts, dev runners, and `pnpm clean` must follow `CODE_STYLE_SPEC.md` §7 (Build Source Integrity And Self-Healing). Git-tracked build-critical source files must be verified before builds and self-healed from git when missing; `clean` must not delete them.

## Build, Test, and Verification

No standard build manifest was detected at this root. Read `README.md`, local `specs/`, and parent repository guidance before choosing commands. Record any manual verification in the task result.

## Agent Execution Rules

Use the convention dictionary instead of broad context loading. Do not hand-edit generated SDK output unless the task is explicitly about generated artifacts and the source contract is verified. Do not replace generated SDK integration with raw HTTP. Keep changes scoped to the owning module, package, crate, or app root. Record the exact verification commands and important outputs before reporting completion.

## Task-Specific Standards

- App SDK consumer import work loads `../sdkwork-specs/APP_SDK_INTEGRATION_SPEC.md` and runs `node ../sdkwork-specs/tools/check-app-sdk-consumer-imports.mjs --workspace .`.
- HTTP API contract work loads `../sdkwork-specs/API_SPEC.md` and runs `node ../sdkwork-specs/tools/check-api-operation-patterns.mjs --workspace .` plus `node ../sdkwork-specs/tools/check-api-response-envelope.mjs --workspace .`.
- List/search work loads `../sdkwork-specs/PAGINATION_SPEC.md` and runs `node ../sdkwork-specs/tools/check-pagination.mjs --workspace .`.
- Source configuration work loads `../sdkwork-specs/SOURCE_CONFIG_SPEC.md`; concrete deployable-root values remain owned by `etc/`.

## Human Review Rules

Request human review before breaking SDKWORK standards, changing public naming, altering security/auth behavior, changing database migrations or production deployment config, deleting data/files, or changing generated SDK ownership. Surface unresolved spec paths, app identity conflicts, component ownership conflicts, and API authority ambiguity instead of guessing.

## Repository Local Conventions

Durable repository-specific guidance lives in [`specs/kernel-local-conventions.md`](specs/kernel-local-conventions.md). The remaining notes below are local context only; if they conflict with `../sdkwork-specs/` or machine contracts under `specs/`, the SDKWORK standards and local specs win.

### Project Structure & Module Organization

This repository defines the SDKWork kernel standard for agent and code-agent systems. Rust crates live at `sdkwork-agent-kernel/` (L0 SPI), `sdkwork-code-kernel/` (code-agent SPI), `sdkwork-agent-provider-spi/` (L1 provider integration), `sdkwork-agent-provider-transport-*/` (L2 transports), `agent-providers/crates/sdkwork-agent-provider-*/` (L3 per-framework implementations: codex, claude-code, opencode, gemini-cli, openclaw, hermes, mimo-code, rig), `sdkwork-agent-server/` (operational HTTP server), `sdkwork-agent-client/` (desktop/mobile bridge), `sdkwork-agent-database/` (runtime transient session/message/task state), `sdkwork-agent-session/`, `sdkwork-agent-streaming/`, `sdkwork-agent-api-bridge/`, and `sdkwork-kernel-plugins/` (plugin trait + provider-core + platform plugins). TypeScript integration ships through `sdks/sdkwork-agent-internal-sdk/` (`@sdkwork/agent-internal-sdk`). Cross-cutting contracts and schemas are in root `specs/`. Third-party source trees under `external/` are fixed-revision, read-only inputs. L3 provider crates may consume an approved upstream public facade through root workspace dependencies; provider-neutral kernel core and SPI crates must not depend on those trees or expose their types.

### Kernel ↔ Agents Responsibility Boundary

SDKWork follows a Linux-kernel-style split. This repository (`sdkwork-kernel`) owns **SPI definitions, runtime mechanisms, and transient runtime state only**. Business persistence and application surfaces belong to the sibling `sdkwork-agents` repository.

| Concern | Owner | Location |
| --- | --- | --- |
| Agent SPI (18 core provider families: model, tool, policy, context, memory, knowledge, planning, host, protocol_adapter, mcp, skill, collaboration, telemetry, task_scheduling, agent_classification, message_query, agent_installer, agent_configuration; extension SPI: secret, sandbox, cancellation, rate_limit, model_stream, session_activity, a2a, backend_health, plugin, resilience/retry, kernel hooks, unified stream events) | kernel | `sdkwork-agent-kernel/src/` |
| Provider integration SPI + transports (Rust SDK in-process, Node/Python subprocess, IPC) | kernel | `sdkwork-agent-provider-spi/`, `sdkwork-agent-provider-transport-*/` |
| Per-framework provider implementations (pluggable, open-closed) | kernel | `agent-providers/crates/sdkwork-agent-provider-*/` |
| Runtime transient state (active sessions, streaming buffers, in-flight tasks, SSE cursors) | kernel | `sdkwork-agent-database/` (SessionRepository/MessageRepository/TaskRepository/EventRepository traits + sqlite/postgres/memory impls) |
| Operational HTTP server (`/internal/v3/api/intelligence/runtime/*`) | kernel | `sdkwork-agent-server/` |
| Client bridge (desktop/mobile local + hybrid + remote) | kernel | `sdkwork-agent-client/` |
| Business database (agent catalog, agent configuration profiles, long-term session archive, task history, scheduled job registry) | agents | `../sdkwork-agents/` |
| Agent classification catalog, app-api / backend-api / open-api + SDK generation | agents | `../sdkwork-agents/` |
| Integration of sdkwork-knowledge, sdkwork-drive, sdkwork-skills, sdkwork-prompts, sdkwork-memory | agents | `../sdkwork-agents/` |
| Memory provider implementations (permanent / user / growth-tier backends) | agents | `../sdkwork-agents/` (kernel defines `MemoryProvider` SPI + `MemoryTier`/`MemoryScope` model only) |

The kernel MUST NOT own business persistence (agent config catalog, long-term archives) or application HTTP surfaces (app-api/backend-api/open-api). The agents application MUST NOT depend on `sdkwork-agent-provider-*` crates directly; it consumes the kernel via `sdkwork-agent-internal-sdk` and the runtime facade.

### Build, Test, and Development Commands

- `cargo test --manifest-path sdkwork-agent-kernel/Cargo.toml`: run agent-kernel Rust contracts.
- `cargo test --manifest-path sdkwork-code-kernel/Cargo.toml`: run code-kernel Rust contracts.
- `cargo test --manifest-path sdkwork-agent-database/Cargo.toml`: run runtime session/message/task persistence contracts.
- `cargo test --manifest-path sdkwork-agent-server/Cargo.toml`: run operational server HTTP contracts.
- `node scripts/check-kernel-standards.mjs`: verify required specs, schemas, crates, and SDK workspace structure.
- `node scripts/verify-kernel-audit-remediation.mjs`: run the full kernel audit remediation verification matrix.
- `pnpm --dir sdks/sdkwork-agent-internal-sdk/sdkwork-agent-internal-sdk-typescript verify`: verify the agent internal SDK family.
- `pnpm install`: install root `@sdkwork/app-topology` workspace dependency.
- `pnpm topology:validate`: validate `specs/topology.spec.json` against the shared topology schema.
- `pnpm test:topology` / `pnpm test:topology-baggage`: verify topology adoption contracts and retired vocabulary.
- `pnpm test:topology-smoke`: start `sdkwork-agent-server` with the `standalone.development` profile and wait for `/healthz`.
- `pnpm dev`: start the default `standalone.development` stack (agent server only).
- `pnpm verify`: run the merge-ready verification aggregate.
- `pnpm check`: run kernel standards, SDK workspace, and PNPM script checks.

### Coding Style & Naming Conventions

Rust uses standard `cargo fmt` style, snake_case modules, PascalCase types, and explicit contract names such as `AgentRuntimeDiagnostics`. TypeScript SDK packages use scoped names like `@sdkwork/agent-internal-sdk`; source exports should go through `src/index.ts`.

### Testing Guidelines

Prefer contract tests that document public behavior. Rust integration tests belong in each crate's `tests/` directory and typically use names ending in `_contracts.rs`. SDK workspace checks are Node scripts. When adding behavior, add the failing test first and verify the targeted test before broader checks.

### Commit & Pull Request Guidelines

History uses Conventional Commit style, for example `feat(agent-kernel): add task scheduling SPI pause/resume contracts` and `refactor(agent-server): centralize problem error category mapping`. Use a scoped subject, keep it imperative, and keep unrelated changes out of the same commit. Pull requests should summarize the contract or subsystem changed, list verification commands run, link relevant specs or issues, and include screenshots only for visible UI changes.

### Security & Architecture Notes

Kernel core, provider SPI, provider-neutral transports, server, client, and database crates must not depend on React, Vite, product UI, or provider-specific `external/` source trees. An L3 crate under `agent-providers/crates/sdkwork-agent-provider-*` may depend on a fixed, read-only external source tree only through its documented public facade and root workspace dependency declaration. It must not modify the submodule, query private provider persistence, or leak provider-specific types into L0/L1 contracts. Product applications must use typed SDK clients rather than raw kernel HTTP mutations. Generated SDK contracts, schemas, and provider manifests should be updated deliberately and reviewed as compatibility surfaces.

Agent runtime HTTP on `application.public-ingress` uses the SDKWork `internal-api` surface only (`/internal/v3/api/intelligence/runtime/*`). Authoritative OpenAPI lives under `apis/internal-api/`; SDK family materialization runs through `node sdks/materialize-agent-internal-api-openapi.mjs` before `node scripts/check-agent-sdk-workspace.mjs`. Retired application-local prefixes such as `/api/kernel/*` must not be remounted.

<!-- SDKWORK-NAMING-STANDARD: v1 -->
## Rust Naming And Dependency Declaration

Authority: `../sdkwork-specs/NAMING_SPEC.md` section 3.1 and section 3.2.

Two identifier planes exist in every Rust crate and they MUST NOT be mixed: the package plane
(Cargo, filesystem, lock file) uses kebab-case, and the crate plane (lib target, modules, source
imports) uses snake_case.

- `[package].name`, the crate directory, `[features]` keys, and `[[bin]].name` use kebab-case.
- `[lib].name`, module files, module directories, and Rust imports use snake_case.
- A crate whose `[package].name` contains a hyphen SHOULD declare `[lib].name` explicitly
  (default: package name with every `-` replaced by `_`). A shorter lib name is allowed only
  when declared explicitly and used consistently by every consumer.
- Cargo dependency keys, `[workspace.dependencies]` keys, and `Cargo.lock` entries use the
  dependency package name. Use `package = "..."` when an alias is required.
- Every external crate referenced by `src/` MUST be declared in that crate's `[dependencies]`.
  Test-only crates belong in `[dev-dependencies]`; `build.rs` crates belong in
  `[build-dependencies]`.
- Never delete a dependency line, and never demote one from `[dependencies]` to
  `[dev-dependencies]`, while `src/` still imports it. Verify manifest cleanups with the
  command below before committing them.
- Regenerate and commit `Cargo.lock` in the same change as any dependency table edit.

Verification:

```bash
node ../sdkwork-specs/tools/check-rust-crate-naming-standard.mjs --root .
```
<!-- /SDKWORK-NAMING-STANDARD: v1 -->

<!-- SDKWORK-RUST-CODE-STANDARD: v1 -->
## Rust Code Standard

Authority: `../sdkwork-specs/RUST_CODE_SPEC.md` (v2, industry-best baseline); package/crate
naming and dependency declaration are normative in `../sdkwork-specs/NAMING_SPEC.md` section 3.1
and 3.2.

- Crates are responsibility-shaped: service, repository-sqlx, routes, service-host, native-host,
  worker, assembly, gateway. No generic `core`/`common`/`backend`/`runtime` suffixes.
- Errors are typed enums (`thiserror`) implementing `std::error::Error` with a `source` chain.
  `anyhow` only at binary/CLI/test boundaries, never in lib `[dependencies]`.
- No `unsafe` without a `// SAFETY:` comment; crates default to `unsafe_code = "forbid"`.
  No `unwrap`/`expect`/`panic!`/`todo!`/`dbg!` in library code reachable from public API.
- No lock guard held across `.await`; every external await has a timeout; spawned tasks are
  awaited/detached with a documented owner; retries are bounded, jittered, and idempotent.
- Public API is minimal, documented, `#[must_use]` where applicable, and semver-clean. Leaking
  framework types (`sqlx::Row`, axum extractors) through public signatures is forbidden.
- Workspace root declares `[workspace.package]` (edition, rust-version) and `[workspace.lints]`
  (RUST_CODE_SPEC.md section 13 baseline); every member inherits both with
  `edition.workspace = true` and `[lints] workspace = true`.

Verification:

```bash
node ../sdkwork-specs/tools/check-rust-crate-naming-standard.mjs --root .
node ../sdkwork-specs/tools/check-rust-manifest-standard.mjs --root .
# when service/repository/route/gateway dependencies change:
node ../sdkwork-specs/tools/check-rust-backend-composition.mjs --root .
```
<!-- /SDKWORK-RUST-CODE-STANDARD: v1 -->

<!-- SDKWORK-TYPESCRIPT-CODE-STANDARD: v1 -->
## TypeScript Code Standard

Authority: `../sdkwork-specs/TYPESCRIPT_CODE_SPEC.md` (v2, industry-best baseline).

- `tsconfig` runs `strict: true` and the strict family; public APIs are typed and `any`-free.
  `import type` is required for type-only imports (`verbatimModuleSyntax`).
- Errors are typed at package/service boundaries; no empty catches, no swallowed promise
  rejections, no bare `throw new Error('...')` for business failures.
- Async: every promise is settled; external awaits have timeouts; `AbortSignal` accepted for
  cancellable work; bounded concurrency; no unbounded `Promise.all`.
- Public API is minimal, JSDoc-documented, `@deprecated` where applicable, and semver-clean.
- Discriminated unions model closed variant sets; no `as`/`@ts-ignore` bypasses without a guard.
- Node/build runners verify build-critical sources and self-heal from git (CODE_STYLE_SPEC §7);
  `pnpm clean` never deletes git-tracked build-critical files.

Verification:

```bash
pnpm typecheck && pnpm test && pnpm lint
node ../sdkwork-specs/tools/check-application-layering.mjs --root .
```
<!-- /SDKWORK-TYPESCRIPT-CODE-STANDARD: v1 -->

<!-- SDKWORK-PNPM-WORKSPACE-STANDARD: v1 -->
## pnpm Workspace Dependency And Package Import

Authority: `../sdkwork-specs/PNPM_WORKSPACE_DEPENDENCY_SPEC.md` (companion to
`../sdkwork-specs/DEPENDENCY_MANAGEMENT_SPEC.md`).

Sibling SDKWork repositories are consumed through a dual-track model that MUST stay consistent:

- **Local development** (`pnpm dev`, `pnpm build`): pnpm workspace protocol. Each sibling
  package is declared ONCE in this repository root `pnpm-workspace.yaml` `packages:` as a
  `../sdkwork-*` relative path, and consumed with `workspace:*` in `package.json`. Never use
  `file:`/`link:`/git-URL specifiers for SDKWork sibling packages in any environment.
- **CI / release packaging**: git-repository dependency checkout. Every sibling referenced by the
  local workspace MUST have a matching `dependencies[]` entry in `sdkwork.workflow.json` so CI
  clones the sibling into the same `../sdkwork-*` relative layout (`GITHUB_WORKFLOW_SPEC.md`).
  `package.json` is never rewritten for CI.

Import rules for sibling SDKWork packages:

- Import by package name only: `import { X } from "@sdkwork/package-name"`. The specifier MUST
  equal the target package's `package.json` `name` exactly - no shortening, renaming, or alias.
- Forbidden: relative imports that cross a package boundary into another SDKWork repository or
  another workspace package's `src/` (for example `import ... from "../../sdkwork-appbase/.../src/..."`).
- Consume only the public `exports` surface of a package; never deep-import sibling `src/` internals.
- Every non-relative import in a workspace member MUST resolve to that member's own
  `dependencies`/`devDependencies`/`peerDependencies` (import closure).
- Vite aliases MUST NOT rename or redirect `@sdkwork/*` packages, MUST NOT be added to make a
  resolution error pass, and are allowed only for documented bootstrap/SDK-generation entrypoints.
- Fix a resolution failure by correcting the workspace declaration or the package `exports`,
  not by adding an alias.

Verification:

```bash
node ../sdkwork-specs/tools/verify-repo.mjs --root .
node ../sdkwork-specs/tools/check-workspace-member-protocol.mjs --root .
node ../sdkwork-specs/tools/check-dependency-list-completeness.mjs --target <repo-name>
```
<!-- /SDKWORK-PNPM-WORKSPACE-STANDARD: v1 -->

<!-- SDKWORK-SDK-GENERATION-STANDARD: v1 -->
## Generated SDK Output Is Generator-Owned

Authority: `../sdkwork-specs/SDK_SPEC.md` and `../sdkwork-specs/SDK_WORKSPACE_GENERATION_SPEC.md`.

Everything generated under `sdks/` — `generated/server-openapi/` trees, generated language
workspaces, `dist/` build output, generated `sdkwork-sdk.json`, generated
`.sdkwork/sdkwork-generator-*` reports, and standardizer-synced OpenAPI snapshots — is produced by
the canonical SDK generator `../sdkwork-sdk-generator/bin/sdkgen.js` (`@sdkwork/sdk-generator`).

- Do not hand-edit generated SDK files, including type definitions, dist bundles, and generated
  package metadata. Manual edits are overwritten by the next generation run and break
  reproducibility and contract audits.
- When generated or compiled SDK output does not meet a contract or standard, fix the upstream
  source — authored API contract, route manifest, OpenAPI authority, derived `*.sdkgen.*` input,
  generator profile, or `custom/` runtime build scripts — then regenerate through the standard
  generation command. Do not patch generated output in place.
- Remove stale generated files by re-running the family generation command, which owns cleanup of
  disappeared routes and models; do not hand-prune generated trees.
- The only approved handwritten surfaces are `custom/` roots inside generated workspaces and
  authored `composed/` facades outside `generated/server-openapi`.

Verification:

```bash
node ../sdkwork-specs/tools/sync-agent-sdk-generation-standard.mjs --root . --check
```
<!-- /SDKWORK-SDK-GENERATION-STANDARD: v1 -->


## Deployment Standard (bin/)

Per `../sdkwork-specs/MODULE_BIN_SPEC.md`, this module ships the standardized
nine-entrypoint `bin/` family; all build/package/deploy/installer work `MUST`
go through them. See `bin/README.md` for the usage card and
`bin/lib/module.sh` for the delegation wiring (hooks not yet wired to a
canonical repository command fail fast with guidance).

- App types declared: see `SDKWORK_APP_TYPES` in `bin/lib/module.sh`;
  environments: `development`, `test`, `staging`, `demo`, `production`.
- Image reference: `registry.sdkwork.com/apps/<docker-name>:<version>`
  (`DOCKER_SPEC.md` §2.1; no `latest`, no env-suffixed tags).
- Authoritative specs: `MODULE_BIN_SPEC.md`, `DOCKER_SPEC.md`,
  `DEPLOYMENT_SPEC.md`, `OPERATIONS_SPEC.md`.
<!-- /SDKWORK-DEPLOYMENT-STANDARD: scaffolded -->

<!-- SDKWORK-DESTRUCTIVE-OPERATION-STANDARD: v1 -->
## Destructive Operation Safety

Authority: `../sdkwork-specs/DESTRUCTIVE_OPERATION_SPEC.md`.

Deletion must be explicit, enumerated, and reviewable. Deleting by pattern instead of by named
path is forbidden. Wildcards are for read-only commands only.

- `git rm -r`, `git rm` over a directory or pattern, and `git clean -f`/`-fd`/`-fdx` are
  FORBIDDEN. A recursive `git rm` stages many deletions in one index transaction; if the process
  is interrupted (SIGTERM, timeout, sandbox kill, crash) entries are already gone from disk while
  the index is only half-written, which is silent non-atomic mass data loss.
- Delete tracked files with `rm <exact/path>` on each named path, let `git status --short`
  record the `D` entries, then stage only the enumerated paths. Commit the deletion separately
  from functional changes.
- Shell and script deletion by wildcard is FORBIDDEN: `rm -rf`/`rm -r`/`rm -f` with
  `*`/`**`/`?`/`[...]`/brace expansion, `find ... -delete`, `find ... -exec rm`,
  `find ... | xargs rm`, `for f in *; do rm ...`, `del /S /Q`, `rd /S /Q`,
  `Remove-Item -Recurse -Force` on a glob, `shutil.rmtree`, `fs.rm(dir, { recursive: true })`,
  and `rimraf` over a glob.
- A deletion MUST NOT be combined in one shell invocation with a build, install, network, or
  publish step, and MUST NOT derive its targets from an unvalidated argument, environment
  variable, or configuration value.
- Permitted narrow deletion: `rm <exact/path>`; a short literal path list owned by the tool that
  declares it; the module's own generated artifacts through its owning tool
  (`pnpm clean`, `cargo clean`) per `CODE_STYLE_SPEC.md` §7; and
  `git restore --worktree --source=HEAD -- <exact paths>`.
- Required sequence before any deletion: enumerate exact paths; confirm every path resolves inside
  the active repository or module root; classify tracked/generated/cached/unknown; prefer `rm`
  plus tracked `git status`; delete in batches of 20 or fewer with a status check between
  batches; report the removed paths and the authorizing decision.
- Request explicit human confirmation before deleting any git-tracked path, any directory tree,
  any path resolving outside the active repository root, or more than 20 paths.
- Recovery after an accidental mass deletion: clear a stale `.git/index.lock`, write the path
  list to a file INSIDE the repository (never `/tmp` on Windows, where the Git Bash path space
  and the native tool path space disagree), and run a single
  `git restore --worktree --pathspec-from-file=<repo-relative-list>`. Never loop one
  version-control call per path; the same termination cause interrupts the loop part-way.

Verification (from the repository root):

```bash
node ../sdkwork-specs/tools/sync-agent-destructive-operation-standard.mjs --root . --check
```
<!-- /SDKWORK-DESTRUCTIVE-OPERATION-STANDARD: v1 -->
