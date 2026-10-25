import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

const MAX_GLOB_SCAN_ENTRIES = 20_000;
const EVIDENCE_FILE_PATTERNS = [
  /\.sha256$/u,
  /\.cyclonedx\.json$/u,
  /(^|[/\\])signing-policy\.json$/u,
];

export function readJsonFile(filePath) {
  return JSON.parse(fs.readFileSync(filePath, 'utf8'));
}

export function readReleaseContext(kernelRoot, env = process.env) {
  const manifest = readJsonFile(path.join(kernelRoot, 'agent.app.config.json'));
  const workflow = readJsonFile(path.join(kernelRoot, 'sdkwork.workflow.json'));
  const version =
    env.SDKWORK_PACKAGE_VERSION?.trim() ||
    env.SDKWORK_RELEASE_VERSION?.trim() ||
    manifest.release?.currentVersion ||
    workflow.release?.defaultVersion?.trim?.() ||
    '0.0.0';
  const matrix = validateReleaseTargetMatrix({ manifest, workflow, version });
  return { manifest, workflow, version, ...matrix };
}

export function releaseDirFor(kernelRoot, packageId) {
  return path.join(kernelRoot, 'dist', 'release', packageId);
}

export function sbomPathFor(kernelRoot, packageId, version) {
  return path.join(releaseDirFor(kernelRoot, packageId), `${packageId}-${version}.cyclonedx.json`);
}

export function checksumPathFor(kernelRoot, packageId, version) {
  return path.join(releaseDirFor(kernelRoot, packageId), `${packageId}-${version}.sha256`);
}

export function releaseArtifactName({ workflow, packageId, version, format }) {
  const artifactPrefix = workflow.release?.artifactPrefix ?? workflow.app?.id ?? 'sdkwork-kernel';
  return `${artifactPrefix}-${packageId}-${version}.${format}`;
}

export function releaseArtifactPathFor({ kernelRoot, workflow, packageId, version, format }) {
  return path.join(
    releaseDirFor(kernelRoot, packageId),
    releaseArtifactName({ workflow, packageId, version, format }),
  );
}

export function packageFormatToManifestFormat(format) {
  return String(format).toUpperCase().replace(/[^A-Z0-9]+/gu, '_').replace(/^_+|_+$/gu, '');
}

export function normalizeFormatToken(format) {
  return String(format).toLowerCase().replace(/[^a-z0-9]+/gu, '-').replace(/^-+|-+$/gu, '');
}

function isEnabledPackage(pkg) {
  return pkg?.enabled !== false;
}

function enabledManifestPackages(manifest) {
  return (manifest.artifacts?.installConfig?.packages ?? []).filter(isEnabledPackage);
}

function packageIdForWorkflowTarget(target) {
  if (typeof target?.packageId === 'string' && target.packageId.trim()) {
    return target.packageId.trim();
  }
  if (Array.isArray(target?.formats) && target.formats.length === 1) {
    return String(target.id ?? '').trim();
  }
  return '';
}

function canonicalPackageIdForTarget(target) {
  if (!target || !Array.isArray(target.formats) || target.formats.length !== 1) {
    return '';
  }
  const format = target.formats[0];
  const formatToken = normalizeFormatToken(format);
  const prefix =
    target.platform === 'linux' && ['deb', 'rpm'].includes(format)
      ? `linux-${target.distribution}`
      : target.platform;
  const parts = [
    prefix,
    target.architecture,
    target.deploymentProfile,
    target.profile,
    ...(target.variant ? [target.variant] : []),
    formatToken,
  ];
  return parts.filter(Boolean).join('-');
}

function compareField(errors, packageId, sourceLabel, sourceValue, targetLabel, targetValue) {
  if (sourceValue !== targetValue) {
    errors.push(
      `${packageId} ${sourceLabel} (${sourceValue ?? 'missing'}) must match ${targetLabel} (${targetValue ?? 'missing'})`,
    );
  }
}

function releaseNoteForVersion(manifest, version) {
  const notes = manifest.release?.notes ?? [];
  return (
    notes.find((note) => note.version === version) ??
    notes.find((note) => note.current === true) ??
    notes[0]
  );
}

export function validateReleaseTargetMatrix({ manifest, workflow, version }) {
  const errors = [];
  const packageById = new Map();
  const targetByPackageId = new Map();
  const packages = enabledManifestPackages(manifest);
  const workflowTargets = workflow.targets ?? [];

  if (packages.length === 0) {
    errors.push('agent.app.config.json artifacts.installConfig.packages must declare at least one enabled package');
  }

  for (const pkg of packages) {
    if (!pkg.id) {
      errors.push('agent.app.config.json contains an enabled package without id');
      continue;
    }
    if (packageById.has(pkg.id)) {
      errors.push(`duplicate agent.app.config.json package id: ${pkg.id}`);
      continue;
    }
    packageById.set(pkg.id, pkg);
  }

  for (const target of workflowTargets) {
    const packageId = packageIdForWorkflowTarget(target);
    if (!packageId) {
      errors.push(`workflow target ${target?.id ?? '<unknown>'} must resolve to exactly one package id`);
      continue;
    }
    const canonicalPackageId = canonicalPackageIdForTarget(target);
    if (canonicalPackageId && packageId !== canonicalPackageId) {
      errors.push(`workflow target ${target.id} package id must be canonical: ${canonicalPackageId}`);
    }
    if (targetByPackageId.has(packageId)) {
      errors.push(`duplicate sdkwork.workflow.json package id: ${packageId}`);
      continue;
    }
    targetByPackageId.set(packageId, target);
  }

  const releaseNote = releaseNoteForVersion(manifest, version);
  if (!releaseNote) {
    errors.push(`agent.app.config.json release.notes must include a current note for ${version}`);
  } else {
    const notePackageIds = new Set(releaseNote.packageIds ?? []);
    for (const packageId of packageById.keys()) {
      if (!notePackageIds.has(packageId)) {
        errors.push(`release note ${releaseNote.version ?? '<unknown>'} must include package id ${packageId}`);
      }
    }
    for (const packageId of notePackageIds) {
      if (!packageById.has(packageId)) {
        errors.push(`release note references unknown package id ${packageId}`);
      }
    }
  }

  for (const [packageId, pkg] of packageById.entries()) {
    const target = targetByPackageId.get(packageId);
    if (!target) {
      errors.push(`sdkwork.workflow.json targets must include manifest package id ${packageId}`);
      continue;
    }
    compareField(errors, packageId, 'deploymentProfile', pkg.deploymentProfile, 'target deploymentProfile', target.deploymentProfile);
    compareField(errors, packageId, 'runtimeTarget', pkg.runtimeTarget, 'target runtimeTarget', target.runtimeTarget);
    compareField(errors, packageId, 'architecture', pkg.architecture, 'target architecture', target.architecture);

    if (Array.isArray(target.formats) && target.formats.length === 1) {
      compareField(
        errors,
        packageId,
        'packageFormat',
        pkg.packageFormat,
        'target format',
        packageFormatToManifestFormat(target.formats[0]),
      );
    }
  }

  for (const packageId of targetByPackageId.keys()) {
    if (!packageById.has(packageId)) {
      errors.push(`sdkwork.workflow.json target ${packageId} must exist in agent.app.config.json install packages`);
    }
  }

  return { errors, packageById, targetByPackageId };
}

export function resolveLifecycleTarget({ kernelRoot, env = process.env, commandName }) {
  const context = readReleaseContext(kernelRoot, env);
  const explicitPackageId = env.SDKWORK_PACKAGE_ID?.trim();
  const packageIds = [...context.packageById.keys()];
  const errors = [...context.errors];

  if (!explicitPackageId && packageIds.length !== 1) {
    errors.push(`${commandName} requires SDKWORK_PACKAGE_ID because ${packageIds.length} release packages are declared`);
  }

  const packageId = explicitPackageId || packageIds[0];
  const packageInfo = context.packageById.get(packageId);
  const target = context.targetByPackageId.get(packageId);
  if (packageId && !packageInfo) {
    errors.push(`SDKWORK_PACKAGE_ID ${packageId} is not declared in agent.app.config.json install packages`);
  }
  if (packageId && !target) {
    errors.push(`SDKWORK_PACKAGE_ID ${packageId} is not declared in sdkwork.workflow.json targets`);
  }

  if (errors.length > 0) {
    throw new Error(errors.join('\n'));
  }

  return { ...context, packageId, packageInfo, target };
}

export function selectedValidationPackageIds(context, env = process.env) {
  const explicitPackageId = env.SDKWORK_PACKAGE_ID?.trim();
  if (explicitPackageId && explicitPackageId !== 'aggregate-release') {
    return [explicitPackageId];
  }
  return [...context.packageById.keys()];
}

export function releaseBinaryPathFor(kernelRoot, platform) {
  const suffix = platform === 'windows' ? '.exe' : '';
  return path.join(kernelRoot, 'target', 'release', `sdkwork-agent-server${suffix}`);
}

export function payloadFilesFromOutputGlobs(kernelRoot, outputGlobs) {
  const payloadGlobs = (outputGlobs ?? []).filter((glob) => !isEvidenceFile(glob));
  const { files, errors } = findFilesForOutputGlobs(kernelRoot, payloadGlobs);
  return {
    files,
    errors,
  };
}

export function isEvidenceFile(filePath) {
  return EVIDENCE_FILE_PATTERNS.some((pattern) => pattern.test(filePath));
}

export function findFilesForOutputGlobs(kernelRoot, outputGlobs) {
  const files = [];
  const errors = [];
  const seen = new Set();

  for (const glob of outputGlobs ?? []) {
    const matches = resolveOutputGlob(kernelRoot, glob);
    if (matches.errors.length > 0) {
      errors.push(...matches.errors);
    }
    if (matches.files.length === 0) {
      errors.push(`output glob matched no files: ${glob}`);
    }
    for (const filePath of matches.files) {
      if (!seen.has(filePath)) {
        seen.add(filePath);
        files.push(filePath);
      }
    }
  }

  return { files, errors };
}

function resolveOutputGlob(kernelRoot, glob) {
  const normalizedGlob = String(glob).replace(/\\/gu, '/');
  if (!/[?*[\]]/u.test(normalizedGlob)) {
    const absolutePath = path.join(kernelRoot, normalizedGlob);
    if (fs.existsSync(absolutePath) && fs.statSync(absolutePath).isFile()) {
      return { files: [absolutePath], errors: [] };
    }
    return { files: [], errors: [] };
  }

  const baseDir = globBaseDir(kernelRoot, normalizedGlob);
  if (!fs.existsSync(baseDir)) {
    return { files: [], errors: [] };
  }

  const pattern = globToRegExp(normalizedGlob);
  const files = [];
  const errors = [];
  let visited = 0;

  function walk(dirPath) {
    visited += 1;
    if (visited > MAX_GLOB_SCAN_ENTRIES) {
      errors.push(`output glob scan exceeded ${MAX_GLOB_SCAN_ENTRIES} filesystem entries for ${glob}`);
      return;
    }
    for (const entry of fs.readdirSync(dirPath, { withFileTypes: true })) {
      const childPath = path.join(dirPath, entry.name);
      if (entry.isDirectory()) {
        walk(childPath);
      } else if (entry.isFile()) {
        const relativePath = path.relative(kernelRoot, childPath).replace(/\\/gu, '/');
        if (pattern.test(relativePath)) {
          files.push(childPath);
        }
      }
    }
  }

  walk(baseDir);
  return { files, errors };
}

function globBaseDir(kernelRoot, normalizedGlob) {
  const firstGlobIndex = normalizedGlob.search(/[?*[\]]/u);
  const prefix = firstGlobIndex === -1 ? normalizedGlob : normalizedGlob.slice(0, firstGlobIndex);
  const prefixDir = prefix.endsWith('/') ? prefix : path.dirname(prefix);
  return path.join(kernelRoot, prefixDir === '.' ? '' : prefixDir);
}

function globToRegExp(glob) {
  let pattern = '^';
  for (let index = 0; index < glob.length; index += 1) {
    const char = glob[index];
    const next = glob[index + 1];
    if (char === '*' && next === '*') {
      pattern += '.*';
      index += 1;
    } else if (char === '*') {
      pattern += '[^/]*';
    } else if (char === '?') {
      pattern += '[^/]';
    } else {
      pattern += escapeRegExp(char);
    }
  }
  pattern += '$';
  return new RegExp(pattern, 'u');
}

function escapeRegExp(value) {
  return value.replace(/[|\\{}()[\]^$+*?.]/gu, '\\$&');
}

export async function sha256File(filePath) {
  const hash = createHash('sha256');
  await new Promise((resolve, reject) => {
    const stream = fs.createReadStream(filePath);
    stream.on('data', (chunk) => hash.update(chunk));
    stream.on('error', reject);
    stream.on('end', resolve);
  });
  return hash.digest('hex');
}

export async function validateChecksumFile({ checksumPath, payloadFiles }) {
  const errors = [];
  if (!fs.existsSync(checksumPath)) {
    errors.push(`missing checksum file: ${checksumPath}`);
    return errors;
  }

  const payloadByName = new Map();
  for (const filePath of payloadFiles) {
    const basename = path.basename(filePath);
    if (payloadByName.has(basename)) {
      errors.push(`duplicate payload basename prevents checksum validation: ${basename}`);
    }
    payloadByName.set(basename, filePath);
  }

  const lines = fs
    .readFileSync(checksumPath, 'utf8')
    .split(/\r?\n/u)
    .map((line) => line.trim())
    .filter(Boolean);
  if (lines.length === 0) {
    errors.push(`checksum file is empty: ${checksumPath}`);
    return errors;
  }

  const checkedPayloads = new Set();
  for (const line of lines) {
    const match = /^([a-fA-F0-9]{64})\s+\*?(.+)$/u.exec(line);
    if (!match) {
      errors.push(`invalid checksum line in ${checksumPath}: ${line}`);
      continue;
    }
    const expectedDigest = match[1].toLowerCase();
    const basename = path.basename(match[2].trim());
    const payloadPath = payloadByName.get(basename);
    if (!payloadPath) {
      errors.push(`checksum file references non-output payload: ${basename}`);
      continue;
    }
    const actualDigest = await sha256File(payloadPath);
    if (actualDigest !== expectedDigest) {
      errors.push(`checksum mismatch for ${basename}`);
    }
    checkedPayloads.add(basename);
  }

  for (const filePath of payloadFiles) {
    const basename = path.basename(filePath);
    if (!checkedPayloads.has(basename)) {
      errors.push(`checksum file does not cover output payload: ${basename}`);
    }
  }

  return errors;
}

export function validateSbomFile({ sbomPath, packageId, version }) {
  const errors = [];
  if (!fs.existsSync(sbomPath)) {
    errors.push(`missing SBOM: ${sbomPath}`);
    return errors;
  }
  let sbom;
  try {
    sbom = readJsonFile(sbomPath);
  } catch (error) {
    errors.push(`invalid SBOM JSON ${sbomPath}: ${error.message}`);
    return errors;
  }
  if (sbom.bomFormat !== 'CycloneDX') {
    errors.push(`SBOM must be CycloneDX: ${sbomPath}`);
  }
  if (sbom.metadata?.component?.name !== packageId) {
    errors.push(`SBOM metadata.component.name must be ${packageId}`);
  }
  if (sbom.metadata?.component?.version !== version) {
    errors.push(`SBOM metadata.component.version must be ${version}`);
  }
  return errors;
}

export function normalizeSbomMetadata({ sbomPath, packageId, version }) {
  const sbom = readJsonFile(sbomPath);
  sbom.metadata = sbom.metadata ?? {};
  sbom.metadata.component = {
    ...(sbom.metadata.component ?? {}),
    type: 'application',
    'bom-ref': `pkg:generic/${packageId}@${version}`,
    name: packageId,
    version,
    purl: `pkg:generic/${packageId}@${version}`,
  };
  fs.writeFileSync(sbomPath, `${JSON.stringify(sbom, null, 2)}\n`, 'utf8');
}
