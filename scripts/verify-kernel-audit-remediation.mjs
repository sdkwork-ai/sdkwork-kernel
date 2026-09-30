#!/usr/bin/env node

import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';

import {
  shouldRetryWindowsCargoCommand,
  WINDOWS_CARGO_FILESYSTEM_MAX_ATTEMPTS
} from './lib/windows-cargo-filesystem-retry.mjs';

const kernelRoot = process.cwd();
const auditEnvironment = { ...process.env };

// Cargo's parallel build-script alias creation is unreliable on Windows when
// CARGO_TARGET_DIR is hosted on a filesystem without hard-link support (for
// example exFAT). Keep the audit deterministic while preserving explicit
// caller overrides and normal Linux CI parallelism.
if (process.platform === 'win32') {
  auditEnvironment.CARGO_BUILD_JOBS ??= '1';
  auditEnvironment.CARGO_INCREMENTAL ??= '0';
}

function truthyEnv(name) {
  const value = process.env[name]?.trim().toLowerCase();
  return value === '1' || value === 'true' || value === 'yes';
}

const commercialRelease =
  process.argv.includes('--commercial-release') ||
  truthyEnv('SDKWORK_KERNEL_COMMERCIAL_RELEASE_VERIFY');

const requiredArtifacts = [
  '.github/workflows/kernel-verification.yml',
  '.github/workflows/kernel-staging-live-sdk.yml',
  'docs/architecture/decisions/ADR-20260626-agents-application-layer-separation.md',
  'sdkwork-agent-server/specs/AGENT_SERVER_HTTP_SURFACE.md',
  'scripts/verify-kernel-audit-remediation.mjs'
];

for (const artifact of requiredArtifacts) {
  const artifactPath = path.join(kernelRoot, artifact);
  if (!fs.existsSync(artifactPath)) {
    console.error(`MISSING audit remediation artifact: ${artifact}`);
    process.exit(1);
  }
}

const commands = [
  ['node', ['scripts/check-kernel-standards.mjs']],
  ['node', ['../sdkwork-specs/tools/check-agent-workflow-standard.mjs', '--root', '.']],
  ['node', ['scripts/check-agent-sdk-workspace.mjs']],
  ['node', ['--test', 'scripts/dev/sdkwork-kernel-utils-standard.test.mjs']],
  ['node', ['scripts/provider-transport-workers/engine-sdk-live.test.mjs']],
  ['node', ['--test', 'scripts/provider-transport-workers/codex-app-server-live.test.mjs']],
  ['node', ['--test', 'scripts/provider-transport-workers/codex-cli-live.test.mjs']],
  ['node', ['--test', 'scripts/provider-transport-workers/provider-cli-live.test.mjs']],
  ['node', ['scripts/provider-transport-workers/engine-sdk-live-staging.test.mjs']],
  ['node', ['scripts/provider-transport-workers/generic-ts-sdk-worker.test.mjs']],
  ['node', ['--test', 'scripts/provider-transport-workers/generic-ts-sdk-worker-app-server.test.mjs']],
  ['node', ['scripts/provider-transport-workers/generic-python-sdk-worker.test.mjs']],
  ['node', ['scripts/provider-transport-workers/hermes-gateway-staging.test.mjs']],
  ['cargo', ['test', '--manifest-path', 'sdkwork-agent-provider-transport-ipc/Cargo.toml', '-q']],
  ['cargo', ['test', '--manifest-path', 'sdkwork-agent-provider-transport-node/Cargo.toml', '-q']],
  ['cargo', ['test', '--manifest-path', 'sdkwork-agent-provider-transport-python/Cargo.toml', '-q']],
  ['node', ['--test', 'tests/kernel_workspace_structure.test.mjs']],
  ['node', ['--test', 'tests/kernel_topology_alignment.test.mjs']],
  ['node', ['--test', 'tests/kernel_deployment_release.test.mjs']],
  ['node', ['--test', 'scripts/dev/sdkwork-kernel-topology-baggage.test.mjs']],
  ['node', ['--test', 'sdkwork-kernel-plugins/tests/kernel_plugin_structure.test.mjs']],
  ['node', ['scripts/check-int64-wire-contract.mjs']],
  ['cargo', ['test', '--manifest-path', 'sdkwork-agent-kernel/Cargo.toml', '-q']],
  ['cargo', ['test', '--manifest-path', 'sdkwork-agent-server/Cargo.toml', '-q']],
  [
    'cargo',
    [
      'test',
      '--features',
      'postgres-sync',
      '--manifest-path',
      'sdkwork-agent-database/Cargo.toml',
      '-q'
    ]
  ],
  ['cargo', ['test', '-p', 'sdkwork-routes-agent-internal-api', '-q']],
  [
    'cargo',
    [
      'test',
      '--features',
      'sqlite',
      '--test',
      'http_internal_runtime_contracts',
      '--manifest-path',
      'sdkwork-agent-server/Cargo.toml',
      '-q'
    ]
  ],
  [
    'cargo',
    [
      'test',
      '--features',
      'sqlite',
      '--test',
      'http_idempotency_contracts',
      '--manifest-path',
      'sdkwork-agent-server/Cargo.toml',
      '-q'
    ]
  ],
  ['cargo', ['test', '--manifest-path', 'sdkwork-agent-client/Cargo.toml', '-q']],
  ['cargo', ['test', '--doc', '--manifest-path', 'sdkwork-agent-kernel/Cargo.toml', '-q']],
  ['cargo', ['test', '--manifest-path', 'sdkwork-code-kernel/Cargo.toml', '-q']],
  [
    'pnpm',
    [
      '--dir',
      'sdks/sdkwork-agent-internal-sdk/sdkwork-agent-internal-sdk-typescript',
      'verify'
    ]
  ]
];

const preflightFailures = [];
const runtimePostgresUri = process.env.SDKWORK_DATABASE_URL?.trim();
if (runtimePostgresUri) {
  commands.push([
    'cargo',
    [
      'test',
      '--features',
      'postgres-sync',
      '--test',
      'agent_runtime_postgres_contracts',
      '--manifest-path',
      'sdkwork-agent-database/Cargo.toml',
      '-q',
      'live_postgres_session_message_roundtrip_when_uri_configured',
      '--',
      '--ignored',
      '--nocapture'
    ]
  ]);
} else if (commercialRelease) {
  preflightFailures.push(
    'commercial release verification requires live runtime PostgreSQL; set SDKWORK_DATABASE_URL'
  );
} else {
  console.log(
    'SKIP: live runtime PostgreSQL contract (set SDKWORK_DATABASE_URL to enable locally).'
  );
}

if (commercialRelease && !truthyEnv('SDKWORK_KERNEL_STAGING_HERMES_GATEWAY')) {
  preflightFailures.push(
    'commercial release verification requires Hermes staging gateway proof; set SDKWORK_KERNEL_STAGING_HERMES_GATEWAY=1'
  );
}

if (commercialRelease) {
  commands.push([
    'node',
    ['scripts/provider-transport-workers/engine-sdk-live-staging.mjs', '--framework', 'all'],
    {
      SDKWORK_KERNEL_STAGING_LIVE_SDK: '1',
      SDKWORK_KERNEL_STAGING_REQUIRE_CREDENTIALS: '1'
    }
  ]);
  commands.push([
    'node',
    ['scripts/provider-transport-workers/hermes-gateway-staging.mjs']
  ]);
}

let failed = 0;
for (const message of preflightFailures) {
  console.error(`FAILED: ${message}`);
  failed += 1;
}
if (commercialRelease && failed > 0) {
  console.error(`Kernel audit verification failed (${failed} commercial release preflight).`);
  process.exit(1);
}

for (const [cmd, args, extraEnv] of commands) {
  const label = `${cmd} ${args.join(' ')}`;
  const captureOutput = cmd === 'cargo' && process.platform === 'win32';
  let result;
  for (
    let attempt = 1;
    attempt <= WINDOWS_CARGO_FILESYSTEM_MAX_ATTEMPTS;
    attempt += 1
  ) {
    result = spawnSync(cmd, args, {
      cwd: kernelRoot,
      env: extraEnv ? { ...auditEnvironment, ...extraEnv } : auditEnvironment,
      stdio: captureOutput ? 'pipe' : 'inherit',
      encoding: captureOutput ? 'utf8' : undefined,
      maxBuffer: captureOutput ? 64 * 1024 * 1024 : undefined,
      shell: process.platform === 'win32'
    });
    if (captureOutput) {
      process.stdout.write(result.stdout ?? '');
      process.stderr.write(result.stderr ?? '');
    }
    if (result.status === 0) {
      break;
    }
    const output = `${result.error?.message ?? ''}\n${result.stdout ?? ''}${result.stderr ?? ''}`;
    if (!shouldRetryWindowsCargoCommand(cmd, output, attempt)) {
      break;
    }
    console.warn(
      `warning: retrying ${label} after transient Windows filesystem access denial ` +
      `(${attempt}/${WINDOWS_CARGO_FILESYSTEM_MAX_ATTEMPTS}).`
    );
  }
  if (result.status !== 0) {
    console.error(`FAILED: ${label}`);
    failed += 1;
  } else {
    console.log(`PASSED: ${label}`);
  }
}

if (failed > 0) {
  console.error(`Kernel audit verification failed (${failed} command groups).`);
  process.exit(1);
}

console.log('Kernel audit verification passed.');
