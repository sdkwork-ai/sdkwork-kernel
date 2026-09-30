# sdkwork-agent-internal-sdk

Generated SDKWork v3 API-key open-api transport SDK.

## Installation

```bash
npm install @sdkwork/agent-internal-sdk
# or
yarn add @sdkwork/agent-internal-sdk
# or
pnpm add @sdkwork/agent-internal-sdk
```

## Quick Start

```typescript
import { SdkworkCustomClient } from '@sdkwork/agent-internal-sdk';

const client = new SdkworkCustomClient({
  baseUrl: 'http://localhost:8080',
  timeout: 30000,
});

client.setApiKey('your-api-key');

// Use the SDK
const xSdkworkTenantId = 'x-sdkwork-tenant-id';
const xSdkworkUserId = 'x-sdkwork-user-id';
const xSdkworkIdentityMac = 'x-sdkwork-identity-mac';
const params = {
  xSdkworkTenantId,
  xSdkworkUserId,
  xSdkworkIdentityMac,
};
const result = await client.intelligence.runtime.manifest.get(params);
```

## Authentication

```text
X-API-Key: <apiKey>
```

Configure API key credentials through the generated client API:

```typescript
client.setApiKey('your-api-key');
```


## Configuration (Non-Auth)

```typescript
import { SdkworkCustomClient } from '@sdkwork/agent-internal-sdk';

const client = new SdkworkCustomClient({
  baseUrl: 'http://localhost:8080',
  timeout: 30000, // Request timeout in ms
  headers: {      // Custom headers
    'X-Custom-Header': 'value',
  },
});
```

## API Modules

- `client.intelligence` - intelligence API

## Usage Examples

### intelligence

```typescript
// Load runtime capability manifest
const xSdkworkTenantId = 'x-sdkwork-tenant-id';
const xSdkworkUserId = 'x-sdkwork-user-id';
const xSdkworkIdentityMac = 'x-sdkwork-identity-mac';
const params = {
  xSdkworkTenantId,
  xSdkworkUserId,
  xSdkworkIdentityMac,
};
const result = await client.intelligence.runtime.manifest.get(params);
```

## Error Handling

```typescript
import { SdkworkCustomClient, NetworkError, TimeoutError, AuthenticationError } from '@sdkwork/agent-internal-sdk';

try {
  const xSdkworkTenantId = 'x-sdkwork-tenant-id';
  const xSdkworkUserId = 'x-sdkwork-user-id';
  const xSdkworkIdentityMac = 'x-sdkwork-identity-mac';
  const params = {
    xSdkworkTenantId,
    xSdkworkUserId,
    xSdkworkIdentityMac,
  };
  const result = await client.intelligence.runtime.manifest.get(params);
} catch (error) {
  if (error instanceof AuthenticationError) {
    console.error('Authentication failed:', error.message);
  } else if (error instanceof TimeoutError) {
    console.error('Request timed out:', error.message);
  } else if (error instanceof NetworkError) {
    console.error('Network error:', error.message);
  } else {
    throw error;
  }
}
```

## Publishing

This SDK includes cross-platform publish scripts in `bin/`:
- `bin/publish-core.mjs`
- `bin/publish.sh`
- `bin/publish.ps1`

TypeScript check and publish commands materialize workspace dependency versions in a temporary tarball with pnpm. They reject local-only dependency protocols before npm publication and do not rewrite the source `package.json`.

### Check

```bash
./bin/publish.sh --action check
```

### Publish

```bash
./bin/publish.sh --action publish --channel release
```

```powershell
.\bin\publish.ps1 --action publish --channel test --dry-run
```

> Set `NPM_TOKEN` (and optional `NPM_REGISTRY_URL`) before release publish.

## License

MIT

## Regeneration Contract

- HTTP/OpenAPI generator-owned files are tracked in `.sdkwork/sdkwork-generator-manifest.json`.
- HTTP/OpenAPI generation also writes `.sdkwork/sdkwork-generator-changes.json` so automation can inspect created, updated, deleted, unchanged, scaffolded, and backed-up files plus the classified impact areas, verification plan, and execution decision for the latest generation.
- HTTP/OpenAPI apply mode also writes `.sdkwork/sdkwork-generator-report.json` with the full execution report, including `schemaVersion`, `generator`, stable artifact paths, and the execution handoff commands that match CLI `--json` output.
- CLI JSON output also includes an execution handoff with concrete next commands, including reviewed apply commands for dry-run flows.
- Put HTTP/OpenAPI hand-written wrappers, adapters, and orchestration in `custom/`.
- Files scaffolded under `custom/` are created once and preserved across HTTP/OpenAPI regenerations.
- If an HTTP/OpenAPI generated-owned file was modified locally, its previous content is copied to `.sdkwork/manual-backups/` before overwrite or removal.
- RPC SDK source workspaces use convention-first evidence by default: RPC SDK family naming, language workspace naming, `rpc/*.manifest.json`, proto source references, generated client source, and native package manifests.
- Use `sdkgen inspect --protocol rpc` to verify RPC convention evidence. Request persisted generator evidence only with `--emit-control-plane` for release, CI, audit, or migration workflows; evidence paths are derived by generator convention.
