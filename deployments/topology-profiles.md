# Kernel Deployment Profiles

Purpose: topology-linked deployment handoff for `sdkwork-kernel` release and operations.

Owner: SDKWork kernel maintainers.

## Profile Matrix

Kernel runtime connectivity is declared in [`../specs/topology.spec.json`](../specs/topology.spec.json) and materialized through env files under [`../etc/topology/`](../etc/topology/).

| Topology profile id | Deployment profile | Environment | Env file |
| --- | --- | --- | --- |
| `standalone.development` | standalone | development | `etc/topology/standalone.development.env` |
| `standalone.production` | standalone | production | `etc/topology/standalone.production.env` |
| `cloud.development` | cloud | development | `etc/topology/cloud.development.env` |
| `cloud.production` | cloud | production | `etc/topology/cloud.production.env` |

## Local Development Entrypoints

Use PNPM standard commands (`PNPM_SCRIPT_SPEC.md`):

```bash
pnpm dev
pnpm dev:server:postgres:standalone
pnpm dev:server:cloud
```

These dispatch through `scripts/sdkwork-command.mjs` into `scripts/kernel-dev.mjs`, which loads the matching topology profile env bundle.

## Packaging And Release

- Workflow config: [`../agent.workflow.json`](../agent.workflow.json)
- GitHub packaging entry: [`.github/workflows/package.yml`](../.github/workflows/package.yml)
- Container reference: [`docker/Dockerfile`](./docker/Dockerfile) and [`docker/docker-compose.cloud.yml`](./docker/docker-compose.cloud.yml)
- Kubernetes reference: [`kubernetes/`](./kubernetes/)
- Production runbook: [`runbooks/production-rollout.md`](./runbooks/production-rollout.md)
- Merge-ready verification: `pnpm verify`

Related specs: `../sdkwork-specs/DEPLOYMENT_SPEC.md`, `../sdkwork-specs/APP_RUNTIME_TOPOLOGY_SPEC.md`, `../sdkwork-specs/GITHUB_WORKFLOW_SPEC.md`.

Verification: `pnpm topology:validate` and `pnpm check:pnpm-script-standard`.
