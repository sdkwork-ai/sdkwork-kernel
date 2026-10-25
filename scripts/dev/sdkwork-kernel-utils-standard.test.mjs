#!/usr/bin/env node
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');

function read(relativePath) {
  return fs.readFileSync(path.join(repoRoot, relativePath), 'utf8');
}

const rootCargo = read('Cargo.toml');
assert.match(
  rootCargo,
  /sdkwork-utils-rust\s*=\s*\{[^}]*sdkwork-utils/u,
  'Cargo.toml must declare sdkwork-utils-rust workspace dependency'
);

const agentDatabaseCargo = read('sdkwork-agent-database/Cargo.toml');
assert.match(
  agentDatabaseCargo,
  /sdkwork-utils-rust/u,
  'sdkwork-agent-database must declare sdkwork-utils-rust for postgres-sync alignment'
);

const workflow = JSON.parse(read('agent.workflow.json'));
const dependencyIds = new Set((workflow.dependencies || []).map((dependency) => dependency.id));
assert(dependencyIds.has('sdkwork-utils'), 'agent.workflow.json must declare sdkwork-utils sibling checkout');

const postgresPoolSource = read('sdkwork-agent-database/src/postgres_pool.rs');
assert.match(
  postgresPoolSource,
  /create_pool_from_config/u,
  'postgres_pool.rs must bootstrap pools through sdkwork-database-sqlx'
);

const workspaceManifest = read('pnpm-workspace.yaml');
// The TypeScript SDK consumes only `@sdkwork/sdk-common`, so the pnpm
// workspace intentionally does not link the sibling sdkwork-utils-typescript
// package; the Rust workspace and package workflow below carry the utils
// alignment.
assert.doesNotMatch(
  workspaceManifest,
  /sdkwork-kernel-ui/u,
  'pnpm-workspace.yaml must not reference removed sdkwork-kernel-ui workspace'
);

const packageWorkflow = read('.github/workflows/package.yml');
assert.match(packageWorkflow, /SDKWORK_UTILS_REF/u, 'package workflow must expose SDKWORK_UTILS_REF');

process.stdout.write('sdkwork-kernel utils standard passed\n');
