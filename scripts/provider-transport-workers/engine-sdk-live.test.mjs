// WORKSPACE-PATH:allow-fixture: fixtures name a foreign checkout root, drive, or home directory to exercise path handling, so the literal is the value under assertion rather than a binding this build resolves
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  buildCodexKernelStreamEvent,
  buildStubModelChatResult,
  invokeModelChatLive,
  invokeModelChatStreamLive,
  invokeSessionControlRuntime,
  invokeSessionDiscoveryRuntime,
  mockProviderInvocationAllowed,
  probePackage,
  probeModelChatRuntime,
  resolveModelChatPrompt,
  resolvePackageExportSpecifier,
  resolvePackageSpecifier,
  VERIFIED_PROVIDER_SESSION_ID,
} from './engine-sdk-live.mjs';

const workerDir = path.dirname(fileURLToPath(import.meta.url));
const birdcoderRoot = path.resolve(workerDir, '../../../sdkwork-birdcoder');
const kernelRoot = path.resolve(workerDir, '../..');
const tempRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'sdkwork-engine-sdk-live-'));
const opencodeSdkMirror = path.join(tempRoot, 'opencode-sdk');
const claudeSdkMirror = path.join(tempRoot, 'claude-sdk');
const geminiSdkMirror = path.join(tempRoot, 'gemini-sdk');
const invalidSdkMirror = path.join(tempRoot, 'invalid-sdk');
const codexSdkMirror = path.join(tempRoot, 'codex-sdk');
const openaiSdkMirror = path.join(tempRoot, 'openai-sdk');
const claudeCapturePath = path.join(tempRoot, 'claude-sdk-capture.json');
const claudeDiscoveryCapturePath = path.join(tempRoot, 'claude-sdk-discovery-capture.json');
const codexCapturePath = path.join(tempRoot, 'codex-sdk-capture.json');
const geminiCapturePath = path.join(tempRoot, 'gemini-sdk-capture.json');
const openaiCapturePath = path.join(tempRoot, 'openai-sdk-capture.json');
const opencodeCapturePath = path.join(tempRoot, 'opencode-sdk-capture.json');
fs.mkdirSync(opencodeSdkMirror, { recursive: true });
fs.writeFileSync(
  path.join(opencodeSdkMirror, 'package.json'),
  JSON.stringify({
    type: 'module',
    name: '@opencode-ai/sdk',
    version: '0.0.0-test',
    exports: { '.': './index.js', './v2': './v2.js' },
  }),
  'utf8',
);
fs.mkdirSync(claudeSdkMirror, { recursive: true });
fs.writeFileSync(
  path.join(claudeSdkMirror, 'package.json'),
  JSON.stringify({
    type: 'module',
    name: '@anthropic-ai/claude-agent-sdk',
    version: '0.0.0-test',
    exports: { '.': './index.js' },
  }),
  'utf8',
);
fs.writeFileSync(
  path.join(claudeSdkMirror, 'index.js'),
  `import fs from 'node:fs';

const capturePath = ${JSON.stringify(claudeCapturePath)};
const discoveryCapturePath = ${JSON.stringify(claudeDiscoveryCapturePath)};

function capture(prompt, options) {
  fs.writeFileSync(capturePath, JSON.stringify({
    prompt,
    options: {
      cwd: options.cwd,
      has_abort_controller: Boolean(options.abortController),
      model: options.model,
      permission_mode: options.permissionMode,
      allow_dangerously_skip_permissions: options.allowDangerouslySkipPermissions,
      include_partial_messages: options.includePartialMessages,
      resume: options.resume,
      sandbox: options.sandbox,
    },
  }), 'utf8');
}

export function query({ prompt, options = {} }) {
  capture(prompt, options);
  const sessionId = prompt === 'mismatched Claude session'
    ? 'claude-sdk-unexpected'
    : options.resume ?? 'claude-sdk-created';
  if (prompt === 'interrupt claude turn') {
    // Signals that the in-flight turn is registered so the test can dispatch
    // the in-process session control while the query is still running.
    fs.writeFileSync(capturePath, JSON.stringify({ turn_registered: true }), 'utf8');
    const aborted = new Promise((resolve) => {
      options.abortController?.signal.addEventListener('abort', () => resolve(true), { once: true });
    });
    return (async function* () {
      yield { type: 'system', subtype: 'init', session_id: sessionId };
      await aborted;
      throw new Error('claude sdk turn aborted by session control');
    })();
  }
  return (async function* () {
    yield { type: 'system', subtype: 'init', session_id: sessionId };
    yield {
      type: 'system',
      subtype: 'session_state_changed',
      state: 'running',
      session_id: sessionId,
    };
    yield { type: 'permission_request', session_id: sessionId };
    if (options.includePartialMessages) {
      yield {
        type: 'stream_event',
        uuid: 'claude-assistant-1',
        session_id: sessionId,
        event: { type: 'message_start', message: { id: 'claude-assistant-1' } },
      };
      yield {
        type: 'stream_event',
        uuid: 'claude-assistant-1',
        session_id: sessionId,
        event: {
          type: 'content_block_start',
          index: 0,
          content_block: { type: 'text', text: '' },
        },
      };
      yield {
        type: 'stream_event',
        uuid: 'claude-assistant-1',
        session_id: sessionId,
        event: {
          type: 'content_block_delta',
          index: 0,
          delta: { type: 'text_delta', text: 'claude sdk:' },
        },
      };
      yield {
        type: 'stream_event',
        uuid: 'claude-assistant-1',
        session_id: sessionId,
        event: {
          type: 'content_block_delta',
          index: 0,
          delta: { type: 'text_delta', text: prompt },
        },
      };
      yield {
        type: 'stream_event',
        uuid: 'claude-assistant-1',
        session_id: sessionId,
        event: { type: 'content_block_stop', index: 0 },
      };
    }
    yield {
      type: 'assistant',
      uuid: 'claude-assistant-1',
      session_id: sessionId,
      message: { content: [{ type: 'text', text: 'claude sdk:' + prompt }] },
    };
    yield {
      type: 'result',
      subtype: 'success',
      is_error: false,
      result: 'claude sdk:' + prompt,
      session_id: sessionId,
    };
    yield {
      type: 'system',
      subtype: 'session_state_changed',
      state: 'idle',
      session_id: sessionId,
    };
    yield {
      type: 'prompt_suggestion',
      suggestion: 'This event must not appear after terminal completion',
      session_id: sessionId,
    };
  })();
}

export async function forkSession(sessionId, options = {}) {
  fs.writeFileSync(
    capturePath,
    JSON.stringify({ fork_session: { session_id: sessionId, options } }),
    'utf8',
  );
  return { sessionId: 'claude-sdk-forked' };
}

export async function listSessions(options = {}) {
  fs.writeFileSync(
    discoveryCapturePath,
    JSON.stringify({ method: 'listSessions', options }),
    'utf8',
  );
  const sessions = [{
    sessionId: 'claude-history-1',
    summary: 'Claude history summary',
    customTitle: 'Claude history',
    firstPrompt: 'Review the kernel',
    lastModified: Date.parse('2026-01-03T03:04:05.000Z'),
    createdAt: Date.parse('2026-01-01T01:02:03.000Z'),
    cwd: 'C:/sdkwork/claude-workspace',
    gitBranch: 'main',
    tag: 'commercial',
  }, {
    sessionId: 'claude-history-2',
    summary: 'Claude second page boundary',
    lastModified: Date.parse('2026-01-02T03:04:05.000Z'),
  }, {
    sessionId: 'claude-history-3',
    summary: 'Claude final session',
    lastModified: Date.parse('2026-01-01T03:04:05.000Z'),
    cwd: 'C:/sdkwork/claude-workspace',
  }];
  const offset = options.offset ?? 0;
  return sessions.slice(offset, offset + (options.limit ?? sessions.length));
}

export async function getSessionMessages(sessionId, options = {}) {
  fs.writeFileSync(
    discoveryCapturePath,
    JSON.stringify({ method: 'getSessionMessages', session_id: sessionId, options }),
    'utf8',
  );
  const messages = [{
    type: 'user',
    uuid: 'claude-message-1',
    session_id: sessionId,
    message: { role: 'user', content: [{ type: 'text', text: 'Review the kernel' }] },
    parent_tool_use_id: null,
    parent_agent_id: null,
    timestamp: '2026-01-01T01:02:04.000Z',
  }, {
    type: 'assistant',
    uuid: 'claude-message-2',
    session_id: sessionId,
    message: { role: 'assistant', content: [
      { type: 'text', text: 'Kernel reviewed' },
      { type: 'thinking', thinking: 'Inspect the runtime boundary' },
      { type: 'tool_use', id: 'claude-tool-1', name: 'Read', input: { file_path: 'src/lib.rs' } },
    ] },
    parent_tool_use_id: 'claude-parent-tool-1',
    parent_agent_id: 'claude-agent-1',
    timestamp: '2026-01-01T01:02:05.000Z',
  }, {
    type: 'system',
    uuid: 'claude-message-3',
    session_id: sessionId,
    message: { role: 'system', content: 'Compacted context' },
    parent_tool_use_id: null,
    parent_agent_id: null,
    timestamp: '2026-01-01T01:02:06.000Z',
  }];
  const offset = options.offset ?? 0;
  return messages.slice(offset, offset + (options.limit ?? messages.length));
}
`,
  'utf8',
);
fs.mkdirSync(geminiSdkMirror, { recursive: true });
fs.writeFileSync(
  path.join(geminiSdkMirror, 'package.json'),
  JSON.stringify({
    type: 'module',
    name: '@google/gemini-cli-sdk',
    version: '0.0.0-test',
    exports: { '.': './index.js' },
  }),
  'utf8',
);
fs.writeFileSync(
  path.join(geminiSdkMirror, 'index.js'),
  `import fs from 'node:fs';

const capturePath = ${JSON.stringify(geminiCapturePath)};

function capture(record) {
  fs.writeFileSync(capturePath, JSON.stringify(record), 'utf8');
}

class FakeSession {
  constructor(id, record) {
    this.id = id;
    this.record = record;
  }

  async *sendStream(prompt, signal) {
    this.record.send_stream = { prompt, signal_present: Boolean(signal) };
    capture(this.record);
    yield { type: 'content', value: 'gemini sdk:' + prompt };
    yield { type: 'finished', value: { reason: 'stop' } };
  }
}

export class GeminiCliAgent {
  constructor(options) {
    this.record = { constructor_options: options };
  }

  session(options) {
    this.record.session_options = options ?? null;
    capture(this.record);
    return new FakeSession('gemini-sdk-created', this.record);
  }

  async resumeSession(id) {
    this.record.resume_session_id = id;
    capture(this.record);
    return new FakeSession(
      id === 'gemini-sdk-mismatch-request' ? 'gemini-sdk-different' : id,
      this.record,
    );
  }
}
`,
  'utf8',
);
fs.mkdirSync(openaiSdkMirror, { recursive: true });
fs.writeFileSync(
  path.join(openaiSdkMirror, 'package.json'),
  JSON.stringify({
    type: 'module',
    name: 'openai',
    version: '0.0.0-test',
    exports: { '.': './index.js' },
  }),
  'utf8',
);
fs.writeFileSync(
  path.join(openaiSdkMirror, 'index.js'),
  `import fs from 'node:fs';
const capturePath = ${JSON.stringify(openaiCapturePath)};
export default class OpenAI {
  constructor(options) { this.options = options; }
  chat = { completions: { create: async (request) => {
    fs.writeFileSync(capturePath, JSON.stringify({ options: this.options, request }), 'utf8');
    return { choices: [{ message: { content: 'openclaw sdk:' + request.messages[0].content } }] };
  } } };
}
`,
  'utf8',
);
fs.writeFileSync(
  path.join(opencodeSdkMirror, 'index.js'),
  `import fs from 'node:fs';

const capturePath = ${JSON.stringify(opencodeCapturePath)};

function capture(record) {
  fs.writeFileSync(capturePath, JSON.stringify(record), 'utf8');
}

export async function createOpencodeServer(options = {}) {
  return {
    url: 'http://127.0.0.1:4096',
    async close() {},
  };
}

export function createOpencodeClient(options = {}) {
  const record = {
    client_options: {
      base_url: options.baseUrl,
      directory: options.directory,
    },
  };
  let activeSessionId = null;
  let submittedPrompt = null;
  let resolvePrompt;
  const promptReady = new Promise((resolve) => { resolvePrompt = resolve; });
  capture(record);
  return {
    session: {
      list: async ({ query, signal } = {}) => {
        record.session_list = { query, signal_present: Boolean(signal) };
        capture(record);
        return {
          data: [{
            id: 'opencode-history-1',
            projectID: 'project-1',
            directory: 'C:/sdkwork/opencode-workspace',
            title: 'OpenCode history',
            version: '1.18.11',
            time: {
              created: Date.parse('2026-02-01T01:02:03.000Z'),
              updated: Date.parse('2026-02-02T03:04:05.000Z'),
            },
          }],
        };
      },
      messages: async ({ path, query, signal } = {}) => {
        record.session_messages = { path, query, signal_present: Boolean(signal) };
        capture(record);
        return {
          data: [{
            info: {
              id: 'opencode-message-1',
              sessionID: path.id,
              role: 'assistant',
              parentID: 'opencode-message-parent',
              time: { created: Date.parse('2026-02-02T03:04:06.000Z') },
            },
            parts: [{
              id: 'opencode-part-1',
              sessionID: path.id,
              messageID: 'opencode-message-1',
              type: 'text',
              text: 'Kernel reviewed',
            }],
          }],
        };
      },
      create: async ({ body, signal } = {}) => {
        record.session_create = { body, signal_present: Boolean(signal) };
        activeSessionId = 'opencode-sdk-created';
        capture(record);
        return { data: { id: 'opencode-sdk-created' } };
      },
      get: async ({ path, signal } = {}) => {
        record.session_get = { path, signal_present: Boolean(signal) };
        activeSessionId = path.id;
        capture(record);
        return {
          data: {
            id: path.id === 'opencode-sdk-mismatch-request'
              ? 'opencode-sdk-different'
              : path.id,
          },
        };
      },
      update: async ({ path, body, signal } = {}) => {
        record.session_update = { path, body, signal_present: Boolean(signal) };
        capture(record);
        return { data: { id: path.id } };
      },
      fork: async ({ path, body, signal } = {}) => {
        record.session_fork = { path, body, signal_present: Boolean(signal) };
        capture(record);
        return { data: { id: 'opencode-sdk-forked' } };
      },
      prompt: async ({ path, body, signal } = {}) => {
        record.session_prompt = { path, body, signal_present: Boolean(signal) };
        activeSessionId = path.id;
        submittedPrompt = body.parts[0].text;
        resolvePrompt();
        capture(record);
        return {
          data: {
            parts: [{ type: 'text', text: 'opencode sdk:' + body.parts[0].text }],
          },
        };
      },
    },
    event: {
      subscribe: async ({ signal } = {}) => {
        record.event_subscribe = { signal_present: Boolean(signal) };
        capture(record);
        return {
          stream: (async function* () {
            for (const sessionID of ['opencode-sdk-created', 'opencode-sdk-existing']) {
              yield {
                type: 'session.status',
                properties: { sessionID, status: { type: 'busy' } },
              };
              yield { type: 'session.idle', properties: { sessionID } };
            }
          })(),
        };
      },
    },
    event: {
      subscribe: async ({ signal } = {}) => {
        record.event_subscribe = { signal_present: Boolean(signal) };
        capture(record);
        return {
          stream: (async function* () {
            await promptReady;
            const messageID = 'opencode-assistant-1';
            yield {
              type: 'session.idle',
              properties: { sessionID: 'another-session' },
            };
            yield {
              type: 'message.updated',
              properties: {
                info: {
                  id: messageID,
                  sessionID: activeSessionId,
                  role: 'assistant',
                  time: { created: Date.now() },
                },
              },
            };
            yield {
              type: 'session.status',
              properties: { sessionID: activeSessionId, status: { type: 'busy' } },
            };
            yield {
              type: 'message.part.updated',
              properties: {
                part: {
                  id: 'opencode-text-1',
                  sessionID: activeSessionId,
                  messageID,
                  type: 'text',
                  text: 'opencode sdk:',
                },
                delta: 'opencode sdk:',
              },
            };
            yield {
              type: 'message.part.updated',
              properties: {
                part: {
                  id: 'opencode-text-1',
                  sessionID: activeSessionId,
                  messageID,
                  type: 'text',
                  text: 'opencode sdk:' + submittedPrompt,
                },
                delta: submittedPrompt,
              },
            };
            yield {
              type: 'message.part.updated',
              properties: {
                part: {
                  id: 'opencode-reasoning-1',
                  sessionID: activeSessionId,
                  messageID,
                  type: 'reasoning',
                  text: 'Inspect provider events',
                  time: { start: Date.now(), end: Date.now() },
                },
              },
            };
            yield {
              type: 'permission.updated',
              properties: {
                id: 'permission-1',
                sessionID: activeSessionId,
                title: 'Allow read',
                metadata: {},
                time: { created: Date.now() },
              },
            };
            yield {
              type: 'session.idle',
              properties: { sessionID: activeSessionId },
            };
            yield {
              type: 'message.part.updated',
              properties: {
                part: {
                  id: 'post-terminal',
                  sessionID: activeSessionId,
                  messageID,
                  type: 'text',
                  text: 'must not be emitted',
                },
              },
            };
          })(),
        };
      },
    },
  };
}
`,
  'utf8',
);
fs.writeFileSync(
  path.join(opencodeSdkMirror, 'v2.js'),
  `import fs from 'node:fs';

const capturePath = ${JSON.stringify(opencodeCapturePath)};

function capture(record) {
  fs.writeFileSync(capturePath, JSON.stringify(record), 'utf8');
}

export async function createOpencodeServer(options = {}) {
  return {
    url: 'http://127.0.0.1:4096',
    async close() {},
    signal_present: Boolean(options.signal),
  };
}

export function createOpencodeClient(options = {}) {
  const record = { client_options: { base_url: options.baseUrl, directory: options.directory } };
  let activeSessionId = null;
  let submittedPrompt = null;
  let resolvePrompt;
  const promptReady = new Promise((resolve) => { resolvePrompt = resolve; });
  capture(record);
  return {
    session: {
      create: async (parameters = {}, { signal } = {}) => {
        record.session_create = { parameters, signal_present: Boolean(signal) };
        activeSessionId = 'opencode-sdk-created';
        capture(record);
        return { data: { id: activeSessionId } };
      },
      get: async (parameters = {}, { signal } = {}) => {
        record.session_get = { parameters, signal_present: Boolean(signal) };
        activeSessionId = parameters.sessionID === 'opencode-sdk-mismatch-request'
          ? 'opencode-sdk-different'
          : parameters.sessionID;
        capture(record);
        return { data: { id: activeSessionId } };
      },
      update: async (parameters = {}, { signal } = {}) => {
        record.session_update = { parameters, signal_present: Boolean(signal) };
        capture(record);
        return { data: { id: parameters.sessionID } };
      },
      fork: async (parameters = {}, { signal } = {}) => {
        record.session_fork = { parameters, signal_present: Boolean(signal) };
        capture(record);
        return { data: { id: 'opencode-sdk-forked' } };
      },
      prompt: async (parameters = {}, { signal } = {}) => {
        record.session_prompt = { parameters, signal_present: Boolean(signal) };
        activeSessionId = parameters.sessionID;
        submittedPrompt = parameters.parts?.[0]?.text;
        resolvePrompt();
        capture(record);
        return { data: { parts: [{ type: 'text', text: 'opencode sdk:' + submittedPrompt }] } };
      },
    },
    event: {
      subscribe: async (parameters = {}, { signal } = {}) => {
        record.event_subscribe = { parameters, signal_present: Boolean(signal) };
        capture(record);
        return {
          stream: (async function* () {
            await promptReady;
            const messageID = 'opencode-assistant-1';
            yield {
              type: 'message.updated',
              properties: { info: { id: messageID, sessionID: activeSessionId, role: 'assistant' } },
            };
            yield {
              type: 'session.status',
              properties: { sessionID: activeSessionId, status: { type: 'busy' } },
            };
            yield {
              type: 'message.part.updated',
              properties: {
                part: { id: 'opencode-text-1', sessionID: activeSessionId, messageID, type: 'text', text: 'opencode sdk:' },
                delta: 'opencode sdk:',
              },
            };
            yield {
              type: 'message.part.updated',
              properties: {
                part: { id: 'opencode-text-1', sessionID: activeSessionId, messageID, type: 'text', text: 'opencode sdk:' + submittedPrompt },
                delta: submittedPrompt,
              },
            };
            yield {
              type: 'message.part.updated',
              properties: {
                part: { id: 'opencode-reasoning-1', sessionID: activeSessionId, messageID, type: 'reasoning', text: 'Inspect provider events' },
              },
            };
            yield { type: 'session.idle', properties: { sessionID: activeSessionId } };
          })(),
        };
      },
    },
    permission: {
      reply: async (parameters = {}, { signal } = {}) => {
        record.permission_reply = { parameters, signal_present: Boolean(signal) };
        capture(record);
        return { data: true };
      },
      respond: async (parameters = {}, { signal } = {}) => {
        record.permission_respond = { parameters, signal_present: Boolean(signal) };
        capture(record);
        return { data: true };
      },
    },
    question: {
      reply: async (parameters = {}, { signal } = {}) => {
        record.question_reply = { parameters, signal_present: Boolean(signal) };
        capture(record);
        return { data: true };
      },
      reject: async (parameters = {}, { signal } = {}) => {
        record.question_reject = { parameters, signal_present: Boolean(signal) };
        capture(record);
        return { data: true };
      },
    },
    v2: {
      session: {
        list: async (parameters = {}, { signal } = {}) => {
          record.session_list_v2 = { parameters, signal_present: Boolean(signal) };
          capture(record);
          return {
            data: {
              data: [{
                id: 'opencode-history-1',
                parentID: 'opencode-history-parent',
                projectID: 'project-1',
                agent: 'build',
                model: { providerID: 'anthropic', id: 'claude-sonnet-4-6' },
                cost: 25,
                tokens: {
                  input: 10,
                  output: 20,
                  reasoning: 3,
                  cache: { read: 4, write: 5 },
                },
                summary: { additions: 6, deletions: 2, files: 3 },
                time: {
                  created: Date.parse('2026-02-01T01:02:03.000Z'),
                  updated: Date.parse('2026-02-02T03:04:05.000Z'),
                },
                title: 'OpenCode history',
                location: {
                  directory: 'C:/sdkwork/opencode-workspace',
                  workspaceID: 'workspace-1',
                },
              }],
              cursor: { previous: 'opencode-session-previous', next: 'opencode-session-next' },
            },
          };
        },
        messages: async (parameters = {}, { signal } = {}) => {
          record.session_messages_v2 = { parameters, signal_present: Boolean(signal) };
          capture(record);
          return {
            data: {
              data: [{
                id: 'opencode-message-1',
                sessionID: parameters.sessionID === 'opencode-history-mismatch'
                  ? 'opencode-history-other'
                  : parameters.sessionID,
                type: 'assistant',
                agent: 'build',
                model: { providerID: 'anthropic', id: 'claude-sonnet-4-6' },
                content: [{
                  id: 'opencode-part-1',
                  sessionID: parameters.sessionID,
                  messageID: 'opencode-message-1',
                  type: 'text',
                  text: 'Kernel reviewed',
                }, {
                  id: 'opencode-part-2',
                  sessionID: parameters.sessionID,
                  messageID: 'opencode-message-1',
                  type: 'reasoning',
                  text: 'Inspect the SDK adapter',
                }, {
                  id: 'opencode-part-3',
                  sessionID: parameters.sessionID,
                  messageID: 'opencode-message-1',
                  type: 'tool',
                  callID: 'opencode-call-1',
                  tool: 'read',
                  state: {
                    status: 'completed',
                    input: { filePath: 'src/lib.rs' },
                    content: [{ type: 'text', text: 'source' }],
                    structured: {},
                    result: 'source',
                  },
                }],
                time: {
                  created: Date.parse('2026-02-02T03:04:06.000Z'),
                  completed: Date.parse('2026-02-02T03:04:07.000Z'),
                },
              }],
              cursor: { next: 'opencode-message-next' },
            },
          };
        },
        get: async ({ sessionID } = {}, { signal } = {}) => {
          record.session_get_v2 = { session_id: sessionID, signal_present: Boolean(signal) };
          capture(record);
          return { data: { data: { id: sessionID } } };
        },
        interrupt: async ({ sessionID } = {}, { signal } = {}) => {
          record.session_interrupt = { session_id: sessionID, signal_present: Boolean(signal) };
          capture(record);
          return { data: undefined };
        },
        compact: async ({ sessionID } = {}, { signal } = {}) => {
          record.session_compact = { session_id: sessionID, signal_present: Boolean(signal) };
          capture(record);
          return { data: undefined };
        },
      },
    },
  };
}
`,
  'utf8',
);
const opencodeDurableMirror = path.join(tempRoot, 'opencode-durable-sdk');
const opencodeDurableCapturePath = path.join(tempRoot, 'opencode-durable-capture.json');
fs.mkdirSync(opencodeDurableMirror, { recursive: true });
fs.writeFileSync(
  path.join(opencodeDurableMirror, 'package.json'),
  JSON.stringify({
    type: 'module',
    name: '@opencode-ai/sdk',
    version: '0.0.0-test',
    exports: { '.': './index.js', './v2': './v2.js' },
  }),
  'utf8',
);
fs.writeFileSync(
  path.join(opencodeDurableMirror, 'v2.js'),
  `import fs from 'node:fs';

const capturePath = ${JSON.stringify(opencodeDurableCapturePath)};

function capture(record) {
  fs.writeFileSync(capturePath, JSON.stringify(record), 'utf8');
}

export async function createOpencodeServer(options = {}) {
  return {
    url: 'http://127.0.0.1:4097',
    async close() {},
    signal_present: Boolean(options.signal),
  };
}

export function createOpencodeClient(options = {}) {
  const record = { client_options: { base_url: options.baseUrl, directory: options.directory } };
  let activeSessionId = null;
  let submittedPrompt = null;
  let resolvePrompt;
  const promptReady = new Promise((resolve) => { resolvePrompt = resolve; });
  capture(record);
  return {
    session: {
      create: async (parameters = {}, { signal } = {}) => {
        record.session_create = { parameters, signal_present: Boolean(signal) };
        activeSessionId = 'opencode-durable-created';
        capture(record);
        return { data: { id: activeSessionId } };
      },
      get: async (parameters = {}, { signal } = {}) => {
        record.session_get = { parameters, signal_present: Boolean(signal) };
        activeSessionId = parameters.sessionID === 'opencode-durable-mismatch-request'
          ? 'opencode-durable-different'
          : parameters.sessionID;
        capture(record);
        return { data: { id: activeSessionId } };
      },
      update: async (parameters = {}, { signal } = {}) => {
        record.session_update = { parameters, signal_present: Boolean(signal) };
        capture(record);
        return { data: { id: parameters.sessionID } };
      },
      // The legacy prompt/subscribe routes are intentionally absent so that a
      // v1 fallback cannot satisfy this mirror: only the durable v2 surface
      // (session.prompt + event.subscribe under v2) may be used.
    },
      v2: {
        session: {
          prompt: async (parameters = {}, { signal } = {}) => {
            record.session_prompt_v2 = { parameters, signal_present: Boolean(signal) };
            activeSessionId = parameters.sessionID;
            submittedPrompt = parameters.prompt?.text;
            resolvePrompt();
            capture(record);
            return {
              data: {
                admittedSeq: 41,
                id: parameters.id,
                sessionID: parameters.sessionID,
                prompt: { text: submittedPrompt },
                delivery: parameters.delivery,
                timeCreated: Date.now(),
              },
            };
          },
          active: async (parameters = {}, { signal } = {}) => {
            record.session_active_v2 = { parameters, signal_present: Boolean(signal) };
            capture(record);
            return { data: { data: {} } };
          },
        },
      event: {
        subscribe: async (parameters = {}, { signal } = {}) => {
          record.event_subscribe_v2 = { parameters, signal_present: Boolean(signal) };
          capture(record);
          return {
            stream: (async function* () {
              await promptReady;
              const messageID = 'opencode-durable-assistant-1';
              yield {
                id: 'evt-1',
                type: 'session.status',
                data: { sessionID: activeSessionId, status: { type: 'busy' } },
              };
              yield {
                id: 'evt-2',
                type: 'message.updated',
                data: {
                  sessionID: activeSessionId,
                  info: {
                    id: messageID,
                    sessionID: activeSessionId,
                    role: 'assistant',
                    time: { created: Date.now() },
                  },
                },
              };
              yield {
                id: 'evt-3',
                type: 'message.part.updated',
                data: {
                  sessionID: activeSessionId,
                  part: {
                    id: 'opencode-durable-text-1',
                    sessionID: activeSessionId,
                    messageID,
                    type: 'text',
                    text: 'opencode durable:',
                  },
                  time: Date.now(),
                },
              };
              yield {
                id: 'evt-4',
                type: 'message.part.updated',
                data: {
                  sessionID: activeSessionId,
                  part: {
                    id: 'opencode-durable-text-1',
                    sessionID: activeSessionId,
                    messageID,
                    type: 'text',
                    text: 'opencode durable:' + submittedPrompt,
                  },
                  time: Date.now(),
                },
              };
              yield {
                id: 'evt-5',
                type: 'message.part.updated',
                data: {
                  sessionID: activeSessionId,
                  part: {
                    id: 'opencode-durable-reasoning-1',
                    sessionID: activeSessionId,
                    messageID,
                    type: 'reasoning',
                    text: 'Inspect durable events',
                    time: { start: Date.now(), end: Date.now() },
                  },
                  time: Date.now(),
                },
              };
              // Sync bridge envelope: the versioned type suffix and the
              // syncEvent wrapper must be normalized to the plain event.
              yield {
                id: 'evt-6',
                type: 'sync',
                syncEvent: {
                  type: 'session.status.1',
                  id: 'evt-6-inner',
                  seq: 1,
                  aggregateID: 'session-agg-1',
                  data: {
                    sessionID: activeSessionId,
                    status: { type: 'busy' },
                  },
                },
              };
              yield {
                id: 'evt-7',
                type: 'session.idle',
                data: { sessionID: activeSessionId, idleAt: Date.now(), lastSeq: 42 },
              };
              yield {
                id: 'evt-8',
                type: 'message.part.updated',
                data: {
                  sessionID: activeSessionId,
                  part: {
                    id: 'post-terminal',
                    sessionID: activeSessionId,
                    messageID,
                    type: 'text',
                    text: 'must not be emitted',
                  },
                  time: Date.now(),
                },
              };
            })(),
          };
        },
      },
    },
  };
}
`,
  'utf8',
);
fs.mkdirSync(codexSdkMirror, { recursive: true });fs.writeFileSync(
  path.join(codexSdkMirror, 'package.json'),
  JSON.stringify({
    type: 'module',
    name: '@openai/codex-sdk',
    version: '0.0.0-test',
    exports: { '.': './index.js' },
  }),
  'utf8',
);
fs.writeFileSync(
  path.join(codexSdkMirror, 'index.js'),
  `import fs from 'node:fs';

const capturePath = ${JSON.stringify(codexCapturePath)};

function capture(value) {
  fs.writeFileSync(capturePath, JSON.stringify(value), 'utf8');
}

class FakeThread {
  constructor(id, record) {
    this.id = id;
    this.record = record;
  }

  async run(prompt, turnOptions = {}) {
    this.record.run = {
      prompt,
      signal_present: Boolean(turnOptions.signal),
    };
    if (!this.id) {
      this.id = 'thread-sdk-started';
    }
    capture(this.record);
    return {
      finalResponse: 'official sdk:' + prompt,
      items: [{ type: 'agent_message', text: 'official sdk:' + prompt }],
    };
  }

  async runStreamed(prompt, turnOptions = {}) {
    this.record.stream = {
      prompt,
      signal_present: Boolean(turnOptions.signal),
    };
    const thread = this;
    capture(this.record);
    return {
      events: (async function* () {
        if (!thread.id) {
          thread.id = 'thread-sdk-streamed';
        }
        yield { type: 'thread.started', thread_id: thread.id };
        yield { type: 'item.updated', item: { id: 'message-1', type: 'agent_message', text: 'official' } };
        if (prompt === 'stream emits fatal error') {
          yield { type: 'error', message: 'stream transport failed' };
          return;
        }
        if (prompt === 'stream ends incomplete') {
          return;
        }
        yield { type: 'item.updated', item: { id: 'message-1', type: 'agent_message', text: 'official sdk' } };
        yield { type: 'item.completed', item: { id: 'message-1', type: 'agent_message', text: 'official sdk stream' } };
        yield { type: 'turn.completed', usage: {} };
      })(),
    };
  }
}

export class Codex {
  constructor(options = {}) {
    this.record = { constructor_options: options };
  }

  startThread(options = {}) {
    this.record.start_thread_options = options;
    return new FakeThread(null, this.record);
  }

  resumeThread(id, options = {}) {
    this.record.resume_thread_id = id;
    this.record.resume_thread_options = options;
    return new FakeThread(id, this.record);
  }
}
`,
  'utf8',
);
fs.mkdirSync(invalidSdkMirror, { recursive: true });
fs.writeFileSync(
  path.join(invalidSdkMirror, 'package.json'),
  JSON.stringify({
    type: 'module',
    name: '@sdkwork/invalid-sdk',
    version: '0.0.0-test',
    exports: { '.': './missing.js' },
  }),
  'utf8',
);

process.env.SDKWORK_AGENT_SDK_WORKSPACE_ROOT = birdcoderRoot;
delete process.env.SDKWORK_KERNEL_PROFILE_ID;
delete process.env.SDKWORK_KERNEL_ALLOW_MOCK_PROVIDERS;
delete process.env.SDKWORK_AGENT_SDK_PACKAGE_PATHS;

assert.match(
  fs.readFileSync(path.join(workerDir, 'engine-sdk-live.mjs'), 'utf8'),
  /external\/opencode\/packages\/sdk\/js/,
  'OpenCode SDK resolver should include the canonical SDK mirror path',
);
assert.match(
  fs.readFileSync(path.join(workerDir, 'engine-sdk-live.mjs'), 'utf8'),
  /external\/gemini\/packages\/sdk/,
  'Gemini SDK resolver should include the canonical SDK mirror path',
);

assert.equal(mockProviderInvocationAllowed(), true, 'dev profile should allow mock fallback');

const appTopologyPath = resolvePackageSpecifier('@sdkwork/app-topology');
assert.ok(
  appTopologyPath?.startsWith('file:'),
  'native package resolution should return an importable file URL, not a Windows absolute path',
);
await import(appTopologyPath);

const installedOpencodeV2Path = resolvePackageExportSpecifier('@opencode-ai/sdk', './v2');
assert.match(
  installedOpencodeV2Path ?? '',
  /@opencode-ai\+sdk@[^/]+\/node_modules\/@opencode-ai\/sdk\/dist\/v2\/index\.js$/u,
  'ESM-only OpenCode v2 exports must resolve to the installed official package',
);

process.env.SDKWORK_AGENT_SDK_PACKAGE_PATHS = JSON.stringify({
  '@openai/codex-sdk': path.join(kernelRoot, 'external/codex/sdk/typescript'),
});
const codexPath = resolvePackageSpecifier('@openai/codex-sdk');
assert.equal(
  codexPath,
  null,
  'unbuilt source mirrors must not be treated as importable live SDK packages',
);

process.env.SDKWORK_AGENT_SDK_PACKAGE_PATHS = JSON.stringify({
  '@opencode-ai/sdk': opencodeSdkMirror,
});
const opencodePath = resolvePackageSpecifier('@opencode-ai/sdk');
assert.ok(opencodePath, 'opencode sdk should resolve through explicit package path injection');
assert.ok(
  opencodePath.endsWith('/index.js') && opencodePath.includes('opencode-sdk'),
  'opencode resolver should return an importable package entry file',
);

process.env.OPENCODE_SERVER_URL = 'http://127.0.0.1:4096';
const sessionControlBase = {
  control_request_id: 'control-opencode',
  session_id: 'session-canonical-control',
  provider_session_id: 'opencode-sdk-existing',
  policy_decision_id: 'policy-decision-control',
  timeout_ms: 2_000,
};
const interrupted = await invokeSessionControlRuntime('@opencode-ai/sdk', {
  ...sessionControlBase,
  operation: 'session_interrupt',
  reason: 'user_cancelled',
});
assert.equal(interrupted.status, 'applied');
assert.equal(interrupted.provider_session_id, 'opencode-sdk-existing');
assert.deepEqual(JSON.parse(fs.readFileSync(opencodeCapturePath, 'utf8')).session_interrupt, {
  session_id: 'opencode-sdk-existing',
  signal_present: true,
});

const compacted = await invokeSessionControlRuntime('@opencode-ai/sdk', {
  ...sessionControlBase,
  operation: 'session_compact',
});
assert.equal(compacted.status, 'applied');
assert.deepEqual(JSON.parse(fs.readFileSync(opencodeCapturePath, 'utf8')).session_compact, {
  session_id: 'opencode-sdk-existing',
  signal_present: true,
});

const forked = await invokeSessionControlRuntime('@opencode-ai/sdk', {
  ...sessionControlBase,
  operation: 'session_fork',
  before_message_id: 'message-7',
});
assert.equal(forked.provider_session_id, 'opencode-sdk-existing');
assert.equal(forked.forked_provider_session_id, 'opencode-sdk-forked');
assert.deepEqual(JSON.parse(fs.readFileSync(opencodeCapturePath, 'utf8')).session_fork, {
  parameters: {
    sessionID: 'opencode-sdk-existing',
    messageID: 'message-7',
  },
  signal_present: true,
});
delete process.env.OPENCODE_SERVER_URL;

const codexSessionControlBase = {
  control_request_id: 'control-codex',
  session_id: 'session-canonical-codex-control',
  provider_session_id: 'codex-thread-existing',
  policy_decision_id: 'policy-decision-codex-control',
  timeout_ms: 2_000,
};
await assert.rejects(
  invokeSessionControlRuntime('@openai/codex-sdk', {
    ...codexSessionControlBase,
    operation: 'session_compact',
    focus: 'retain deployment context',
  }),
  /thread\/compact\/start does not support a focus parameter/,
);
await assert.rejects(
  invokeSessionControlRuntime('@openai/codex-sdk', {
    ...codexSessionControlBase,
    operation: 'session_fork',
    before_message_id: 'message-not-a-turn',
  }),
  /cannot be mapped to a Codex Turn id/,
);

process.env.SDKWORK_AGENT_SDK_PACKAGE_PATHS = JSON.stringify({
  '@anthropic-ai/claude-agent-sdk': claudeSdkMirror,
});
const claudeControlBase = {
  control_request_id: 'control-claude',
  session_id: 'session-canonical-claude-control',
  provider_session_id: 'claude-sdk-existing',
  policy_decision_id: 'policy-decision-claude-control',
  timeout_ms: 2_000,
};
const claudeNoOpInterrupt = await invokeSessionControlRuntime('@anthropic-ai/claude-agent-sdk', {
  ...claudeControlBase,
  operation: 'session_interrupt',
  model_request_id: 'req-claude-nonexistent',
  reason: 'user_cancelled',
});
assert.equal(claudeNoOpInterrupt.status, 'no_op');
assert.equal(claudeNoOpInterrupt.provider_session_id, 'claude-sdk-existing');
await assert.rejects(
  invokeSessionControlRuntime('@anthropic-ai/claude-agent-sdk', {
    ...claudeControlBase,
    operation: 'session_compact',
  }),
  /unsupported Claude session control operation: session_compact/,
  'The official Claude agent SDK exposes no session compact trigger',
);
const claudeForked = await invokeSessionControlRuntime('@anthropic-ai/claude-agent-sdk', {
  ...claudeControlBase,
  operation: 'session_fork',
  working_directory: 'C:/sdkwork/claude-workspace',
  before_message_id: 'claude-message-7',
});
assert.equal(claudeForked.status, 'applied');
assert.equal(claudeForked.provider_session_id, 'claude-sdk-existing');
assert.equal(claudeForked.forked_provider_session_id, 'claude-sdk-forked');
assert.deepEqual(
  JSON.parse(fs.readFileSync(claudeCapturePath, 'utf8')).fork_session,
  {
    session_id: 'claude-sdk-existing',
    options: {
      cwd: 'C:/sdkwork/claude-workspace',
      upToMessageId: 'claude-message-7',
    },
  },
  'Claude session fork must use the official forkSession() API',
);

const interruptedClaudeTurn = invokeModelChatLive('@anthropic-ai/claude-agent-sdk', {
  model_request_id: 'req-claude-interrupt',
  session_id: 'session-canonical-claude-interrupt',
  provider_session_id: 'claude-sdk-existing',
  messages: ['interrupt claude turn'],
  timeout_ms: 5_000,
});
const claudeInterruptDeadline = Date.now() + 2_000;
for (;;) {
  try {
    const registered = JSON.parse(fs.readFileSync(claudeCapturePath, 'utf8'));
    if (registered.turn_registered === true) {
      break;
    }
  } catch {
    // capture file not written yet
  }
  if (Date.now() > claudeInterruptDeadline) {
    throw new Error('claude turn did not register for in-process interrupt');
  }
  await new Promise((resolve) => setTimeout(resolve, 20));
}
const claudeInterrupted = await invokeSessionControlRuntime('@anthropic-ai/claude-agent-sdk', {
  ...claudeControlBase,
  operation: 'session_interrupt',
  model_request_id: 'req-claude-interrupt',
  reason: 'user_cancelled',
});
assert.equal(claudeInterrupted.status, 'applied');
assert.equal(claudeInterrupted.provider_session_id, 'claude-sdk-existing');
await assert.rejects(
  interruptedClaudeTurn,
  /claude sdk turn aborted by session control/,
  'The in-process session control must abort the active Claude query',
);

process.env.SDKWORK_AGENT_SDK_PACKAGE_PATHS = JSON.stringify({
  '@sdkwork/invalid-sdk': invalidSdkMirror,
});
assert.equal(
  resolvePackageSpecifier('@sdkwork/invalid-sdk'),
  null,
  'local package mirrors with missing entry files must not be marked resolved',
);

process.env.SDKWORK_AGENT_SDK_WORKSPACE_ROOT = kernelRoot;
process.env.SDKWORK_AGENT_SDK_PACKAGE_PATHS = JSON.stringify({
  '@openai/codex-sdk': path.join(kernelRoot, 'external/codex/sdk/typescript'),
  '@anthropic-ai/claude-agent-sdk': path.join(kernelRoot, 'external/claude-code'),
  '@google/gemini-cli-sdk': [
    path.join(kernelRoot, 'external/gemini/packages/sdk'),
    path.join(kernelRoot, 'external/gemini-cli/packages/sdk'),
  ],
});
const geminiPath = resolvePackageSpecifier('@google/gemini-cli-sdk');
assert.equal(
  resolvePackageSpecifier('@anthropic-ai/claude-agent-sdk'),
  null,
  'Claude source-tree mirror is not the official SDK package unless it exposes an importable matching package',
);
assert.equal(
  geminiPath,
  null,
  'missing Gemini SDK package mirror must not be marked resolved',
);

const stub = buildStubModelChatResult(
  '@openai/codex-sdk',
  { model_request_id: 'req-1', messages: ['hello'] },
  probePackage('@openai/codex-sdk'),
);
assert.equal(stub.mode, 'stub');

const wirePrompt = resolveModelChatPrompt({
  messages: ['legacy'],
  wire_messages: [{ role: 'user', content: [{ type: 'text', text: 'structured' }] }],
});
assert.equal(wirePrompt, 'structured', 'wire_messages should drive live prompt resolution');

process.env.SDKWORK_AGENT_SDK_PACKAGE_PATHS = JSON.stringify({
  '@anthropic-ai/claude-agent-sdk': claudeSdkMirror,
});
assert.equal(
  probeModelChatRuntime('@anthropic-ai/claude-agent-sdk').runtime_mode,
  'sdk_live',
  'a resolved Claude Agent SDK must remain the primary runtime even when the CLI is installed',
);
const claudeActivity = [];
const claudeResult = await invokeModelChatLive(
  '@anthropic-ai/claude-agent-sdk',
  {
    model_request_id: 'req-claude-sdk-new',
    session_id: 'session-canonical-claude-new',
    model_id: 'claude-sonnet-4-6',
    working_directory: 'C:/sdkwork/claude-workspace',
    timeout_ms: 2_000,
    messages: ['Claude prompt'],
    execution_options: { approval_policy: 'accept-edits' },
  },
  {
    onActivity: async (event) => claudeActivity.push(event),
  },
);
assert.equal(claudeResult.provider_session_id, 'claude-sdk-created');
assert.equal(claudeResult[VERIFIED_PROVIDER_SESSION_ID], true);
assert.deepEqual(claudeResult.messages, ['claude sdk:Claude prompt']);
assert.deepEqual(
  claudeActivity.map((event) => event.phase),
  ['started', 'working', 'working', 'idle', 'terminal'],
);
const claudeCapture = JSON.parse(fs.readFileSync(claudeCapturePath, 'utf8'));
assert.deepEqual(claudeCapture, {
  prompt: 'Claude prompt',
  options: {
    cwd: 'C:/sdkwork/claude-workspace',
      has_abort_controller: true,
      model: 'claude-sonnet-4-6',
      permission_mode: 'acceptEdits',
    },
  });
assert.equal(
  Object.hasOwn(claudeCapture.options, 'resume'),
  false,
  'canonical session_id must not resume a Claude provider Session.',
);
const claudeStreamChunks = [];
const claudeStreamEvents = [];
const claudeStreamResult = await invokeModelChatStreamLive(
  '@anthropic-ai/claude-agent-sdk',
  {
    model_request_id: 'req-claude-sdk-stream',
    session_id: 'session-canonical-claude-stream',
    turn_id: 'turn-canonical-claude-stream',
    messages: ['Claude streamed prompt'],
    timeout_ms: 2_000,
  },
  {
    onChunk: async (chunk) => claudeStreamChunks.push(chunk),
    onEvent: async (event) => claudeStreamEvents.push(event),
  },
);
assert.equal(claudeStreamResult.provider_session_id, 'claude-sdk-created');
assert.deepEqual(claudeStreamResult.chunks, []);
assert.deepEqual(claudeStreamChunks, [
  { sequence: 0, content: 'claude sdk:' },
  { sequence: 1, content: 'Claude streamed prompt' },
]);
assert.deepEqual(
  claudeStreamEvents.map((event) => event.event_type),
  [
    'agent.turn.started',
    'agent.message.started',
    'agent.message.updated',
    'agent.message.updated',
    'agent.message.completed',
    'agent.turn.completed',
  ],
);
assert.equal(claudeStreamEvents.at(-1).event_type, 'agent.turn.completed');
assert.equal(claudeStreamEvents.at(-1).step_id, 'turn-canonical-claude-stream');
assert.equal(claudeStreamEvents[1].payload.providerId, 'claude-code');
assert.equal(claudeStreamEvents[1].payload.providerSessionId, 'claude-sdk-created');
assert.equal(
  claudeStreamEvents.some((event) => (
    event.payload.rawProviderPayload?.type === 'prompt_suggestion'
  )),
  false,
  'Claude must not forward SDK events after the provider terminal acknowledgement',
);
assert.equal(
  JSON.parse(fs.readFileSync(claudeCapturePath, 'utf8')).options.include_partial_messages,
  true,
  'Claude streaming must use includePartialMessages from the official SDK',
);
const resumedClaudeResult = await invokeModelChatLive('@anthropic-ai/claude-agent-sdk', {
  model_request_id: 'req-claude-sdk-resume',
  session_id: 'session-canonical-claude-resume',
  provider_session_id: 'claude-sdk-existing',
  messages: ['Resume Claude'],
});
assert.equal(resumedClaudeResult.provider_session_id, 'claude-sdk-existing');
assert.equal(resumedClaudeResult[VERIFIED_PROVIDER_SESSION_ID], true);
assert.equal(
  JSON.parse(fs.readFileSync(claudeCapturePath, 'utf8')).options.resume,
  'claude-sdk-existing',
  'Claude resume must use the official query resume option.',
);
await invokeModelChatLive('@anthropic-ai/claude-agent-sdk', {
  model_request_id: 'req-claude-sdk-bypass',
  messages: ['Bypass Claude permissions'],
  execution_options: { approval_policy: 'bypass-permissions' },
});
const bypassClaudeCapture = JSON.parse(fs.readFileSync(claudeCapturePath, 'utf8'));
assert.equal(bypassClaudeCapture.options.permission_mode, 'bypassPermissions');
assert.equal(bypassClaudeCapture.options.allow_dangerously_skip_permissions, true);

await invokeModelChatLive('@anthropic-ai/claude-agent-sdk', {
  model_request_id: 'req-claude-sdk-sandbox',
  messages: ['Sandbox Claude'],
  execution_options: { sandbox_mode: 'workspace-write' },
});
const sandboxClaudeCapture = JSON.parse(fs.readFileSync(claudeCapturePath, 'utf8'));
assert.deepEqual(sandboxClaudeCapture.options.sandbox, {
  enabled: true,
  autoAllowBashIfSandboxed: true,
  failIfUnavailable: true,
});
const mismatchedClaudeActivity = [];
await assert.rejects(
  invokeModelChatLive(
    '@anthropic-ai/claude-agent-sdk',
    {
      model_request_id: 'req-claude-sdk-mismatch',
      session_id: 'session-canonical-claude-mismatch',
      provider_session_id: 'claude-sdk-existing',
      messages: ['mismatched Claude session'],
    },
    { onActivity: async (event) => mismatchedClaudeActivity.push(event) },
  ),
  /resumed a different provider session/,
);
assert.deepEqual(
  mismatchedClaudeActivity,
  [],
  'an unverified request session id must not be published as provider activity',
);

const claudeSessions = await invokeSessionDiscoveryRuntime(
  '@anthropic-ai/claude-agent-sdk',
  {
    operation: 'session_list',
    working_directory: 'C:/sdkwork/claude-workspace',
    limit: 2,
  },
);
assert.deepEqual(claudeSessions.items, [{
  provider_session_id: 'claude-history-1',
  title: 'Claude history',
  preview: 'Review the kernel',
  summary: 'Claude history summary',
  created_at: '2026-01-01T01:02:03.000Z',
  updated_at: '2026-01-03T03:04:05.000Z',
  cwd: 'C:/sdkwork/claude-workspace',
  metadata: { 'claude.git_branch': 'main', 'claude.tag': 'commercial' },
}, {
  provider_session_id: 'claude-history-2',
  title: 'Claude second page boundary',
  summary: 'Claude second page boundary',
  updated_at: '2026-01-02T03:04:05.000Z',
  cwd: 'C:/sdkwork/claude-workspace',
}]);
assert.equal(typeof claudeSessions.next_cursor, 'string');
assert.equal(claudeSessions.previous_cursor, undefined);
assert.notEqual(claudeSessions.next_cursor, '2', 'Claude offsets must remain opaque');
assert.deepEqual(
  JSON.parse(fs.readFileSync(claudeDiscoveryCapturePath, 'utf8')),
  {
    method: 'listSessions',
    options: { dir: 'C:/sdkwork/claude-workspace', limit: 2 },
  },
);
const claudeSessionsLastPage = await invokeSessionDiscoveryRuntime(
  '@anthropic-ai/claude-agent-sdk',
  {
    operation: 'session_list',
    working_directory: 'C:/sdkwork/claude-workspace',
    limit: 2,
    cursor: claudeSessions.next_cursor,
  },
);
assert.deepEqual(
  claudeSessionsLastPage.items.map((session) => session.provider_session_id),
  ['claude-history-3'],
);
assert.equal(claudeSessionsLastPage.next_cursor, undefined);
assert.equal(typeof claudeSessionsLastPage.previous_cursor, 'string');
assert.deepEqual(
  JSON.parse(fs.readFileSync(claudeDiscoveryCapturePath, 'utf8')),
  {
    method: 'listSessions',
    options: { dir: 'C:/sdkwork/claude-workspace', limit: 2, offset: 2 },
  },
);
const claudeHistory = await invokeSessionDiscoveryRuntime(
  '@anthropic-ai/claude-agent-sdk',
  {
    operation: 'session_history',
    provider_session_id: 'claude-history-1',
    working_directory: 'C:/sdkwork/claude-workspace',
    limit: 2,
  },
);
assert.deepEqual(
  claudeHistory.items.map((message) => ({
    id: message.provider_message_id,
    role: message.role,
    text: message.parts[0]?.text,
    created_at: message.created_at,
  })),
  [{
    id: 'claude-message-1',
    role: 'user',
    text: 'Review the kernel',
    created_at: '2026-01-01T01:02:04.000Z',
  }, {
    id: 'claude-message-2',
    role: 'agent',
    text: 'Kernel reviewed',
    created_at: '2026-01-01T01:02:05.000Z',
  }],
);
assert.deepEqual(claudeHistory.items[1].parts.slice(1), [{
  part_id: 'claude-message-2:1',
  kind: 'text',
  text: 'Inspect the runtime boundary',
  metadata: { 'sdkwork.provider.content_type': 'thinking' },
}, {
  part_id: 'claude-tool-1',
  kind: 'tool_call_ref',
  tool_call_id: 'claude-tool-1',
  name: 'Read',
  json: { file_path: 'src/lib.rs' },
  metadata: { 'sdkwork.provider.content_type': 'tool' },
}]);
assert.deepEqual(claudeHistory.items[1].metadata, {
  parent_tool_use_id: 'claude-parent-tool-1',
  parent_agent_id: 'claude-agent-1',
});
assert.equal(
  Object.hasOwn(claudeHistory.items[1], 'parent_provider_message_id'),
  false,
  'Claude parent_tool_use_id is tool lineage, not parent message lineage',
);
assert.equal(typeof claudeHistory.next_cursor, 'string');
assert.equal(claudeHistory.previous_cursor, undefined);
assert.deepEqual(
  JSON.parse(fs.readFileSync(claudeDiscoveryCapturePath, 'utf8')),
  {
    method: 'getSessionMessages',
    session_id: 'claude-history-1',
    options: {
      dir: 'C:/sdkwork/claude-workspace',
      limit: 2,
      includeSystemMessages: true,
    },
  },
);
const claudeHistoryLastPage = await invokeSessionDiscoveryRuntime(
  '@anthropic-ai/claude-agent-sdk',
  {
    operation: 'session_history',
    provider_session_id: 'claude-history-1',
    working_directory: 'C:/sdkwork/claude-workspace',
    limit: 2,
    cursor: claudeHistory.next_cursor,
  },
);
assert.deepEqual(
  claudeHistoryLastPage.items.map((message) => ({
    id: message.provider_message_id,
    role: message.role,
    text: message.parts[0]?.text,
  })),
  [{ id: 'claude-message-3', role: 'system', text: 'Compacted context' }],
);
assert.equal(claudeHistoryLastPage.next_cursor, undefined);
assert.equal(typeof claudeHistoryLastPage.previous_cursor, 'string');
await assert.rejects(
  invokeSessionDiscoveryRuntime('@anthropic-ai/claude-agent-sdk', {
    operation: 'session_history',
    provider_session_id: 'claude-history-1',
    working_directory: 'C:/sdkwork/claude-workspace',
    limit: 2,
    cursor: claudeSessions.next_cursor,
  }),
  /cursor.*session_history|session_history.*cursor/,
  'Claude cursors must be bound to their operation and provider session',
);
await assert.rejects(
  invokeSessionDiscoveryRuntime('@anthropic-ai/claude-agent-sdk', {
    operation: 'session_list',
    limit: 201,
  }),
  /between 1 and 200/,
  'provider SDK discovery must not request more than the L1 page bound',
);

process.env.SDKWORK_AGENT_SDK_PACKAGE_PATHS = JSON.stringify({
  '@google/gemini-cli-sdk': geminiSdkMirror,
});
const geminiResult = await invokeModelChatLive('@google/gemini-cli-sdk', {
  model_request_id: 'req-gemini-sdk-new',
  session_id: 'session-canonical-gemini-new',
  model_id: 'gemini-2.5-pro',
  working_directory: 'C:/sdkwork/gemini-workspace',
  timeout_ms: 2_000,
  messages: ['Gemini prompt'],
});
assert.equal(geminiResult.provider_session_id, 'gemini-sdk-created');
assert.equal(geminiResult[VERIFIED_PROVIDER_SESSION_ID], true);
assert.deepEqual(geminiResult.messages, ['gemini sdk:Gemini prompt']);
const geminiCapture = JSON.parse(fs.readFileSync(geminiCapturePath, 'utf8'));
assert.deepEqual(geminiCapture.constructor_options, {
  instructions: '',
  cwd: 'C:/sdkwork/gemini-workspace',
  model: 'gemini-2.5-pro',
});
assert.equal(geminiCapture.session_options, null);
assert.deepEqual(geminiCapture.send_stream, {
  prompt: 'Gemini prompt',
  signal_present: true,
});
const resumedGeminiResult = await invokeModelChatLive('@google/gemini-cli-sdk', {
  model_request_id: 'req-gemini-sdk-resume',
  session_id: 'session-canonical-gemini-resume',
  provider_session_id: 'gemini-sdk-existing',
  messages: ['Resume Gemini'],
});
assert.equal(resumedGeminiResult.provider_session_id, 'gemini-sdk-existing');
assert.equal(resumedGeminiResult[VERIFIED_PROVIDER_SESSION_ID], true);
assert.equal(
  JSON.parse(fs.readFileSync(geminiCapturePath, 'utf8')).resume_session_id,
  'gemini-sdk-existing',
  'Gemini resume must use the official agent.resumeSession API.',
);
await assert.rejects(
  invokeModelChatLive('@google/gemini-cli-sdk', {
    model_request_id: 'req-gemini-sdk-mismatch',
    session_id: 'session-canonical-gemini-mismatch',
    provider_session_id: 'gemini-sdk-mismatch-request',
    messages: ['Mismatch Gemini'],
  }),
  /resumed a different provider session/,
);

process.env.SDKWORK_AGENT_SDK_PACKAGE_PATHS = JSON.stringify({
  '@opencode-ai/sdk': opencodeSdkMirror,
});
const opencodeResult = await invokeModelChatLive('@opencode-ai/sdk', {
  model_request_id: 'req-opencode-sdk-new',
  session_id: 'session-canonical-opencode-new',
  model_id: 'opencode/big-pickle',
  working_directory: 'C:/sdkwork/opencode-workspace',
  timeout_ms: 2_000,
  messages: ['OpenCode prompt'],
  execution_options: { approval_policy: 'allow-edits' },
});
assert.equal(opencodeResult.provider_session_id, 'opencode-sdk-created');
assert.equal(opencodeResult[VERIFIED_PROVIDER_SESSION_ID], true);
assert.deepEqual(opencodeResult.messages, ['opencode sdk:OpenCode prompt']);
const opencodeCapture = JSON.parse(fs.readFileSync(opencodeCapturePath, 'utf8'));
assert.deepEqual(opencodeCapture.client_options, {
  base_url: 'http://127.0.0.1:4096',
  directory: 'C:/sdkwork/opencode-workspace',
});
assert.deepEqual(opencodeCapture.session_create, {
  parameters: {
    permission: [
      { permission: '*', pattern: '*', action: 'ask' },
      { permission: 'read', pattern: '*', action: 'allow' },
      { permission: 'edit', pattern: '*', action: 'allow' },
      { permission: 'glob', pattern: '*', action: 'allow' },
      { permission: 'grep', pattern: '*', action: 'allow' },
      { permission: 'list', pattern: '*', action: 'allow' },
    ],
    model: { id: 'big-pickle', providerID: 'opencode' },
  },
  signal_present: true,
});
assert.deepEqual(opencodeCapture.session_prompt, {
  parameters: {
    sessionID: 'opencode-sdk-created',
    parts: [{ type: 'text', text: 'OpenCode prompt' }],
    model: { providerID: 'opencode', modelID: 'big-pickle' },
  },
  signal_present: true,
});
assert.deepEqual(opencodeCapture.event_subscribe, {
  parameters: {},
  signal_present: true,
});
const opencodeStreamChunks = [];
const opencodeStreamEvents = [];
const opencodeStreamResult = await invokeModelChatStreamLive(
  '@opencode-ai/sdk',
  {
    model_request_id: 'req-opencode-sdk-stream',
    session_id: 'session-canonical-opencode-stream',
    turn_id: 'turn-canonical-opencode-stream',
    messages: ['OpenCode streamed prompt'],
    timeout_ms: 2_000,
  },
  {
    onChunk: async (chunk) => opencodeStreamChunks.push(chunk),
    onEvent: async (event) => opencodeStreamEvents.push(event),
  },
);
assert.equal(opencodeStreamResult.provider_session_id, 'opencode-sdk-created');
assert.deepEqual(opencodeStreamResult.chunks, []);
assert.deepEqual(opencodeStreamChunks, [
  { sequence: 0, content: 'opencode sdk:' },
  { sequence: 1, content: 'OpenCode streamed prompt' },
]);
assert.equal(opencodeStreamEvents[0].event_type, 'agent.turn.started');
assert.equal(opencodeStreamEvents.at(-1).event_type, 'agent.turn.completed');
assert.equal(opencodeStreamEvents.at(-1).step_id, 'turn-canonical-opencode-stream');
assert.equal(
  opencodeStreamEvents.some((event) => (
    event.payload.rawProviderPayload?.properties?.part?.id === 'post-terminal'
  )),
  false,
  'OpenCode must stop consuming the matching Session stream at session.idle',
);
assert.ok(
  opencodeStreamEvents.some((event) => event.event_type === 'agent.reasoning.completed'),
);
const resumedOpencodeActivity = [];
const resumedOpencodeResult = await invokeModelChatLive(
  '@opencode-ai/sdk',
  {
    model_request_id: 'req-opencode-sdk-resume',
    session_id: 'session-canonical-opencode-resume',
    provider_session_id: 'opencode-sdk-existing',
    messages: ['Resume OpenCode'],
    execution_options: { approval_policy: 'allow-all' },
  },
  { onActivity: async (event) => resumedOpencodeActivity.push(event) },
);
assert.equal(resumedOpencodeResult.provider_session_id, 'opencode-sdk-existing');
assert.equal(resumedOpencodeResult[VERIFIED_PROVIDER_SESSION_ID], true);
assert.deepEqual(
  resumedOpencodeActivity.map((event) => event.phase),
  ['started', 'working', 'working', 'working', 'idle', 'terminal'],
);
const resumedOpencodeCapture = JSON.parse(fs.readFileSync(opencodeCapturePath, 'utf8'));
assert.equal(
  Object.hasOwn(resumedOpencodeCapture, 'session_create'),
  false,
  'OpenCode resume must not create another provider session.',
);
assert.deepEqual(resumedOpencodeCapture.session_get, {
  parameters: { sessionID: 'opencode-sdk-existing' },
  signal_present: true,
});
assert.deepEqual(resumedOpencodeCapture.session_update, {
  parameters: {
    sessionID: 'opencode-sdk-existing',
    permission: [{ permission: '*', pattern: '*', action: 'allow' }],
  },
  signal_present: true,
});
assert.equal(
  resumedOpencodeCapture.session_prompt.parameters.sessionID,
  'opencode-sdk-existing',
);

const mismatchedOpencodeActivity = [];
await assert.rejects(
  invokeModelChatLive(
    '@opencode-ai/sdk',
    {
      model_request_id: 'req-opencode-sdk-mismatch',
      session_id: 'session-canonical-opencode-mismatch',
      provider_session_id: 'opencode-sdk-mismatch-request',
      messages: ['Do not invoke the mismatched session'],
    },
    { onActivity: async (event) => mismatchedOpencodeActivity.push(event) },
  ),
  /resumed a different provider session/,
);
assert.deepEqual(mismatchedOpencodeActivity, []);

process.env.SDKWORK_AGENT_SDK_PACKAGE_PATHS = JSON.stringify({
  '@opencode-ai/sdk': opencodeDurableMirror,
});
const durableOpencodeResult = await invokeModelChatLive('@opencode-ai/sdk', {
  model_request_id: 'req-opencode-durable',
  session_id: 'session-canonical-opencode-durable',
  working_directory: 'C:/sdkwork/opencode-workspace',
  timeout_ms: 2_000,
  messages: ['OpenCode durable prompt'],
});
assert.equal(durableOpencodeResult.provider_session_id, 'opencode-durable-created');
assert.equal(durableOpencodeResult[VERIFIED_PROVIDER_SESSION_ID], true);
assert.deepEqual(
  durableOpencodeResult.messages,
  ['opencode durable:OpenCode durable prompt'],
  'Durable v2 turns must assemble assistant text from the event stream',
);
const durableOpencodeCapture = JSON.parse(
  fs.readFileSync(opencodeDurableCapturePath, 'utf8'),
);
assert.equal(
  durableOpencodeCapture.session_prompt_v2.parameters.sessionID,
  'opencode-durable-created',
);
assert.match(
  durableOpencodeCapture.session_prompt_v2.parameters.id,
  /^msg_req-opencode-durable_[a-z0-9]+$/,
  'Durable prompt ids must be unique per admission',
);
assert.deepEqual(durableOpencodeCapture.session_prompt_v2.parameters.prompt, {
  text: 'OpenCode durable prompt',
});
assert.equal(durableOpencodeCapture.session_prompt_v2.parameters.delivery, 'steer');
assert.equal(durableOpencodeCapture.session_prompt_v2.parameters.resume, true);
assert.equal(durableOpencodeCapture.session_prompt_v2.signal_present, true);assert.deepEqual(durableOpencodeCapture.event_subscribe_v2, {
  parameters: {},
  signal_present: true,
});
assert.deepEqual(durableOpencodeCapture.session_active_v2, {
  parameters: {},
  signal_present: true,
});
assert.equal(
  Object.hasOwn(durableOpencodeCapture, 'session_prompt'),
  false,
  'Durable opencode turns must use the v2 prompt route, not the legacy one.',
);
assert.equal(
  Object.hasOwn(durableOpencodeCapture, 'event_subscribe'),
  false,
  'Durable opencode turns must use the v2 event route, not the legacy one.',
);

const durableOpencodeStreamChunks = [];
const durableOpencodeStreamEvents = [];
const durableOpencodeStreamResult = await invokeModelChatStreamLive(
  '@opencode-ai/sdk',
  {
    model_request_id: 'req-opencode-durable-stream',
    session_id: 'session-canonical-opencode-durable-stream',
    turn_id: 'turn-canonical-opencode-durable-stream',
    messages: ['OpenCode durable streamed'],
    timeout_ms: 2_000,
  },
  {
    onChunk: async (chunk) => durableOpencodeStreamChunks.push(chunk),
    onEvent: async (event) => durableOpencodeStreamEvents.push(event),
  },
);
assert.equal(durableOpencodeStreamResult.provider_session_id, 'opencode-durable-created');
assert.deepEqual(durableOpencodeStreamResult.chunks, []);
assert.deepEqual(durableOpencodeStreamChunks, [
  { sequence: 0, content: 'opencode durable:' },
  { sequence: 1, content: 'OpenCode durable streamed' },
]);
assert.equal(durableOpencodeStreamEvents[0].event_type, 'agent.turn.started');
assert.equal(durableOpencodeStreamEvents.at(-1).event_type, 'agent.turn.completed');
assert.ok(
  durableOpencodeStreamEvents.some((event) => event.event_type === 'agent.reasoning.completed'),
);
assert.equal(
  durableOpencodeStreamEvents.some((event) => (
    event.payload.rawProviderPayload?.data?.part?.id === 'post-terminal'
  )),
  false,
  'Durable opencode streams must stop consuming events at session.idle',
);
assert.ok(
  durableOpencodeStreamEvents.some((event) => (
    event.payload.rawProviderPayload?.id === 'evt-6-inner'
    && event.payload.rawProviderPayload?.type === 'session.status'
  )),
  'Durable sync envelopes must be unwrapped to the plain event type',
);

const durableOpencodeResumeResult = await invokeModelChatLive(
  '@opencode-ai/sdk',
  {
    model_request_id: 'req-opencode-durable-resume',
    session_id: 'session-canonical-opencode-durable-resume',
    provider_session_id: 'opencode-durable-existing',
    messages: ['Resume durable'],
  },
);
assert.equal(durableOpencodeResumeResult.provider_session_id, 'opencode-durable-existing');
assert.equal(durableOpencodeResumeResult[VERIFIED_PROVIDER_SESSION_ID], true);
const durableOpencodeResumeCapture = JSON.parse(
  fs.readFileSync(opencodeDurableCapturePath, 'utf8'),
);
assert.equal(
  Object.hasOwn(durableOpencodeResumeCapture, 'session_create'),
  false,
  'Durable opencode resume must not create another provider session.',
);
assert.equal(
  durableOpencodeResumeCapture.session_prompt_v2.parameters.sessionID,
  'opencode-durable-existing',
);
assert.equal(
  durableOpencodeResumeCapture.session_prompt_v2.parameters.resume,
  true,
  'Durable resume turns must wake the session agent loop.',
);

process.env.SDKWORK_AGENT_SDK_PACKAGE_PATHS = JSON.stringify({
  '@opencode-ai/sdk': opencodeSdkMirror,
});
process.env.OPENCODE_SERVER_URL = 'http://127.0.0.1:4096';
const opencodeSessions = await invokeSessionDiscoveryRuntime('@opencode-ai/sdk', {
  operation: 'session_list',
  working_directory: 'C:/sdkwork/opencode-workspace',
  limit: 3,
});
assert.deepEqual(opencodeSessions.items, [{
  provider_session_id: 'opencode-history-1',
  parent_provider_session_id: 'opencode-history-parent',
  title: 'OpenCode history',
  created_at: '2026-02-01T01:02:03.000Z',
  updated_at: '2026-02-02T03:04:05.000Z',
  cwd: 'C:/sdkwork/opencode-workspace',
  model: 'claude-sonnet-4-6',
  model_provider: 'anthropic',
  input_tokens: 10,
  output_tokens: 20,
  cached_tokens: 4,
  reasoning_tokens: 3,
  cost_cents: 2500,
  additions: 6,
  deletions: 2,
  files_changed: 3,
  metadata: {
    project_id: 'project-1',
    workspace_id: 'workspace-1',
    agent: 'build',
    cache_write_tokens: 5,
  },
}]);
assert.equal(opencodeSessions.previous_cursor, 'opencode-session-previous');
assert.equal(opencodeSessions.next_cursor, 'opencode-session-next');
assert.deepEqual(
  JSON.parse(fs.readFileSync(opencodeCapturePath, 'utf8')).session_list_v2,
  {
    parameters: {
      directory: 'C:/sdkwork/opencode-workspace',
      limit: 3,
      order: 'desc',
    },
    signal_present: true,
  },
);
await invokeSessionDiscoveryRuntime('@opencode-ai/sdk', {
  operation: 'session_list',
  working_directory: 'C:/sdkwork/opencode-workspace',
  limit: 3,
  cursor: opencodeSessions.next_cursor,
});
assert.deepEqual(
  JSON.parse(fs.readFileSync(opencodeCapturePath, 'utf8')).session_list_v2.parameters,
  {
    directory: 'C:/sdkwork/opencode-workspace',
    limit: 3,
    cursor: 'opencode-session-next',
  },
  'OpenCode cursors must be passed back to the official SDK without interpretation',
);
const opencodeHistory = await invokeSessionDiscoveryRuntime('@opencode-ai/sdk', {
  operation: 'session_history',
  provider_session_id: 'opencode-history-1',
  working_directory: 'C:/sdkwork/opencode-workspace',
  limit: 7,
});
assert.deepEqual(opencodeHistory.items, [{
  provider_message_id: 'opencode-message-1',
  provider_session_id: 'opencode-history-1',
  role: 'agent',
  parts: [{
    part_id: 'opencode-part-1',
    kind: 'text',
    text: 'Kernel reviewed',
  }, {
    part_id: 'opencode-part-2',
    kind: 'text',
    text: 'Inspect the SDK adapter',
    metadata: { 'sdkwork.provider.content_type': 'reasoning' },
  }, {
    part_id: 'opencode-part-3',
    kind: 'tool_call_ref',
    tool_call_id: 'opencode-call-1',
    name: 'read',
    json: {
      id: 'opencode-part-3',
      sessionID: 'opencode-history-1',
      messageID: 'opencode-message-1',
      type: 'tool',
      callID: 'opencode-call-1',
      tool: 'read',
      state: {
        status: 'completed',
        input: { filePath: 'src/lib.rs' },
        content: [{ type: 'text', text: 'source' }],
        structured: {},
        result: 'source',
      },
    },
    metadata: {
      'sdkwork.provider.content_type': 'tool',
      'sdkwork.provider.status': 'completed',
      'sdkwork.provider.has_result': true,
    },
  }],
  created_at: '2026-02-02T03:04:06.000Z',
}]);
assert.equal(opencodeHistory.previous_cursor, undefined);
assert.equal(opencodeHistory.next_cursor, 'opencode-message-next');
assert.deepEqual(
  JSON.parse(fs.readFileSync(opencodeCapturePath, 'utf8')).session_messages_v2,
  {
    parameters: { sessionID: 'opencode-history-1', limit: 7, order: 'asc' },
    signal_present: true,
  },
);
await assert.rejects(
  invokeSessionDiscoveryRuntime('@opencode-ai/sdk', {
    operation: 'session_history',
    provider_session_id: 'opencode-history-mismatch',
    limit: 7,
  }),
  /resumed a different provider session than requested/,
  'OpenCode history items must retain the requested provider Session affinity',
);
delete process.env.OPENCODE_SERVER_URL;

process.env.SDKWORK_AGENT_SDK_PACKAGE_PATHS = JSON.stringify({
  '@openai/codex-sdk': codexSdkMirror,
});
const codexOperation = {
  model_request_id: 'req-codex-sdk',
  model_id: 'gpt-5.4',
  session_id: 'session-canonical-sdk',
  provider_session_id: 'thread-sdk-existing',
  working_directory: 'C:/sdkwork/workspace',
  timeout_ms: 2_000,
  messages: ['legacy prompt'],
  wire_messages: [{ role: 'user', content: [{ type: 'text', text: 'official sdk prompt' }] }],
  execution_options: {
    approval_policy: 'onrequest',
    sandbox_mode: 'workspace_write',
    approvals_reviewer: 'auto_review',
    full_auto: false,
    skip_git_repo_check: true,
  },
};
const codexResult = await invokeModelChatLive('@openai/codex-sdk', codexOperation);
assert.equal(codexResult.ok, true);
assert.equal(codexResult.mode, 'sdk_live');
assert.equal(codexResult.provider_session_id, 'thread-sdk-existing');
assert.deepEqual(codexResult.messages, ['official sdk:official sdk prompt']);
const codexCapture = JSON.parse(fs.readFileSync(codexCapturePath, 'utf8'));
assert.deepEqual(codexCapture.constructor_options, {
  config: { approvals_reviewer: 'auto_review' },
});
assert.equal(codexCapture.resume_thread_id, 'thread-sdk-existing');
assert.deepEqual(codexCapture.resume_thread_options, {
  model: 'gpt-5.4',
  workingDirectory: 'C:/sdkwork/workspace',
  sandboxMode: 'workspace-write',
  approvalPolicy: 'on-request',
  skipGitRepoCheck: true,
});
assert.equal(codexCapture.run.prompt, 'official sdk prompt');
assert.equal(codexCapture.run.signal_present, true);

const codexStreamResult = await invokeModelChatStreamLive('@openai/codex-sdk', {
  model_request_id: 'req-codex-sdk-stream',
  messages: ['stream prompt'],
  timeout_ms: 2_000,
});
assert.equal(codexStreamResult.provider_session_id, 'thread-sdk-streamed');
assert.deepEqual(codexStreamResult.chunks, [
  { sequence: 0, content: 'official' },
  { sequence: 1, content: ' sdk' },
  { sequence: 2, content: ' stream' },
]);

const deliveredCodexChunks = [];
const deliveredCodexEvents = [];
const callbackCodexStreamResult = await invokeModelChatStreamLive(
  '@openai/codex-sdk',
  {
    model_request_id: 'req-codex-sdk-stream-callback',
    session_id: 'session-canonical-stream',
    turn_id: 'turn-canonical-stream',
    messages: ['stream callback prompt'],
    timeout_ms: 2_000,
  },
  {
    onChunk: async (chunk) => {
      deliveredCodexChunks.push(chunk);
    },
    onEvent: async (event) => {
      deliveredCodexEvents.push(event);
    },
  },
);
assert.deepEqual(deliveredCodexChunks, [
  { sequence: 0, content: 'official' },
  { sequence: 1, content: ' sdk' },
  { sequence: 2, content: ' stream' },
]);
assert.deepEqual(
  callbackCodexStreamResult.chunks,
  [],
  'callback delivery must not retain every Codex chunk in the worker result',
);
assert.equal(callbackCodexStreamResult.provider_session_id, 'thread-sdk-streamed');
assert.deepEqual(
  deliveredCodexEvents.map((event) => event.event_type),
  [
    'agent.session.started',
    'agent.message.updated',
    'agent.message.updated',
    'agent.message.completed',
    'agent.turn.completed',
  ],
);
assert.equal(deliveredCodexEvents[0].session_id, 'session-canonical-stream');
assert.equal(
  deliveredCodexEvents[0].payload.providerSessionId,
  'thread-sdk-streamed',
);
assert.equal(Object.hasOwn(deliveredCodexEvents[0].payload, 'threadId'), false);
assert.equal(deliveredCodexEvents[1].step_id, 'turn-canonical-stream');
assert.equal(deliveredCodexEvents[1].payload.providerItemId, 'message-1');
assert.equal(deliveredCodexEvents[1].correlation_id, 'req-codex-sdk-stream-callback');
assert.equal(deliveredCodexEvents[1].payload.providerEventType, 'item.updated');
assert.equal(deliveredCodexEvents[1].payload.item.text, 'official');

const commandKernelEvent = buildCodexKernelStreamEvent(
  {
    type: 'item.completed',
    item: {
      id: 'command-1',
      type: 'command_execution',
      command: 'pnpm test',
      aggregated_output: 'passed',
      status: 'completed',
    },
  },
  {
    model_request_id: 'req-command',
    session_id: 'session-canonical-command',
    turn_id: 'turn-canonical-command',
  },
  'thread-command',
  3,
);
assert.equal(commandKernelEvent.event_type, 'agent.tool.completed');
assert.equal(commandKernelEvent.source, 'tool');
assert.equal(commandKernelEvent.step_id, 'turn-canonical-command');
assert.equal(commandKernelEvent.payload.providerItemId, 'command-1');
assert.equal(commandKernelEvent.payload.sequence, 3);
assert.equal(commandKernelEvent.session_id, 'session-canonical-command');
assert.equal(commandKernelEvent.payload.providerSessionId, 'thread-command');

const failedCodexChunks = [];
await assert.rejects(
  invokeModelChatStreamLive(
    '@openai/codex-sdk',
    {
      model_request_id: 'req-codex-sdk-stream-error',
      messages: ['stream emits fatal error'],
      timeout_ms: 2_000,
    },
    {
      onChunk: async (chunk) => {
        failedCodexChunks.push(chunk);
      },
    },
  ),
  /stream transport failed/,
);
assert.deepEqual(failedCodexChunks, [{ sequence: 0, content: 'official' }]);
await assert.rejects(
  invokeModelChatStreamLive('@openai/codex-sdk', {
    model_request_id: 'req-codex-sdk-stream-incomplete',
    messages: ['stream ends incomplete'],
    timeout_ms: 2_000,
  }),
  /missing turn\.completed event/,
);

const newThreadResult = await invokeModelChatLive('@openai/codex-sdk', {
  model_request_id: 'req-codex-sdk-new',
  messages: ['new thread'],
  execution_options: { full_auto: true },
});
assert.equal(newThreadResult.provider_session_id, 'thread-sdk-started');
const newThreadCapture = JSON.parse(fs.readFileSync(codexCapturePath, 'utf8'));
assert.deepEqual(newThreadCapture.start_thread_options, {
  sandboxMode: 'workspace-write',
  approvalPolicy: 'on-failure',
});
await invokeModelChatLive('@openai/codex-sdk', {
  model_request_id: 'req-codex-sdk-full-access',
  messages: ['allow configured full access'],
  execution_options: {
    sandbox_mode: 'danger-full-access',
    approval_policy: 'never',
  },
});
const fullAccessCapture = JSON.parse(fs.readFileSync(codexCapturePath, 'utf8'));
assert.equal(fullAccessCapture.start_thread_options.sandboxMode, 'danger-full-access');
assert.equal(fullAccessCapture.start_thread_options.approvalPolicy, 'never');

process.env.SDKWORK_AGENT_SDK_PACKAGE_PATHS = JSON.stringify({ openai: openaiSdkMirror });
process.env.OPENCLAW_GATEWAY_URL = 'http://127.0.0.1:18789';
process.env.OPENCLAW_GATEWAY_TOKEN = 'gateway-test-token';
const openclawResult = await invokeModelChatLive('openai', {
  model_request_id: 'req-openclaw-sdk',
  model_id: 'default',
  messages: ['gateway prompt'],
});
assert.equal(openclawResult.ok, true);
assert.equal(openclawResult.messages[0], 'openclaw sdk:gateway prompt');
const openaiCapture = JSON.parse(fs.readFileSync(openaiCapturePath, 'utf8'));
assert.equal(openaiCapture.options.baseURL, 'http://127.0.0.1:18789/v1');
assert.equal(openaiCapture.options.apiKey, 'gateway-test-token');
assert.equal(openaiCapture.request.model, 'default');
delete process.env.OPENCLAW_GATEWAY_URL;
delete process.env.OPENCLAW_GATEWAY_TOKEN;

process.env.SDKWORK_KERNEL_PROFILE_ID = 'cloud.production';
process.env.SDKWORK_KERNEL_ENVIRONMENT = 'production';
delete process.env.SDKWORK_KERNEL_ALLOW_MOCK_PROVIDERS;
assert.equal(mockProviderInvocationAllowed(), false, 'production topology profile should block mock fallback');

console.log('engine-sdk-live contract passed.');
