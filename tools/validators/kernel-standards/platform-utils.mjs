import path from 'node:path';

const REQUIRED_UTILS_RUST_CONSUMERS = ['sdkwork-agent-database/src/postgres_pool.rs'];

/**
 * sdkwork-utils alignment: workspace dependency, workflow checkout, and canonical Rust consumers.
 */
export function validatePlatformUtils({ kernelRoot, errors, readFileIfExists }) {
  const workspaceCargoText = readFileIfExists(path.join(kernelRoot, 'Cargo.toml'));
  if (!workspaceCargoText) {
    errors.push('missing workspace Cargo.toml for sdkwork-utils integration');
    return;
  }

  if (!workspaceCargoText.includes('sdkwork-utils-rust =')) {
    errors.push(
      'Cargo.toml [workspace.dependencies] must declare sdkwork-utils-rust for sdkwork-utils integration'
    );
  }

  const agentDatabaseCargo = readFileIfExists(path.join(kernelRoot, 'sdkwork-agent-database/Cargo.toml'));
  if (!agentDatabaseCargo || !agentDatabaseCargo.includes('sdkwork-utils-rust')) {
    errors.push('sdkwork-agent-database/Cargo.toml must declare sdkwork-utils-rust for postgres-sync alignment');
  }

  for (const relativePath of REQUIRED_UTILS_RUST_CONSUMERS) {
    const source = readFileIfExists(path.join(kernelRoot, relativePath));
    if (!source) {
      errors.push(`${relativePath} must exist for sdkwork-database-sqlx pool bootstrap`);
      continue;
    }
    if (!source.includes('create_pool_from_config')) {
      errors.push(`${relativePath} must bootstrap pools through sdkwork-database-sqlx`);
    }
  }

  const workflowPath = path.join(kernelRoot, 'agent.workflow.json');
  const workflowText = readFileIfExists(workflowPath);
  if (!workflowText) {
    errors.push('agent.workflow.json must exist for sdkwork-utils sibling checkout');
    return;
  }

  let workflowConfig;
  try {
    workflowConfig = JSON.parse(workflowText);
  } catch (error) {
    errors.push(`invalid json: agent.workflow.json: ${error.message}`);
    return;
  }

  const dependencyIds = new Set((workflowConfig.dependencies || []).map((entry) => entry.id));
  if (!dependencyIds.has('sdkwork-utils')) {
    errors.push('agent.workflow.json must declare sdkwork-utils sibling checkout');
  }

  const utilsTestPath = path.join(kernelRoot, 'scripts/dev/sdkwork-kernel-utils-standard.test.mjs');
  const utilsTest = readFileIfExists(utilsTestPath);
  if (!utilsTest) {
    errors.push('scripts/dev/sdkwork-kernel-utils-standard.test.mjs must exist for sdkwork-utils standard verification');
  }

  const adrPath = path.join(kernelRoot, 'docs/architecture/decisions/ADR-20260618-platform-framework-adoption.md');
  const adrText = readFileIfExists(adrPath);
  if (adrText && !adrText.includes('sdkwork-utils')) {
    errors.push(
      'docs/architecture/decisions/ADR-20260618-platform-framework-adoption.md must document sdkwork-utils adoption'
    );
  }
}
