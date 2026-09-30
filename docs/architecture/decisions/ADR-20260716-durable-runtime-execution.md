# ADR-20260716-durable-runtime-execution

Status: accepted
Requirement: REQ-2026-0001
Owner: SDKWork kernel maintainers
Date: 2026-07-16
Specs: `AGENT_RUNTIME_SPEC.md`, `AGENT_PLANNING_EXECUTION_SPEC.md`, `API_SPEC.md`, `DATABASE_SPEC.md`, `SECURITY_SPEC.md`, `ARCHITECTURE_DECISION_SPEC.md`

## Context

The internal runtime currently persists a task when
`runtime.sessions.tasks.submit` is called and returns `201`, but no durable
executor claims or runs that task. Task cancellation changes persisted state,
yet cannot interrupt work that was never started. Permission decisions are
persisted as `pending -> allow|deny`, but the permission record contains policy
display metadata only and cannot safely reconstruct, claim, revalidate, or
resume the protected operation.

This is a production-blocking semantic gap. A detached Tokio task or an
in-memory queue would lose accepted work during process exit, cannot coordinate
multiple pods, and would create false completion behavior. The remedy changes
the database, internal HTTP API, generated SDK, security boundary, and runtime
topology, so implementation requires human review before migration or public
contract changes begin.

## Decision

### API and SDK boundary

- Replace the pre-release task mutation with an async command at
  `POST /internal/v3/api/intelligence/runtime/sessions/{sessionId}/tasks`.
- Return HTTP `202` with `SdkWorkAsyncData`: `accepted: true`, the initial
  `runId` as `operationId`, `status: pending`, and a relative `pollUrl` for
  `GET /internal/v3/api/intelligence/runtime/runs/{runId}`.
- Keep task and run identity distinct. A task is the stable user intent; every
  initial execution or retry is a separate run. Retry creates a new run and
  returns a new async operation id.
- Add typed retrieve and command operations required by `AGENT_RUNTIME_SPEC.md`:
  retrieve run, retry task, and pause/resume/cancel run. Commands use standard
  SDKWork envelopes and `ProblemDetail`; no bare DTO or legacy success shape is
  introduced.
- Change the authored OpenAPI first, regenerate route/SDK artifacts through
  their owning tools, and remove the old pre-release submit mutation rather
  than maintaining two ambiguous contracts. No generated file is hand-edited.

### Persistence model

Migration v5 is additive and non-destructive for both SQLite and PostgreSQL.
It introduces these runtime-owned transient records:

| Record | Purpose | Required invariants |
| --- | --- | --- |
| `runs` | One execution attempt for a task | unique `(task_id, attempt)`; explicit state; retry schedule; cancellation request; terminal outcome; lease owner/expiry; monotonic fencing token |
| `steps` | Ordered or dependency-linked execution units | belongs to one run; explicit action kind/state; bounded metadata; provider and descriptor revision; causation and idempotency identity |
| `permission_operations` | Resumable protected operation linked to an existing permission decision | one operation per permission request; run/step/tool-call links; policy and descriptor revisions; expiry; claim/lease/fence; result/error state; encrypted payload reference only |

The existing `tasks` table remains the task identity and user-intent record.
Task, run, step, permission-operation, and event transitions are written in one
database transaction. State transitions use compare-and-set predicates; a
zero-row update is a conflict, not success. Database constraints reject invalid
attempt numbers, negative fencing tokens, and duplicate identities. Claim and
retention indexes are defined from the exact worker and cleanup predicates.

No secret or raw protected tool input is stored in plaintext. Resumable input
uses an approved authenticated-encryption service with an external key id, or
an opaque reference to approved secret storage. The database stores ciphertext
metadata or the opaque reference, a request digest for idempotency, and bounded
sanitized result/error metadata. Logs and events never contain the payload,
credential material, SQL details, or provider internals.

### Claiming, execution, and recovery

- A bounded worker pool shares the existing provider and persistence admission
  limits; worker count, claim batch size, lease duration, retry count, and
  backoff have strict startup bounds.
- PostgreSQL claims ready runs in a short transaction with
  `FOR UPDATE SKIP LOCKED`. SQLite uses one short write transaction with a
  compare-and-set update and never holds its connection mutex during provider
  execution.
- Every successful claim increments a database-owned fencing token. Completion,
  retry, pause, cancellation, and permission transitions require the current
  owner and fencing token, preventing an expired worker from committing stale
  output.
- Leases are renewed before expiry with bounded jitter. Expired non-terminal
  leases become claimable; recovery emits an event and preserves attempt and
  causation history. Process shutdown stops new claims, requests cancellation,
  and performs a bounded drain without marking unfinished work complete.
- Execution follows intake, context collection, plan validation, policy
  evaluation, step execution, observation, and reconciliation. Unsupported or
  absent planning/provider capabilities fail with a typed stable error; there
  is no synthetic fallback.
- Cancellation is durable and idempotent. It is checked before claim, between
  steps, during lease renewal, and propagated to model/tool/host/provider
  cancellation when supported. Timeout and cancellation remain distinct.

### Permission resume

An `allow` decision does not directly execute inside the HTTP request. It makes
the linked permission operation eligible for the bounded worker. Before a
claim is executed, the worker revalidates expiry, tenant/user ownership,
session and run state, current tool descriptor/provider revision, policy
revision, requested scope, and idempotency digest. Any mismatch fails closed
and emits a sanitized audit event.

Permission execution uses `pending -> decided -> claimable -> executing ->
completed|failed|expired|cancelled` operation state, independently from the
human `allow|deny` decision. A deny decision atomically blocks the step/run and
records an audit event. Repeated decisions and claims are idempotent; conflicting
decisions return a standard conflict problem.

### High availability and bounded resources

The database is the durable coordination authority. In-process notification is
only a latency optimization; workers always recover through bounded indexed
claims. Polling uses bounded batches, exponential idle backoff with jitter, and
no unbounded collection. PostgreSQL is the server authority in standalone and
cluster deployments. SQLite provides contract evidence only for isolated
client-local and test fixtures; it is not a server or multi-writer cluster
database.

Fixed-cardinality metrics cover claim latency, ready/running counts, lease
expiry/recovery, retries, terminal outcomes, permission resume outcomes, queue
rejection, and worker saturation. Tenant, session, task, run, step, permission,
and provider ids are forbidden metric labels.

## State Transitions

```text
task: created -> accepted -> planned -> running -> awaiting_permission
      -> paused -> completed | failed | cancelled

run:  created -> planning -> executing -> awaiting_permission -> paused
      -> completed | failed | cancelled

step: created -> ready -> running -> awaiting_permission
      -> completed | failed | skipped | cancelled
```

Only repository methods own transitions. HTTP handlers and workers request a
typed transition and cannot write arbitrary state strings.

## Alternatives

### Detached Tokio tasks or an in-memory channel

Rejected because accepted work disappears on restart, cannot be fenced across
pods, and cannot provide durable retry, recovery, or permission resume.

### Add execution columns only to `tasks`

Rejected because retries must create runs, tasks can contain multiple steps,
and approval resume requires operation-specific security and idempotency data.
One wide task row would couple unrelated lifecycle concerns and erase history.

### Use Redis as the execution source of truth

Rejected for the first implementation because task state and audit events
already require SQL transactions. Redis or a broker may later provide wake-up
and fan-out, but it must not become a second, weakly coordinated state machine.

### Preserve `POST .../tasks` with `201`

Rejected because the operation is named submit and starts long-running AI
execution. `API_SPEC.md` requires async command semantics for work that cannot
complete inside the normal HTTP latency budget. The application is not yet in
production, so retaining an ambiguous compatibility route would create debt.

## Consequences

- Accepted tasks survive restart and can be executed safely by multiple pods.
- SQLite and PostgreSQL share lifecycle contracts while using store-appropriate
  claim mechanics.
- The authored OpenAPI and generated internal SDK change before GA; consumers
  must adopt the async response and run polling surface.
- The migration adds tables and indexes and therefore needs reviewed rollout,
  rollback, backup, contention, and retention evidence.
- Safe permission resume requires an approved encryption/secret-storage
  integration. It must remain fail-closed until that dependency is configured.
- Shared cross-pod event notification is still required for low-latency SSE at
  commercial cluster scale, but it is not required for durable correctness.

## Verification

Implementation is not complete until all of the following pass on the exact
revision:

- SQLite and PostgreSQL migration checksum, upgrade-from-v4, idempotency, and
  schema-drift contracts.
- Repository parity tests for every legal/illegal transition, atomic event
  write, claim race, expired lease, fencing rejection, retry-new-run behavior,
  cancellation, timeout, and permission decision/resume path.
- Multi-worker contention tests proving one active fenced owner per run/step,
  including process interruption between provider completion and commit.
- HTTP/OpenAPI/generated SDK contracts for `202 SdkWorkAsyncData`, run polling,
  command envelopes, numeric problems, auth scope, and idempotency keys.
- Bounded load/soak tests covering queue saturation, slow providers, database
  pool exhaustion, worker restart, SSE consumers, stable memory, and graceful
  shutdown without OOM or deadlock.
- Live disposable PostgreSQL execution of the complete ignored integration
  suite; missing URI fails the commercial gate and is never counted as pass.
- Mandatory SDKWork API response, operation-pattern, pagination, SDK consumer,
  docs, clippy, workspace, and commercial verification commands.

## Review Gate

Human approval is required for the following package before implementation:

- v5 SQLite/PostgreSQL additive schema and migration/rollback plan;
- replacement of the pre-release submit route and generated SDK contract;
- authenticated-encryption or approved secret-reference integration for
  resumable protected input;
- production worker defaults, retry classes, retention, and rollout gates.

Approved by the repository owner on 2026-07-17. The current PRD and architecture
docs report durable multi-step task execution as an implementation P0 and
permission resume as a production-evidence P0 until their remaining
verification requirements pass.

## Supersedes / Superseded By

Supersedes: none.

Superseded by: none.
