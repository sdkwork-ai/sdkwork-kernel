#!/usr/bin/env node

import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import {
  checksumPathFor,
  payloadFilesFromOutputGlobs,
  readReleaseContext,
  sbomPathFor,
  selectedValidationPackageIds,
  validateChecksumFile,
  validateSbomFile,
} from './kernel-release-targets.mjs';

const kernelRoot = process.cwd();
const errors = [];
const context = readReleaseContext(kernelRoot);
const { manifest, version } = context;

errors.push(...context.errors);

const topologySpec = manifest.metadata?.topologySpec;
if (topologySpec !== 'specs/topology.spec.json') {
  errors.push('agent.app.config.json metadata.topologySpec must reference specs/topology.spec.json');
}
for (const [envName, envConfig] of Object.entries(manifest.environments ?? {})) {
  if (!envConfig?.topologyProfileId) {
    errors.push(`agent.app.config.json environments.${envName} must declare topologyProfileId`);
  }
  if (envConfig?.accessUrl) {
    errors.push(`agent.app.config.json environments.${envName} must use accessUrlEnv instead of accessUrl`);
  }
}

for (const packageId of selectedValidationPackageIds(context)) {
  const target = context.targetByPackageId.get(packageId);
  const packageInfo = context.packageById.get(packageId);
  if (!target || !packageInfo) {
    errors.push(`SDKWORK_PACKAGE_ID ${packageId} must be declared in both manifest and workflow targets`);
    continue;
  }

  const payloads = payloadFilesFromOutputGlobs(kernelRoot, target.outputGlobs);
  errors.push(...payloads.errors);
  if (payloads.files.length === 0) {
    errors.push(`missing release payload for package ${packageId}`);
  }

  const expectedExtension = target.formats?.[0];
  if (expectedExtension) {
    for (const filePath of payloads.files) {
      if (!filePath.endsWith(`.${expectedExtension}`)) {
        errors.push(
          `release payload ${path.relative(kernelRoot, filePath)} must match package format ${expectedExtension}`,
        );
      }
    }
  }

  const sbomPath = sbomPathFor(kernelRoot, packageId, version);
  const checksumPath = checksumPathFor(kernelRoot, packageId, version);

  if (manifest.security?.sbomRequired) {
    errors.push(...validateSbomFile({ sbomPath, packageId, version }));
  }
  if (manifest.security?.checksumRequired) {
    errors.push(
      ...(await validateChecksumFile({
        checksumPath,
        payloadFiles: payloads.files,
      })),
    );
  }
}

if (errors.length > 0) {
  for (const error of errors) {
    console.error(`RELEASE VALIDATION: ${error}`);
  }
  process.exit(1);
}

console.log('Release artifact validation passed.');
