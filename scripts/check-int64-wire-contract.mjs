#!/usr/bin/env node
// Int64 wire contract checker (API_SPEC §13.6).
//
// Every OpenAPI property with `format: int64` MUST be declared as
// `type: string` + decimal `pattern` + `x-sdkwork-int64-string: true`.
// `type: integer` + `format: int64` is a contract violation: generated
// TypeScript SDKs would emit `number` and browsers silently round ids past
// 2^53. The sibling checker `check-api-operation-patterns.mjs` does not
// enforce this rule yet, so the kernel validates it locally for its authority
// OpenAPI and every derived SDK snapshot.

import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';

const REQUIRED_MARKERS = ['type: string', "pattern: '^-?[0-9]+$'", 'x-sdkwork-int64-string: true'];

const files = [
  'apis/internal-api/intelligence/sdkwork-agent-internal-api.openapi.yaml',
  'sdks/sdkwork-agent-internal-sdk/openapi/sdkwork-agent-internal-api.openapi.yaml',
  'sdks/sdkwork-agent-internal-sdk/openapi/sdkwork-agent-internal-api.sdkgen.yaml'
];

const root = process.cwd();
const failures = [];

function propertyHeaderLine(line) {
  const match = /^(\s*)([A-Za-z0-9_-]+):\s*$/.exec(line);
  if (!match) {
    return null;
  }
  return { indent: match[1].length, name: match[2] };
}

function propertyBlockEnd(lines, startIndex, propertyIndent) {
  let end = lines.length;
  for (let index = startIndex + 1; index < lines.length; index += 1) {
    const line = lines[index];
    if (line.trim() === '') {
      continue;
    }
    const header = propertyHeaderLine(line);
    const indent = line.length - line.trimStart().length;
    if (header && header.indent <= propertyIndent) {
      end = index;
      break;
    }
    if (!header && indent <= propertyIndent && line.trimStart().startsWith('- ')) {
      // Sequence items belong to the property block; keep scanning.
      continue;
    }
  }
  return end;
}

function scanFile(relativePath) {
  const absolutePath = path.join(root, relativePath);
  if (!fs.existsSync(absolutePath)) {
    failures.push(`${relativePath}: file not found`);
    return;
  }
  const lines = fs.readFileSync(absolutePath, 'utf8').split(/\r?\n/);
  const headers = [];
  for (let index = 0; index < lines.length; index += 1) {
    const header = propertyHeaderLine(lines[index]);
    if (header) {
      headers.push({ ...header, index });
    }
  }
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    if (!/^\s*format:\s*int64\s*$/.test(line)) {
      continue;
    }
    const indent = line.length - line.trimStart().length;
    const owner = headers
      .filter((header) => header.indent === indent - 2 && header.index < index)
      .pop();
    if (!owner) {
      failures.push(`${relativePath}:${index + 1}: format: int64 without an owning property`);
      continue;
    }
    const blockEnd = propertyBlockEnd(lines, index, indent - 2);
    const block = lines.slice(owner.index, blockEnd).join('\n');
    for (const marker of REQUIRED_MARKERS) {
      if (!block.includes(marker)) {
        failures.push(
          `${relativePath}:${owner.index + 1}: property "${owner.name}" with format: int64 is missing "${marker}" (API_SPEC §13.6)`
        );
      }
    }
    if (/^\s*type:\s*integer\s*$/m.test(block)) {
      failures.push(
        `${relativePath}:${owner.index + 1}: property "${owner.name}" declares type: integer with format: int64 — int64 fields MUST be type: string (API_SPEC §13.6)`
      );
    }
  }
}

for (const file of files) {
  scanFile(file);
}

if (failures.length > 0) {
  for (const failure of failures) {
    console.error(`[int64-wire-contract] ${failure}`);
  }
  console.error(`int64 wire contract check FAILED with ${failures.length} violation(s)`);
  process.exit(1);
}
console.log('int64 wire contract check passed (3 OpenAPI files)');
