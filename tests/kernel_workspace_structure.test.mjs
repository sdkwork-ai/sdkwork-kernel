import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(__dirname, '..');
const removedSdkAssemblyManifest = ['.sdkwork', 'assembly.json'].join('-');

const standardDirectories = [
  'apis',
  'apps',
  'crates',
  'sdks',
  'jobs',
  'tools',
  'plugins',
  'examples',
  'configs',
  'deployments',
  'scripts',
  'docs',
  'tests'
];

test('repository root exposes the SDKWork standard directory dictionary', () => {
  for (const directory of standardDirectories) {
    const directoryPath = path.join(root, directory);
    assert.equal(fs.existsSync(directoryPath), true, `${directory}/ should exist`);
    assert.equal(fs.statSync(directoryPath).isDirectory(), true, `${directory}/ should be a directory`);

    const readmePath = path.join(directoryPath, 'README.md');
    assert.equal(fs.existsSync(readmePath), true, `${directory}/README.md should exist`);
    assert.equal(fs.statSync(readmePath).isFile(), true, `${directory}/README.md should be a file`);
  }
});

test('.sdkwork documentation is repository-specific and resolves root standards', () => {
  for (const relativePath of [
    '.sdkwork/README.md',
    '.sdkwork/skills/README.md',
    '.sdkwork/plugins/README.md'
  ]) {
    const content = fs.readFileSync(path.join(root, relativePath), 'utf8');
    assert.doesNotMatch(content, /\$(name|specPath)/, `${relativePath} must not retain template variables`);
    assert.match(
      content,
      /\.\.\/sdkwork-specs\/SDKWORK_WORKSPACE_SPEC\.md/,
      `${relativePath} should cite SDKWORK_WORKSPACE_SPEC.md`
    );
  }
});

test('documented verification commands target the current repository root', () => {
  const readme = fs.readFileSync(path.join(root, 'README.md'), 'utf8');

  for (const staleCommand of [
    'node kernel/scripts/check-kernel-standards.mjs',
    'node kernel/sdkwork-kernel-ui/scripts/check-kernel-ui-architecture.mjs',
    'cargo test --manifest-path kernel/sdkwork-agent-kernel/Cargo.toml',
    'cargo test --manifest-path kernel/sdkwork-agent-business/Cargo.toml',
    'cargo test --manifest-path kernel/sdkwork-code-kernel/Cargo.toml',
    'pnpm --dir kernel/sdkwork-kernel-ui'
  ]) {
    assert.equal(readme.includes(staleCommand), false, `README.md must not document stale command: ${staleCommand}`);
  }

  assert.match(readme, /\.\.\/sdkwork-specs\/README\.md/, 'README.md should link the sibling sdkwork-specs entrypoint');
  assert.match(readme, /SDKWORK_WORKSPACE_SPEC\.md/, 'README.md should cite the workspace directory standard');
});

test('structure checks run successfully from the repository root', () => {
  const result = spawnSync(process.execPath, ['scripts/check-kernel-standards.mjs'], {
    cwd: root,
    encoding: 'utf8'
  });
  assert.equal(
    result.status,
    0,
    `kernel standards check should pass from root\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`
  );
});

test('current dictionary files do not point to pre-sdkwork-specs roots', () => {
  const stalePatterns = [
    /sdkwork-birdcoder\/kernel/,
    /apps\/sdkwork-birdcoder/,
    /apps\/docs\/ARCHITECT\.md/,
    /\]\((?:\.\.\/){3,}specs\//
  ];

  for (const filePath of listCurrentDictionaryFiles()) {
    const content = fs.readFileSync(filePath, 'utf8');
    for (const pattern of stalePatterns) {
      assert.doesNotMatch(content, pattern, `${path.relative(root, filePath)} must not match ${pattern}`);
    }
  }
});

test('component canonical spec paths resolve from component roots', () => {
  for (const manifestPath of listComponentSpecFiles(root)) {
    const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
    const componentRoot = resolveComponentRoot(manifestPath, manifest);

    for (const canonicalSpec of manifest.canonicalSpecs ?? []) {
      assert.ok(
        canonicalSpec.path,
        `${path.relative(root, manifestPath)} canonical spec ${canonicalSpec.file} should declare a path`
      );

      const resolvedPath = path.resolve(componentRoot, canonicalSpec.path);
      assert.equal(
        fs.existsSync(resolvedPath),
        true,
        `${path.relative(root, manifestPath)} canonical spec ${canonicalSpec.file} should resolve to ${path.relative(root, resolvedPath)}`
      );
      assert.equal(
        fs.statSync(resolvedPath).isFile(),
        true,
        `${path.relative(root, manifestPath)} canonical spec ${canonicalSpec.file} should resolve to a file`
      );
    }
  }
});

test('component specs declare SDK metadata and authored source standards explicitly', () => {
  for (const manifestPath of listComponentSpecFiles(root)) {
    const relativePath = path.relative(root, manifestPath);
    const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
    const component = manifest.component ?? {};
    const contracts = manifest.contracts ?? {};
    const canonicalSpecFiles = new Set((manifest.canonicalSpecs ?? []).map((spec) => spec.file));
    const isSdkArea = component.type === 'sdk-family' || relativePath.startsWith(`sdks${path.sep}`);
    const ownsAuthoredSource =
      component.generated !== true &&
      ['rust-crate', 'node-package', 'react-package', 'web-backend-service', 'rust-route-crate'].includes(
        component.type
      );

    assert.equal(
      Array.isArray(contracts.sdkDependencies),
      true,
      `${relativePath} should declare contracts.sdkDependencies explicitly`
    );
    assert.equal(
      Array.isArray(contracts.dependencyApiExports),
      true,
      `${relativePath} should declare contracts.dependencyApiExports explicitly`
    );
    assert.equal(
      Array.isArray(contracts.dependencyApiSurfaces),
      true,
      `${relativePath} should declare contracts.dependencyApiSurfaces explicitly`
    );
    for (const [index, surface] of (contracts.dependencyApiSurfaces ?? []).entries()) {
      assert.equal(
        typeof surface,
        'object',
        `${relativePath} contracts.dependencyApiSurfaces[${index}] should be an object runtime surface declaration`
      );
      assert.notEqual(
        surface,
        null,
        `${relativePath} contracts.dependencyApiSurfaces[${index}] should not be null`
      );
    }

    if (isSdkArea) {
      assert.equal(
        component.surface,
        expectedSdkComponentSurface(relativePath, manifest),
        `${relativePath} should declare the standard SDK component surface`
      );
    }

    if (component.type === 'sdk-family') {
      assert.equal(
        Object.hasOwn(contracts, 'routeManifest'),
        true,
        `${relativePath} should declare contracts.routeManifest explicitly`
      );
      assert.ok(
        contracts.routeManifest === null || typeof contracts.routeManifest === 'string',
        `${relativePath} contracts.routeManifest should be null or a route manifest path`
      );
    }

    if (!isSdkArea && !component.surface) {
      assert.equal(
        typeof component.surfaceNotRequiredReason,
        'string',
        `${relativePath} should explain why component.surface is not required`
      );
      assert.ok(
        component.surfaceNotRequiredReason.trim().length >= 48,
        `${relativePath} component.surfaceNotRequiredReason should be specific`
      );
      assert.doesNotMatch(
        component.surfaceNotRequiredReason,
        /\b(?:todo|tbd|n\/a)\b/i,
        `${relativePath} component.surfaceNotRequiredReason should not be a placeholder`
      );
    }

    if (component.type === 'sdk-family') {
      assert.equal(
        (component.manifests ?? []).includes('sdk-manifest.json'),
        true,
        `${relativePath} SDK family component.manifests should include sdk-manifest.json`
      );
      assert.equal(
        (component.manifests ?? []).includes(removedSdkAssemblyManifest),
        false,
        `${relativePath} SDK family component.manifests must not reference removed parallel SDK metadata`
      );
      assert.equal(
        (contracts.runtimeEntrypoints ?? []).includes(removedSdkAssemblyManifest),
        false,
        `${relativePath} SDK family runtimeEntrypoints must not reference removed parallel SDK metadata`
      );
      assert.equal(
        (contracts.configKeys ?? []).includes(removedSdkAssemblyManifest),
        false,
        `${relativePath} SDK family configKeys must not reference removed parallel SDK metadata`
      );
      for (const specFile of [
        'SDK_SPEC.md',
        'SDK_WORKSPACE_GENERATION_SPEC.md',
        'API_SPEC.md',
        'TEST_SPEC.md',
        'DOCUMENTATION_SPEC.md'
      ]) {
        assert.equal(
          canonicalSpecFiles.has(specFile),
          true,
          `${relativePath} should cite ${specFile} in canonicalSpecs`
        );
      }
    }

    if (ownsAuthoredSource) {
      for (const specFile of ['CODE_STYLE_SPEC.md', 'NAMING_SPEC.md']) {
        assert.equal(
          canonicalSpecFiles.has(specFile),
          true,
          `${relativePath} should cite ${specFile} in canonicalSpecs`
        );
      }
    }
  }
});

test('member Cargo manifests consume SDKWork sibling crates through workspace dependencies', () => {
  const rootCargoPath = path.join(root, 'Cargo.toml');

  for (const cargoPath of listCargoTomlFiles(root)) {
    if (path.resolve(cargoPath) === path.resolve(rootCargoPath)) continue;

    const relativePath = path.relative(root, cargoPath);
    const content = fs.readFileSync(cargoPath, 'utf8');
    for (const line of content.split(/\r?\n/)) {
      const match = line
        .replace(/\s+#.*$/u, '')
        .match(/^\s*([A-Za-z0-9_-]+)\s*=\s*\{([^}]*\bpath\s*=)[^}]*\}/u);
      if (!match) continue;

      const dependencyName = match[1].replaceAll('_', '-');
      if (!dependencyName.startsWith('sdkwork-')) continue;
      assert.doesNotMatch(
        line,
        /\bpath\s*=\s*"\.\.(?:\/|\\)sdkwork-/u,
        `${relativePath} dependency ${dependencyName} should use workspace = true instead of a sibling path`
      );
    }
  }
});

test('standards alignment records architecture decision and quality gate evidence', () => {
  const adrPath = path.join(
    root,
    'docs',
    'architecture',
    'decisions',
    'ADR-20260612-sdkwork-kernel-root-dictionary.md'
  );
  const qualityPath = path.join(
    root,
    'docs',
    'architecture',
    'tech',
    'TECH-sdkwork-standards-alignment-20260612.md'
  );

  assert.equal(fs.existsSync(adrPath), true, 'standards alignment ADR should exist');
  assert.equal(fs.existsSync(qualityPath), true, 'standards alignment quality gate evidence should exist');

  const adr = fs.readFileSync(adrPath, 'utf8');
  for (const requiredText of [
    'Status: accepted',
    'Owner:',
    'Date: 2026-06-12',
    'Specs:',
    '## Context',
    '## Decision',
    '## Alternatives',
    '## Consequences',
    '## Verification',
    'SDKWORK_WORKSPACE_SPEC.md',
    'COMPONENT_SPEC.md',
    'SDK_SPEC.md',
    'ARCHITECTURE_DECISION_SPEC.md',
    'QUALITY_GATE_SPEC.md',
    'sdkwork-agent-kernel/',
    'sdkwork-code-kernel/',
    'sdkwork-agent-server/',
    'sdkwork-kernel-plugins/',
    'ADR-20260626-agents-application-layer-separation.md'
  ]) {
    assert.ok(adr.includes(requiredText), `ADR should include ${requiredText}`);
  }

  const quality = fs.readFileSync(qualityPath, 'utf8');
  for (const requiredText of [
    'SDKWork Standards Alignment Quality Gate Evidence',
    'Date: 2026-06-12',
    'QUALITY_GATE_SPEC.md',
    'ARCHITECTURE_DECISION_SPEC.md',
    'SDKWORK_WORKSPACE_SPEC.md',
    'COMPONENT_SPEC.md',
    'SDK_SPEC.md',
    'SDK_WORKSPACE_GENERATION_SPEC.md',
    'TEST_SPEC.md',
    'Definition Of Ready',
    'Definition Of Done',
    'node --test tests\\*.test.mjs',
    'node scripts\\check-kernel-standards.mjs',
    'node scripts\\check-agent-sdk-workspace.mjs',
    '7 tests pass',
    '8 tests pass',
    '9 tests pass',
    '10 tests pass',
    '11 tests pass',
    '12 tests pass',
    '13 tests pass',
    '14 tests pass',
    '15 tests pass',
    '16 tests pass',
    '17 tests pass',
    '18 tests pass',
    'Kernel standards conformance check passed.',
    'Agent SDK workspace check passed.',
    'No generated output was hand-edited',
    'SDK metadata scan',
    'SDK surface scan',
    'tools/validators/kernel-standards/check-kernel-standards.mjs',
    'tools/validators/kernel-standards/component-specs.mjs',
    'tools/validators/kernel-standards/workspace-evidence.mjs',
    'tools/validators/kernel-standards/agent-knowledge-memory-contracts.mjs',
    'tools/validators/kernel-standards/kernel-contracts.mjs',
    'tools/validators/kernel-standards/platform-integration.mjs',
    'tools/validators/agent-sdk-workspace/check-agent-sdk-workspace.mjs',
    'tools/validators/agent-sdk-workspace/sdkgen-standard-checks.mjs',
    'tools/validators/agent-sdk-workspace/sdk-family-metadata-checks.mjs',
    'tools/validators/agent-sdk-workspace/openapi-checks.mjs',
    'tools/validators/agent-sdk-workspace/generated-typescript-api-surface-checks.mjs',
    'Generated TypeScript API surface validator split',
    '20 tests pass',
    'Component Surface Applicability Follow-up',
    'surfaceNotRequiredReason',
    '19 non-SDK component specs document why component.surface is not required',
    'Component Dependency Policy Follow-up',
    'All component specs explicitly declare contracts.sdkDependencies and contracts.dependencyApiExports',
    'Component Dependency API Surface Follow-up',
    'All component specs explicitly declare contracts.dependencyApiSurfaces',
    'Component SDK Route Manifest Applicability Follow-up',
    'SDK family root component specs explicitly declare contracts.routeManifest as null',
    '"gapCount": 0',
    'Platform Framework Adoption Follow-up',
    'ADR-20260618-platform-framework-adoption.md',
    'ADR-20260626-agents-application-layer-separation.md',
    'apis/internal-api/authority-index.json',
    'platform-integration.mjs'
  ]) {
    assert.ok(quality.includes(requiredText), `quality gate evidence should include ${requiredText}`);
  }
  assert.equal(
    quality.includes('19 non-SDK component specs still omit component.surface'),
    false,
    'quality gate evidence should not keep the stale component.surface residual risk'
  );
});

test('platform framework adoption records ADR, internal API authority index, and validator module', () => {
  const adrPath = path.join(
    root,
    'docs',
    'architecture',
    'decisions',
    'ADR-20260618-platform-framework-adoption.md'
  );
  const separationAdrPath = path.join(
    root,
    'docs',
    'architecture',
    'decisions',
    'ADR-20260626-agents-application-layer-separation.md'
  );
  const authorityIndexPath = path.join(root, 'apis', 'internal-api', 'authority-index.json');
  const platformValidatorPath = path.join(
    root,
    'tools',
    'validators',
    'kernel-standards',
    'platform-integration.mjs'
  );

  assert.equal(fs.existsSync(adrPath), true, 'platform framework adoption ADR should exist');
  assert.equal(fs.existsSync(separationAdrPath), true, 'agents application layer separation ADR should exist');
  assert.equal(fs.existsSync(authorityIndexPath), true, 'internal-api API authority index should exist');
  assert.equal(fs.existsSync(platformValidatorPath), true, 'platform integration validator should exist');
  assert.equal(fs.existsSync(path.join(root, 'apis', 'agent-business')), false, 'retired agent-business apis must be removed');

  const adr = fs.readFileSync(adrPath, 'utf8');
  for (const requiredText of [
    'Status: accepted',
    'WEB_FRAMEWORK_SPEC.md',
    'DATABASE_SPEC.md',
    'sdkwork-web-framework',
    'sdkwork-database',
    'sdkwork-discovery',
    'Phase 0',
    'Phase 2',
    'Phase 3',
    'Phase 4'
  ]) {
    assert.ok(adr.includes(requiredText), `platform ADR should include ${requiredText}`);
  }

  const authorityIndex = JSON.parse(fs.readFileSync(authorityIndexPath, 'utf8'));
  const surfaces = new Set(authorityIndex.authorities.map((entry) => entry.surface));
  assert.equal(surfaces.has('internal-api'), true, 'authority index should include internal-api');

  const workspaceCargo = fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8');
  for (const dependency of [
    'sdkwork-web-core',
    'sdkwork-web-axum',
    'sdkwork-database-config',
    'sdkwork-database-sqlx'
  ]) {
    assert.match(
      workspaceCargo,
      new RegExp(`${dependency} =`),
      `workspace Cargo.toml should declare ${dependency}`
    );
  }

  const standardsValidator = fs.readFileSync(
    path.join(root, 'tools', 'validators', 'kernel-standards', 'check-kernel-standards.mjs'),
    'utf8'
  );
  assert.match(
    standardsValidator,
    /from '\.\/platform-integration\.mjs'/,
    'kernel standards validator should import platform integration checks'
  );
  assert.match(
    standardsValidator,
    /from '\.\/platform-utils\.mjs'/,
    'kernel standards validator should import platform utils checks'
  );
  assert.match(
    standardsValidator,
    /from '\.\/platform-pnpm-scripts\.mjs'/,
    'kernel standards validator should import platform pnpm script checks'
  );
});

test('sdkwork-utils integration records workspace dependency, validator, and standard test', () => {
  const workspaceCargo = fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8');
  assert.match(workspaceCargo, /sdkwork-utils-rust =/, 'workspace Cargo.toml should declare sdkwork-utils-rust');

  const workflow = JSON.parse(fs.readFileSync(path.join(root, 'agent.workflow.json'), 'utf8'));
  const dependencyIds = new Set((workflow.dependencies || []).map((entry) => entry.id));
  assert.equal(dependencyIds.has('sdkwork-utils'), true, 'workflow should declare sdkwork-utils dependency');

  const utilsValidatorPath = path.join(root, 'tools', 'validators', 'kernel-standards', 'platform-utils.mjs');
  const utilsTestPath = path.join(root, 'scripts', 'dev', 'sdkwork-kernel-utils-standard.test.mjs');
  assert.equal(fs.existsSync(utilsValidatorPath), true, 'platform-utils validator should exist');
  assert.equal(fs.existsSync(utilsTestPath), true, 'utils standard test should exist');

  const adr = fs.readFileSync(
    path.join(root, 'docs', 'architecture', 'decisions', 'ADR-20260618-platform-framework-adoption.md'),
    'utf8'
  );
  assert.ok(adr.includes('sdkwork-utils'), 'platform ADR should document sdkwork-utils adoption');
});

test('platform packaging records agent.workflow.json and package workflow entrypoint', () => {
  const workflowPath = path.join(root, 'agent.workflow.json');
  const packageWorkflowPath = path.join(root, '.github', 'workflows', 'package.yml');

  assert.equal(fs.existsSync(workflowPath), true, 'agent.workflow.json should exist');
  assert.equal(fs.existsSync(packageWorkflowPath), true, 'package workflow should exist');

  const workflow = JSON.parse(fs.readFileSync(workflowPath, 'utf8'));
  assert.equal(workflow.app?.id, 'sdkwork-kernel');
  assert.ok(Array.isArray(workflow.targets) && workflow.targets.length > 0, 'workflow should declare package targets');

  const packageWorkflow = fs.readFileSync(packageWorkflowPath, 'utf8');
  assert.match(
    packageWorkflow,
    /sdkwork-github-workflow\/\.github\/workflows\/sdkwork-package\.yml/,
    'package workflow should call reusable sdkwork-github-workflow packaging workflow'
  );
  assert.match(
    packageWorkflow,
    /config_path: sdkwork\.workflow\.json/,
    'package workflow should pass agent.workflow.json'
  );
});

test('agent implementation type records architecture decision and quality gate evidence', () => {
  const adrPath = path.join(
    root,
    'docs',
    'architecture',
    'decisions',
    'ADR-20260612-agent-implementation-type.md'
  );
  const qualityPath = path.join(
    root,
    'docs',
    'architecture',
    'tech',
    'TECH-sdkwork-standards-alignment-20260612.md'
  );
  const planPath = path.join(
    root,
    'docs',
    'archive',
    'superpowers',
    'plans',
    '2026-06-12-agent-implementation-type.md'
  );

  assert.equal(fs.existsSync(adrPath), true, 'agent implementation type ADR should exist');
  assert.equal(fs.existsSync(qualityPath), true, 'standards alignment quality gate evidence should exist');
  assert.equal(fs.existsSync(planPath), true, 'agent implementation type execution plan should exist');

  const adr = fs.readFileSync(adrPath, 'utf8');
  for (const requiredText of [
    'Status: accepted',
    'Requirement: agent-implementation-type',
    'Date: 2026-06-12',
    'ARCHITECTURE_DECISION_SPEC.md',
    'DATABASE_SPEC.md',
    'API_SPEC.md',
    'SDK_SPEC.md',
    'implementationKind',
    'implementationType',
    'sdkwork-native',
    'openai-agents',
    'langgraph',
    'a_agent_business.implementation_type',
    'Generated SDK output was refreshed through generator commands',
    '## Verification'
  ]) {
    assert.ok(adr.includes(requiredText), `agent implementation type ADR should include ${requiredText}`);
  }

  const quality = fs.readFileSync(qualityPath, 'utf8');
  for (const requiredText of [
    'Agent Implementation Type Follow-up',
    'ADR-20260612-agent-implementation-type.md',
    '2026-06-12-agent-implementation-type.md',
    'ADR-20260626-agents-application-layer-separation.md',
    'node sdks/workspace-agent-sdkgen.mjs --mode dry-run',
    'hasChanges: false',
    'Agent implementation type evidence is enforced by tools/validators/kernel-standards/workspace-evidence.mjs',
    '19 tests pass'
  ]) {
    assert.ok(quality.includes(requiredText), `quality gate evidence should include ${requiredText}`);
  }

  const plan = fs.readFileSync(planPath, 'utf8');
  assert.doesNotMatch(plan, /^- \[ \]/m, 'agent implementation type plan should have no unfinished steps');
  assert.match(plan, /Execution Evidence/, 'agent implementation type plan should record execution evidence');
});

test('kernel standards command delegates to a reusable tools validator', () => {
  const scriptPath = path.join(root, 'scripts', 'check-kernel-standards.mjs');
  const validatorPath = path.join(
    root,
    'tools',
    'validators',
    'kernel-standards',
    'check-kernel-standards.mjs'
  );
  const validatorReadmePath = path.join(root, 'tools', 'validators', 'kernel-standards', 'README.md');

  assert.equal(fs.existsSync(validatorPath), true, 'kernel standards validator module should exist');
  assert.equal(fs.existsSync(validatorReadmePath), true, 'kernel standards validator README should exist');

  const script = fs.readFileSync(scriptPath, 'utf8');
  assert.match(
    script,
    /\.\.\/tools\/validators\/kernel-standards\/check-kernel-standards\.mjs/,
    'thin script should import the reusable validator'
  );
  assert.match(script, /runKernelStandardsCheck/, 'thin script should call the reusable validator');
  assert.ok(
    script.split(/\r?\n/).filter((line) => line.trim() && !line.trim().startsWith('#!')).length <= 8,
    'scripts/check-kernel-standards.mjs should stay a thin entrypoint'
  );

  const validator = fs.readFileSync(validatorPath, 'utf8');
  assert.match(
    validator,
    /export function runKernelStandardsCheck/,
    'validator module should export runKernelStandardsCheck'
  );
  assert.match(
    validator,
    /Kernel standards conformance check passed\./,
    'validator module should preserve existing success output'
  );

  const validatorReadme = fs.readFileSync(validatorReadmePath, 'utf8');
  for (const requiredText of [
    'Purpose:',
    'Owner:',
    'Allowed content:',
    'Forbidden content:',
    'Related specs:',
    'Verification:',
    '../../../../sdkwork-specs/TYPESCRIPT_CODE_SPEC.md',
    'node scripts/check-kernel-standards.mjs'
  ]) {
    assert.ok(validatorReadme.includes(requiredText), `validator README should include ${requiredText}`);
  }
});

test('kernel standards validator splits component spec validation into a focused module', () => {
  const validatorPath = path.join(
    root,
    'tools',
    'validators',
    'kernel-standards',
    'check-kernel-standards.mjs'
  );
  const componentSpecsPath = path.join(
    root,
    'tools',
    'validators',
    'kernel-standards',
    'component-specs.mjs'
  );

  assert.equal(fs.existsSync(componentSpecsPath), true, 'component spec validator module should exist');

  const validator = fs.readFileSync(validatorPath, 'utf8');
  assert.match(
    validator,
    /from '\.\/component-specs\.mjs'/,
    'kernel standards validator should import component spec helpers'
  );
  assert.doesNotMatch(
    validator,
    /function validateComponentSpecMetadata/,
    'kernel standards validator should not inline component spec metadata validation'
  );
  assert.doesNotMatch(
    validator,
    /function expectedSdkComponentSurface/,
    'kernel standards validator should not inline SDK surface inference'
  );

  const componentSpecs = fs.readFileSync(componentSpecsPath, 'utf8');
  for (const exportName of [
    'listComponentSpecFiles',
    'validateComponentCanonicalSpecPaths',
    'validateComponentSpecMetadata',
    'expectedSdkComponentSurface'
  ]) {
    assert.match(
      componentSpecs,
      new RegExp(`export function ${exportName}\\b`),
      `component spec validator should export ${exportName}`
    );
  }
  for (const requiredText of ['surfaceNotRequiredReason', 'validateSurfaceNotRequiredReason']) {
    assert.ok(componentSpecs.includes(requiredText), `component spec validator should include ${requiredText}`);
  }
});

test('kernel standards validator splits workspace evidence checks into a focused module', () => {
  const validatorPath = path.join(
    root,
    'tools',
    'validators',
    'kernel-standards',
    'check-kernel-standards.mjs'
  );
  const workspaceEvidencePath = path.join(
    root,
    'tools',
    'validators',
    'kernel-standards',
    'workspace-evidence.mjs'
  );

  assert.equal(fs.existsSync(workspaceEvidencePath), true, 'workspace evidence validator module should exist');

  const validator = fs.readFileSync(validatorPath, 'utf8');
  assert.match(
    validator,
    /from '\.\/platform-integration\.mjs'/,
    'kernel standards validator should import platform integration checks'
  );
  assert.match(
    validator,
    /from '\.\/workspace-evidence\.mjs'/,
    'kernel standards validator should import workspace evidence helpers'
  );
  assert.doesNotMatch(
    validator,
    /const standardRootDirectories = \[/,
    'kernel standards validator should not inline standard root directory data'
  );
  assert.doesNotMatch(
    validator,
    /function validateRequiredStandardsEvidenceFile/,
    'kernel standards validator should not inline lifecycle evidence validation'
  );

  const workspaceEvidence = fs.readFileSync(workspaceEvidencePath, 'utf8');
  for (const requiredText of [
    'export function validateWorkspaceEvidence',
    'standardRootDirectories',
    'requiredStandardsEvidenceFiles',
    'ADR-20260612-agent-implementation-type.md',
    'Agent Implementation Type Follow-up',
    'implementationType',
    'Generated TypeScript API surface validator split',
    'generated-typescript-api-surface-checks.mjs',
    'Component Surface Applicability Follow-up',
    'surfaceNotRequiredReason',
    'Component Dependency Policy Follow-up',
    'contracts.sdkDependencies and contracts.dependencyApiExports',
    'Component Dependency API Surface Follow-up',
    'contracts.dependencyApiSurfaces',
    'Component SDK Route Manifest Applicability Follow-up',
    'contracts.routeManifest',
    'ADR-20260618-platform-framework-adoption.md',
    'platform-integration.mjs',
    'validateRequiredStandardsEvidenceFile',
    'listCurrentDictionaryFiles'
  ]) {
    assert.ok(workspaceEvidence.includes(requiredText), `workspace evidence module should include ${requiredText}`);
  }
});

test('kernel standards validator splits agent knowledge and memory contracts into a focused module', () => {
  const validatorPath = path.join(
    root,
    'tools',
    'validators',
    'kernel-standards',
    'check-kernel-standards.mjs'
  );
  const agentContractsPath = path.join(
    root,
    'tools',
    'validators',
    'kernel-standards',
    'agent-knowledge-memory-contracts.mjs'
  );

  assert.equal(fs.existsSync(agentContractsPath), true, 'agent knowledge and memory validator module should exist');

  const validator = fs.readFileSync(validatorPath, 'utf8');
  assert.match(
    validator,
    /from '\.\/agent-knowledge-memory-contracts\.mjs'/,
    'kernel standards validator should import agent knowledge and memory helpers'
  );
  assert.doesNotMatch(
    validator,
    /a_agent_knowledge_base/,
    'kernel standards validator should not inline knowledge database table checks'
  );
  assert.doesNotMatch(
    validator,
    /function ensureSqlBlockIncludes/,
    'kernel standards validator should not inline SQL block helpers'
  );

  const agentContracts = fs.readFileSync(agentContractsPath, 'utf8');
  for (const requiredText of [
    'export function validateAgentKnowledgeMemoryContracts',
    'a_agent_knowledge_base',
    'a_agent_memory_store',
    'knowledgeList.list',
    'memory_store_created',
    'ensureSqlBlockIncludes'
  ]) {
    assert.ok(agentContracts.includes(requiredText), `agent knowledge and memory module should include ${requiredText}`);
  }
});

test('kernel standards validator splits kernel contract checks into a focused module', () => {
  const validatorPath = path.join(
    root,
    'tools',
    'validators',
    'kernel-standards',
    'check-kernel-standards.mjs'
  );
  const kernelContractsPath = path.join(
    root,
    'tools',
    'validators',
    'kernel-standards',
    'kernel-contracts.mjs'
  );

  assert.equal(fs.existsSync(kernelContractsPath), true, 'kernel contract validator module should exist');

  const validator = fs.readFileSync(validatorPath, 'utf8');
  assert.match(
    validator,
    /from '\.\/kernel-contracts\.mjs'/,
    'kernel standards validator should import kernel contract helpers'
  );
  assert.doesNotMatch(
    validator,
    /const requiredSpecFiles = \[/,
    'kernel standards validator should not inline required spec file data'
  );
  assert.doesNotMatch(
    validator,
    /const requiredSchemas = \[/,
    'kernel standards validator should not inline required schema data'
  );
  assert.doesNotMatch(
    validator,
    /const requiredRustCrates = \[/,
    'kernel standards validator should not inline required Rust crate data'
  );
  assert.doesNotMatch(
    validator,
    /kernel plugin structure check failed/,
    'kernel standards validator should not inline kernel plugin command validation'
  );

  const kernelContracts = fs.readFileSync(kernelContractsPath, 'utf8');
  for (const requiredText of [
    'export function validateKernelContracts',
    'requiredSpecFiles',
    'requiredSchemas',
    'requiredRustCrates',
    'kernel plugin structure check failed',
    'sdkwork-code-kernel must depend on sdkwork-agent-kernel'
  ]) {
    assert.ok(kernelContracts.includes(requiredText), `kernel contract module should include ${requiredText}`);
  }
});

test('kernel standards validator splits runtime topology checks into a focused module', () => {
  const validatorPath = path.join(
    root,
    'tools',
    'validators',
    'kernel-standards',
    'check-kernel-standards.mjs'
  );
  const kernelTopologyPath = path.join(
    root,
    'tools',
    'validators',
    'kernel-standards',
    'kernel-topology.mjs'
  );

  assert.equal(fs.existsSync(kernelTopologyPath), true, 'kernel topology validator module should exist');

  const validator = fs.readFileSync(validatorPath, 'utf8');
  assert.match(
    validator,
    /from '\.\/kernel-topology\.mjs'/,
    'kernel standards validator should import kernel topology helpers'
  );
  assert.match(validator, /validateKernelTopology/, 'kernel standards validator should call validateKernelTopology');

  const kernelTopology = fs.readFileSync(kernelTopologyPath, 'utf8');
  for (const requiredText of [
    'export function validateKernelTopology',
    'scripts/lib/kernel-topology.mjs',
    'docs/topology-standard.md',
    'sdkwork-topology.mjs',
    "'validate'"
  ]) {
    assert.ok(kernelTopology.includes(requiredText), `kernel topology module should include ${requiredText}`);
  }
});

test('agent SDK workspace command delegates to a reusable tools validator', () => {
  const scriptPath = path.join(root, 'scripts', 'check-agent-sdk-workspace.mjs');
  const validatorPath = path.join(
    root,
    'tools',
    'validators',
    'agent-sdk-workspace',
    'check-agent-sdk-workspace.mjs'
  );
  const validatorReadmePath = path.join(root, 'tools', 'validators', 'agent-sdk-workspace', 'README.md');

  assert.equal(fs.existsSync(validatorPath), true, 'agent SDK workspace validator module should exist');
  assert.equal(fs.existsSync(validatorReadmePath), true, 'agent SDK workspace validator README should exist');

  const script = fs.readFileSync(scriptPath, 'utf8');
  assert.match(
    script,
    /\.\.\/tools\/validators\/agent-sdk-workspace\/check-agent-sdk-workspace\.mjs/,
    'thin script should import the reusable agent SDK workspace validator'
  );
  assert.match(script, /runAgentSdkWorkspaceCheck/, 'thin script should call the reusable validator');
  assert.ok(
    script.split(/\r?\n/).filter((line) => line.trim() && !line.trim().startsWith('#!')).length <= 8,
    'scripts/check-agent-sdk-workspace.mjs should stay a thin entrypoint'
  );
  assert.doesNotMatch(
    script,
    /AGENT_SDK_FAMILIES/,
    'thin script should not inline agent SDK family validation'
  );
  assert.doesNotMatch(
    script,
    /function validateOpenApi/,
    'thin script should not inline OpenAPI validation'
  );

  const validator = fs.readFileSync(validatorPath, 'utf8');
  assert.match(
    validator,
    /export function runAgentSdkWorkspaceCheck/,
    'agent SDK workspace validator should export runAgentSdkWorkspaceCheck'
  );
  assert.match(
    validator,
    /Agent SDK workspace check passed\./,
    'agent SDK workspace validator should preserve existing success output'
  );
  assert.match(
    validator,
    /AGENT_SDK_FAMILIES/,
    'agent SDK workspace validator should own agent SDK family validation'
  );

  const validatorReadme = fs.readFileSync(validatorReadmePath, 'utf8');
  for (const requiredText of [
    'Purpose:',
    'Owner:',
    'Allowed content:',
    'Forbidden content:',
    'Related specs:',
    'Verification:',
    '../../../../sdkwork-specs/TYPESCRIPT_CODE_SPEC.md',
    'generated-typescript-api-surface-checks.mjs',
    'node scripts/check-agent-sdk-workspace.mjs'
  ]) {
    assert.ok(validatorReadme.includes(requiredText), `agent SDK workspace validator README should include ${requiredText}`);
  }
});

test('agent SDK workspace validator splits sdkgen standard checks into a focused module', () => {
  const validatorPath = path.join(
    root,
    'tools',
    'validators',
    'agent-sdk-workspace',
    'check-agent-sdk-workspace.mjs'
  );
  const sdkgenStandardPath = path.join(
    root,
    'tools',
    'validators',
    'agent-sdk-workspace',
    'sdkgen-standard-checks.mjs'
  );

  assert.equal(fs.existsSync(sdkgenStandardPath), true, 'sdkgen standard validator module should exist');

  const validator = fs.readFileSync(validatorPath, 'utf8');
  assert.match(
    validator,
    /from '\.\/sdkgen-standard-checks\.mjs'/,
    'agent SDK workspace validator should import sdkgen standard helpers'
  );
  assert.doesNotMatch(
    validator,
    /function validateSdkgenReport/,
    'agent SDK workspace validator should not inline sdkgen report validation'
  );
  assert.doesNotMatch(
    validator,
    /containsLocalDriveAbsolutePath/,
    'agent SDK workspace validator should not inline local absolute path checks'
  );

  const sdkgenStandard = fs.readFileSync(sdkgenStandardPath, 'utf8');
  for (const requiredText of [
    'export function validateSdkgenStandard',
    'validateSdkgenReport',
    'resolveSdkgenEntrypoint',
    'containsLocalDriveAbsolutePath',
    'deprecated sdkgen path'
  ]) {
    assert.ok(sdkgenStandard.includes(requiredText), `sdkgen standard module should include ${requiredText}`);
  }
});

test('agent SDK workspace validator splits SDK family metadata checks into a focused module', () => {
  const validatorPath = path.join(
    root,
    'tools',
    'validators',
    'agent-sdk-workspace',
    'check-agent-sdk-workspace.mjs'
  );
  const sdkFamilyMetadataPath = path.join(
    root,
    'tools',
    'validators',
    'agent-sdk-workspace',
    'sdk-family-metadata-checks.mjs'
  );

  assert.equal(fs.existsSync(sdkFamilyMetadataPath), true, 'SDK family metadata validator module should exist');

  const validator = fs.readFileSync(validatorPath, 'utf8');
  assert.match(
    validator,
    /from '\.\/sdk-family-metadata-checks\.mjs'/,
    'agent SDK workspace validator should import SDK family metadata helpers'
  );
  assert.doesNotMatch(
    validator,
    /const assembly = readJsonIfExists/,
    'agent SDK workspace validator should not inline assembly metadata validation'
  );
  assert.doesNotMatch(
    validator,
    /\.sdkwork-assembly\.json/,
    'agent SDK workspace validator should not require retired per-family assembly files'
  );
  assert.doesNotMatch(
    validator,
    /function assertDependencyList/,
    'agent SDK workspace validator should not inline dependency list comparison'
  );
  assert.doesNotMatch(
    validator,
    /function assertNoGeneratedOwnershipStandardKeys/,
    'agent SDK workspace validator should not inline generated ownership metadata checks'
  );

  const sdkFamilyMetadata = fs.readFileSync(sdkFamilyMetadataPath, 'utf8');
  assert.doesNotMatch(
    sdkFamilyMetadata,
    /const assembly = readJsonIfExists|\.sdkwork-assembly\.json/,
    'SDK family metadata checks should use sdk-manifest.json as the family SSOT, not retired assembly files'
  );
  for (const requiredText of [
    'export function validateSdkFamilyMetadata',
    'sdk-manifest.json',
    'package sdkwork.sdkDependencies',
    'component contracts.sdkDependencies',
    'assertDependencyList',
    'assertNoGeneratedOwnershipStandardKeys'
  ]) {
    assert.ok(
      sdkFamilyMetadata.includes(requiredText),
      `SDK family metadata module should include ${requiredText}`
    );
  }
});

test('agent SDK workspace validator splits OpenAPI checks into a focused module', () => {
  const validatorPath = path.join(
    root,
    'tools',
    'validators',
    'agent-sdk-workspace',
    'check-agent-sdk-workspace.mjs'
  );
  const openApiChecksPath = path.join(
    root,
    'tools',
    'validators',
    'agent-sdk-workspace',
    'openapi-checks.mjs'
  );

  assert.equal(fs.existsSync(openApiChecksPath), true, 'OpenAPI validator module should exist');

  const validator = fs.readFileSync(validatorPath, 'utf8');
  assert.match(
    validator,
    /from '\.\/openapi-checks\.mjs'/,
    'agent SDK workspace validator should import OpenAPI helpers'
  );
  assert.doesNotMatch(
    validator,
    /function validateOpenApi\b/,
    'agent SDK workspace validator should not inline OpenAPI validation'
  );
  assert.doesNotMatch(
    validator,
    /function validateOpenApiOwnership\b/,
    'agent SDK workspace validator should not inline OpenAPI ownership validation'
  );
  assert.doesNotMatch(
    validator,
    /function titleKind\b/,
    'agent SDK workspace validator should not inline OpenAPI title-kind mapping'
  );

  const openApiChecks = fs.readFileSync(openApiChecksPath, 'utf8');
  for (const requiredText of [
    'export function validateOpenApi',
    'validateOpenApiOwnership',
    'titleKind',
    'forbiddenAgentApiPrefixesFor',
    'X-Request-Id',
    'unsupported agent RAG lifecycle contract'
  ]) {
    assert.ok(openApiChecks.includes(requiredText), `OpenAPI module should include ${requiredText}`);
  }
});

test('agent SDK workspace validator splits generated TypeScript API surface checks into a focused module', () => {
  const validatorPath = path.join(
    root,
    'tools',
    'validators',
    'agent-sdk-workspace',
    'check-agent-sdk-workspace.mjs'
  );
  const generatedTypeScriptApiSurfacePath = path.join(
    root,
    'tools',
    'validators',
    'agent-sdk-workspace',
    'generated-typescript-api-surface-checks.mjs'
  );

  assert.equal(
    fs.existsSync(generatedTypeScriptApiSurfacePath),
    true,
    'generated TypeScript API surface validator module should exist'
  );

  const validator = fs.readFileSync(validatorPath, 'utf8');
  assert.match(
    validator,
    /from '\.\/generated-typescript-api-surface-checks\.mjs'/,
    'agent SDK workspace validator should import generated TypeScript API surface helpers'
  );
  assert.doesNotMatch(
    validator,
    /function validateGeneratedAgentApi\b/,
    'agent SDK workspace validator should not inline generated TypeScript API surface validation'
  );
  assert.doesNotMatch(
    validator,
    /function usesScopeFreeCallSurface\b/,
    'agent SDK workspace validator should not inline call surface scope mapping'
  );

  const generatedTypeScriptApiSurface = fs.readFileSync(generatedTypeScriptApiSurfacePath, 'utf8');
  for (const requiredText of [
    'export function validateGeneratedAgentApi',
    'AiMemoryStoresApi',
    'AiKnowledgeBasesApi',
    'memoryStores',
    'knowledgeBases',
    'usesScopeFreeCallSurface'
  ]) {
    assert.ok(
      generatedTypeScriptApiSurface.includes(requiredText),
      `generated TypeScript API surface module should include ${requiredText}`
    );
  }
});

function expectedSdkComponentSurface(relativePath, manifest) {
  const declaredSurface = manifest.sdk?.sdkSurface ?? manifest.sdk?.sdkType;
  if (declaredSurface === 'internal') {
    return 'internal-api';
  }
  if (declaredSurface === 'open' || declaredSurface === 'custom') {
    return 'open-api';
  }
  if (declaredSurface === 'app') {
    return 'app-api';
  }
  if (declaredSurface === 'backend') {
    return 'backend-admin';
  }

  const componentName = manifest.component?.name ?? '';
  const normalizedPath = relativePath.replaceAll('\\', '/');
  const surfaceSource = `${componentName} ${normalizedPath}`;
  if (surfaceSource.includes('-internal-sdk')) {
    return 'internal-api';
  }
  if (surfaceSource.includes('-backend-sdk')) {
    return 'backend-admin';
  }
  if (surfaceSource.includes('-app-sdk')) {
    return 'app-api';
  }

  return 'open-api';
}

function listCurrentDictionaryFiles() {
  const roots = [
    path.join(root, 'README.md'),
    path.join(root, 'AGENTS.md'),
    path.join(root, 'specs'),
    path.join(root, 'sdkwork-agent-kernel', 'README.md'),
    path.join(root, 'sdkwork-agent-kernel', 'specs'),
    path.join(root, 'sdkwork-code-kernel', 'README.md'),
    path.join(root, 'sdkwork-code-kernel', 'specs'),
    path.join(root, 'sdkwork-kernel-plugins', 'README.md'),
    path.join(root, 'sdkwork-kernel-plugins', 'specs')
  ];

  return roots.flatMap((scanRoot) => listDictionaryFiles(scanRoot));
}

function listDictionaryFiles(scanPath) {
  if (!fs.existsSync(scanPath)) {
    return [];
  }

  const stat = fs.statSync(scanPath);
  if (stat.isFile()) {
    return isDictionaryFile(scanPath) ? [scanPath] : [];
  }

  return fs.readdirSync(scanPath, { withFileTypes: true }).flatMap((entry) => {
    const entryPath = path.join(scanPath, entry.name);
    if (entry.isDirectory()) {
      if (entry.name === 'node_modules' || entry.name === 'dist' || entry.name === 'generated') {
        return [];
      }
      return listDictionaryFiles(entryPath);
    }

    return isDictionaryFile(entryPath) ? [entryPath] : [];
  });
}

function isDictionaryFile(filePath) {
  return /(?:README\.md|component\.spec\.json|AGENTS?\.md|.*_SPEC\.md)$/.test(path.basename(filePath));
}

function listComponentSpecFiles(scanPath) {
  if (!fs.existsSync(scanPath)) {
    return [];
  }

  const stat = fs.statSync(scanPath);
  if (stat.isFile()) {
    return path.basename(scanPath) === 'component.spec.json' ? [scanPath] : [];
  }

  return fs.readdirSync(scanPath, { withFileTypes: true }).flatMap((entry) => {
    const entryPath = path.join(scanPath, entry.name);
    if (entry.isDirectory()) {
      if (
        entry.name === '.git' ||
        entry.name === 'node_modules' ||
        entry.name === 'dist' ||
        entry.name === 'generated' ||
        entry.name === 'target' ||
        entry.name === 'external'
      ) {
        return [];
      }
      return listComponentSpecFiles(entryPath);
    }

    return entry.name === 'component.spec.json' ? [entryPath] : [];
  });
}

function listCargoTomlFiles(scanPath) {
  if (!fs.existsSync(scanPath)) {
    return [];
  }

  const stat = fs.statSync(scanPath);
  if (stat.isFile()) {
    return path.basename(scanPath) === 'Cargo.toml' ? [scanPath] : [];
  }

  return fs.readdirSync(scanPath, { withFileTypes: true }).flatMap((entry) => {
    const entryPath = path.join(scanPath, entry.name);
    if (entry.isDirectory()) {
      if (
        entry.name === '.git' ||
        entry.name === 'node_modules' ||
        entry.name === 'dist' ||
        entry.name === 'build' ||
        entry.name === 'target' ||
        entry.name === 'external'
      ) {
        return [];
      }
      return listCargoTomlFiles(entryPath);
    }

    return entry.name === 'Cargo.toml' ? [entryPath] : [];
  });
}

function resolveComponentRoot(manifestPath, manifest) {
  const componentRoot = manifest.component?.root;
  if (!componentRoot) {
    return path.dirname(path.dirname(manifestPath));
  }

  const normalizedRoot = componentRoot.replaceAll('\\', '/');
  const repositoryName = path.basename(root);
  const candidates = [
    path.resolve(root, normalizedRoot),
    normalizedRoot.startsWith(`${repositoryName}/`)
      ? path.resolve(path.dirname(root), normalizedRoot)
      : null,
    path.dirname(path.dirname(manifestPath))
  ].filter(Boolean);

  return candidates.find((candidate) => fs.existsSync(candidate)) ?? candidates[0];
}
