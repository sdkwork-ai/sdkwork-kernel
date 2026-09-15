// WORKSPACE-PATH:allow-fixture: fixtures name a foreign checkout root, drive, or home directory to exercise path handling, so the literal is the value under assertion rather than a binding this build resolves
import assert from 'node:assert/strict';
import test from 'node:test';

import {
  buildCodexInteractionResponse,
  normalizeCodexInteractionRequest,
  projectCodexInteractionServerRequest,
} from './codex-app-server-interactions.mjs';

const common = {
  providerSessionId: 'provider-session-1',
  receivedAt: '2026-08-01T00:00:00.000Z',
  requestId: 41,
  turnId: 'provider-turn-1',
};

test('normalizes every user-mediated Codex request into canonical Session interaction data', () => {
  const command = normalizeCodexInteractionRequest({
    ...common,
    method: 'item/commandExecution/requestApproval',
    params: {
      command: 'pnpm test',
      commandActions: [{ type: 'run', command: 'pnpm test' }],
      cwd: 'E:\\workspace',
      environmentId: 'local',
      itemId: 'command-1',
      networkApprovalContext: { host: 'registry.npmjs.org', protocol: 'https' },
      proposedExecpolicyAmendment: { command: ['pnpm', 'test'] },
      proposedNetworkPolicyAmendments: [{ host: 'registry.npmjs.org', action: 'allow' }],
      providerSessionId: common.providerSessionId,
      reason: 'Install dependencies',
      startedAtMs: 1_785_542_400_000,
      turnId: common.turnId,
    },
  }, { modelRequestId: 'model-request-1', sessionId: 'session.canonical' });

  assert.equal(command.sessionId, 'session.canonical');
  assert.equal(command.category, 'approval');
  assert.equal(command.kind, 'command_execution');
  assert.equal(command.interactionId, '41');
  assert.equal(command.correlation.modelRequestId, 'model-request-1');
  assert.equal(command.correlation.providerRequestId, 41);
  assert.equal(command.correlation.providerRequestIdType, 'number');
  assert.equal(command.request.command, 'pnpm test');
  assert.deepEqual(command.allowedActions, [
    'accept',
    'accept_for_session',
    'accept_with_exec_policy_amendment',
    'apply_network_policy_amendment',
    'decline',
    'cancel',
  ]);
  assert.equal(JSON.stringify(command).includes('thread'), false);

  const fileChange = normalizeCodexInteractionRequest({
    ...common,
    method: 'item/fileChange/requestApproval',
    requestId: 'file-approval-1',
    params: {
      grantRoot: 'E:\\workspace',
      itemId: 'file-change-1',
      providerSessionId: common.providerSessionId,
      reason: 'Apply patch',
      startedAtMs: 1_785_542_400_001,
      turnId: common.turnId,
    },
  }, { sessionId: 'session.canonical' });
  assert.equal(fileChange.kind, 'file_change');
  assert.equal(fileChange.request.grantRoot, 'E:\\workspace');

  const questions = normalizeCodexInteractionRequest({
    ...common,
    method: 'item/tool/requestUserInput',
    requestId: 'question-1',
    params: {
      autoResolutionMs: 60_000,
      isBlocking: false,
      itemId: 'tool-1',
      providerSessionId: common.providerSessionId,
      questions: [{
        header: 'Scope',
        id: 'scope',
        isOther: true,
        isSecret: false,
       options: [{ description: '', label: 'Session' }],
        question: 'Where should this permission apply?',
      }],
      turnId: common.turnId,
    },
  }, { sessionId: 'session.canonical' });
  assert.equal(questions.request.autoResolutionMs, 60_000);
  assert.equal(questions.request.isBlocking, false);
  assert.equal(questions.category, 'user_input');
  assert.equal(questions.kind, 'question_set');
  assert.equal(questions.request.questions[0].allowOther, true);
  assert.equal(questions.request.questions[0].secret, false);
  assert.equal(questions.request.questions[0].options[0].description, '');

  const elicitation = normalizeCodexInteractionRequest({
    ...common,
    method: 'mcpServer/elicitation/request',
    requestId: 'mcp-1',
    params: {
      _meta: { source: 'fixture' },
      message: 'Choose deployment details',
      mode: 'form',
      providerSessionId: common.providerSessionId,
      requestedSchema: { type: 'object' },
      serverName: 'deployments',
      turnId: common.turnId,
    },
  }, { sessionId: 'session.canonical' });
  assert.equal(elicitation.category, 'elicitation');
  assert.equal(elicitation.kind, 'mcp_elicitation');
  assert.deepEqual(elicitation.request.metadata, { source: 'fixture' });

  const openAiElicitation = normalizeCodexInteractionRequest({
    ...common,
    method: 'mcpServer/elicitation/request',
    requestId: 'openai-mcp-1',
    params: {
      _meta: null,
      message: 'Choose a report',
      mode: 'openai/form',
      providerSessionId: common.providerSessionId,
      requestedSchema: ['openai/imagePicker', null, true],
      serverName: 'reports',
      turnId: common.turnId,
    },
  }, { sessionId: 'session.canonical' });
  assert.deepEqual(
    openAiElicitation.request.requestedSchema,
    ['openai/imagePicker', null, true],
  );

  const nullOpenAiElicitation = normalizeCodexInteractionRequest({
    ...common,
    method: 'mcpServer/elicitation/request',
    requestId: 'openai-mcp-null',
    params: {
      _meta: null,
      message: 'Choose a report',
      mode: 'openai/form',
      providerSessionId: common.providerSessionId,
      requestedSchema: null,
      serverName: 'reports',
      turnId: common.turnId,
    },
  }, { sessionId: 'session.canonical' });
  assert.equal(nullOpenAiElicitation.request.requestedSchema, null);

  const permissions = normalizeCodexInteractionRequest({
    ...common,
    method: 'item/permissions/requestApproval',
    requestId: 'permission-1',
    params: {
      cwd: 'E:\\workspace',
      environmentId: 'local',
      itemId: 'permission-item-1',
      permissions: {
        fileSystem: { read: ['E:\\workspace'], write: ['E:\\workspace\\src'] },
        network: { enabled: true },
      },
      providerSessionId: common.providerSessionId,
      reason: 'Run project tests',
      startedAtMs: 1_785_542_400_002,
      turnId: common.turnId,
    },
  }, { sessionId: 'session.canonical' });
  assert.equal(permissions.kind, 'permission_profile');
  assert.equal(permissions.request.cwd, 'E:\\workspace');
  assert.deepEqual(permissions.request.requestedPermissions.network, { enabled: true });
});

test('normalizes desktop option, onboarding, context, and setup requests without leaking Thread', () => {
  const directOption = normalizeCodexInteractionRequest({
    ...common,
    method: 'item/tool/requestOptionPicker',
    params: {
      allowMultiple: true,
      options: [{ label: 'Desktop', description: null }, { label: 'Web' }],
      providerSessionId: common.providerSessionId,
      question: 'Choose surfaces',
      skipLabel: 'Later',
      submitLabel: 'Continue',
      turnId: common.turnId,
    },
  }, { sessionId: 'session.canonical' });
  assert.equal(directOption.kind, 'option_picker');
  assert.equal(directOption.request.options[0].description, null);
  assert.equal(directOption.request.options[1].description, null);
  assert.deepEqual(directOption.allowedActions, ['submit', 'skip', 'dismiss']);

  const onboarding = normalizeDynamic('onboarding-1', 'request_onboarding_input', {
    questions: [{
      header: null,
      id: 'first_task',
      options: [{ label: 'Fix tests', description: '' }, { label: 'Review code' }],
      question: 'What should we do first?',
    }],
  });
  assert.equal(onboarding.category, 'user_input');
  assert.equal(onboarding.kind, 'onboarding_question_set');
  assert.equal(onboarding.request.presentation, 'onboarding');
  assert.equal(onboarding.request.questions[0].header, 'What should we do first?');
  assert.equal(onboarding.request.questions[0].allowOther, true);
  assert.equal(onboarding.request.questions[0].secret, false);
  assert.equal(onboarding.correlation.providerToolCallId, 'call-request_onboarding_input');
  assert.equal(onboarding.correlation.providerToolName, 'request_onboarding_input');

  const dynamicOption = normalizeDynamic('option-1', 'request_option_picker', {
    allowMultiple: false,
    options: [{ label: 'Local', extra: 'discarded' }],
    question: 'Choose a workspace',
  });
  assert.equal(dynamicOption.kind, 'option_picker');
  assert.equal(dynamicOption.request.allowMultiple, false);
  assert.equal(Object.hasOwn(dynamicOption.request.options[0], 'extra'), false);

  const context = normalizeDynamic('context-1', 'setup_codex_context_picker', {});
  assert.equal(context.kind, 'context_source_picker');
  assert.deepEqual(context.allowedActions, ['continue', 'skip', 'dismiss']);

  for (const [step, actions] of [
    ['role', ['submit', 'skip', 'dismiss']],
    ['task', ['submit', 'skip', 'dismiss']],
    ['context', ['continue', 'skip', 'dismiss']],
  ]) {
    const setup = normalizeDynamic(`setup-${step}`, 'setup_codex_step', { step });
    assert.equal(setup.category, 'setup');
    assert.equal(setup.kind, 'setup_step');
    assert.equal(setup.request.step, step);
    assert.deepEqual(setup.allowedActions, actions);
    assert.equal(JSON.stringify(setup).includes('thread'), false);
  }
});

test('projects desktop setup completion and invalid arguments into exact host responses', () => {
  const completed = projectCodexInteractionServerRequest(dynamicRequest(
    71,
    'setup_codex_step',
    { step: 'complete' },
  ), { sessionId: 'session.canonical' });
  assert.deepEqual(completed, {
    disposition: 'automatic_response',
    result: {
      contentItems: [{ type: 'inputText', text: '{"completed":true}' }],
      success: true,
    },
  });

  for (const [requestId, tool, args] of [
    [72, 'request_option_picker', { options: [], question: 42 }],
    [73, 'request_onboarding_input', { questions: [] }],
    [74, 'setup_codex_step', { step: 'plugin' }],
    [75, 'setup_codex_step', { step: 'role', unexpected: true }],
  ]) {
    const projection = projectCodexInteractionServerRequest(
      dynamicRequest(requestId, tool, args),
      { sessionId: 'session.canonical' },
    );
    assert.deepEqual(projection, {
      disposition: 'automatic_response',
      result: {
        contentItems: [{ type: 'inputText', text: `${tool} received invalid arguments.` }],
        success: false,
      },
    });
  }
});

test('compiles canonical resolutions into exact Codex app-server response payloads', () => {
  const command = normalize('item/commandExecution/requestApproval', 'command-1', {});
  assert.deepEqual(buildCodexInteractionResponse(command, {
    action: 'accept_with_exec_policy_amendment',
    execPolicyAmendment: { command: ['pnpm', 'test'] },
  }), {
    decision: {
      acceptWithExecpolicyAmendment: {
        execpolicy_amendment: { command: ['pnpm', 'test'] },
      },
    },
  });
  assert.deepEqual(buildCodexInteractionResponse(command, {
    action: 'apply_network_policy_amendment',
    networkPolicyAmendment: { action: 'allow', host: 'registry.npmjs.org' },
  }), {
    decision: {
      applyNetworkPolicyAmendment: {
        network_policy_amendment: { action: 'allow', host: 'registry.npmjs.org' },
      },
    },
  });

  const fileChange = normalize('item/fileChange/requestApproval', 'file-1', {});
  assert.deepEqual(buildCodexInteractionResponse(fileChange, {
    action: 'accept_for_session',
  }), { decision: 'acceptForSession' });

  const questions = normalize('item/tool/requestUserInput', 'question-1', {
    questions: [
      { id: 'scope', header: 'Scope', question: 'Choose scope', isOther: false, isSecret: false, options: null },
      { id: 'notes', header: 'Notes', question: 'Add notes', isOther: true, isSecret: false, options: null },
    ],
  });
  assert.deepEqual(buildCodexInteractionResponse(questions, {
    action: 'submit',
    answers: { notes: ['Keep tests'], scope: ['Session'] },
  }), {
    answers: {
      notes: { answers: ['Keep tests'] },
      scope: { answers: ['Session'] },
    },
  });

  const elicitation = normalize('mcpServer/elicitation/request', 'mcp-1', {
    message: 'Provide details',
    mode: 'form',
    requestedSchema: { type: 'object' },
    serverName: 'deployments',
  });
  assert.deepEqual(buildCodexInteractionResponse(elicitation, {
    action: 'accept',
    content: { region: 'ap-southeast-1' },
    metadata: { source: 'user' },
  }), {
    action: 'accept',
    content: { region: 'ap-southeast-1' },
    _meta: { source: 'user' },
  });

  const permissions = normalize('item/permissions/requestApproval', 'permission-1', {
    cwd: 'E:\\workspace',
    permissions: { fileSystem: null, network: null },
  });
  assert.deepEqual(buildCodexInteractionResponse(permissions, {
    action: 'grant',
    permissions: { fileSystem: { write: ['E:\\workspace'] } },
    scope: 'session',
    strictAutoReview: true,
  }), {
    permissions: { fileSystem: { write: ['E:\\workspace'] } },
    scope: 'session',
    strictAutoReview: true,
  });
  assert.deepEqual(buildCodexInteractionResponse(permissions, {
    action: 'decline',
  }), { permissions: {}, scope: 'turn' });
});

test('compiles desktop setup resolutions with direct and dynamic response encoding', () => {
  const directOption = normalize('item/tool/requestOptionPicker', 'direct-option-1', {
    options: [{ label: 'Local' }],
    question: 'Choose a workspace',
  });
  const optionResolution = {
    action: 'submit',
    selectedOptions: ['Local'],
    freeformAnswer: null,
  };
  assert.deepEqual(
    buildCodexInteractionResponse(directOption, optionResolution),
    optionResolution,
  );

  const dynamicOption = normalizeDynamic('dynamic-option-1', 'request_option_picker', {
    options: [{ label: 'Local' }],
    question: 'Choose a workspace',
  });
  assert.deepEqual(buildCodexInteractionResponse(dynamicOption, optionResolution), {
    contentItems: [{ type: 'inputText', text: JSON.stringify(optionResolution) }],
    success: true,
  });

  const onboarding = normalizeDynamic('onboarding-2', 'request_onboarding_input', {
    questions: [{
      id: 'first_task',
      options: [{ label: 'Fix tests' }, { label: 'Review code' }],
      question: 'What should we do first?',
    }],
  });
  const onboardingPayload = { answers: { first_task: { answers: ['Fix tests'] } } };
  assert.deepEqual(buildCodexInteractionResponse(onboarding, {
    action: 'submit',
    answers: { first_task: ['Fix tests'] },
  }), {
    contentItems: [{ type: 'inputText', text: JSON.stringify(onboardingPayload) }],
    success: true,
  });

  const context = normalizeDynamic('context-2', 'setup_codex_context_picker', {});
  const contextPayload = { action: 'continue', selectedSources: ['local-folder'] };
  assert.deepEqual(buildCodexInteractionResponse(context, contextPayload), {
    contentItems: [{ type: 'inputText', text: JSON.stringify(contextPayload) }],
    success: true,
  });

  const setupRole = normalizeDynamic('setup-role-2', 'setup_codex_step', { step: 'role' });
  const rolePayload = { action: 'submit', selectedRoles: ['engineering'] };
  assert.deepEqual(buildCodexInteractionResponse(setupRole, rolePayload), {
    contentItems: [{ type: 'inputText', text: JSON.stringify(rolePayload) }],
    success: true,
  });

  const setupTask = normalizeDynamic('setup-task-2', 'setup_codex_step', { step: 'task' });
  const taskPayload = {
    action: 'submit',
    answers: { first_task: { answers: ['Fix tests'] } },
  };
  assert.deepEqual(buildCodexInteractionResponse(setupTask, taskPayload), {
    contentItems: [{ type: 'inputText', text: JSON.stringify(taskPayload) }],
    success: true,
  });
});

test('fails closed for unknown methods and semantically invalid resolutions', () => {
  assert.throws(
    () => normalize('item/unknown/requestApproval', 'unknown-1', {}),
    hasCode('codex_interaction_unsupported_method'),
  );
  const command = normalize('item/commandExecution/requestApproval', 'command-1', {});
  assert.throws(
    () => buildCodexInteractionResponse(command, { action: 'accept_for_session_extra' }),
    hasCode('codex_interaction_invalid_resolution'),
  );
  const questions = normalize('item/tool/requestUserInput', 'question-1', {
    questions: [{ id: 'known', header: 'Known', question: 'Known?', isOther: false, isSecret: false, options: null }],
  });
  assert.throws(
    () => buildCodexInteractionResponse(questions, {
      action: 'submit',
      answers: { unknown: ['value'] },
    }),
    hasCode('codex_interaction_invalid_resolution'),
  );

  const elicitation = normalize('mcpServer/elicitation/request', 'mcp-1', {
    message: 'Provide details',
    mode: 'openai/form',
    requestedSchema: null,
    serverName: 'deployments',
  });
  const permissions = normalize('item/permissions/requestApproval', 'permission-1', {
    cwd: 'E:\\workspace',
    permissions: { fileSystem: null, network: null },
  });
  const dynamicOption = normalizeDynamic('option-invalid', 'request_option_picker', {
    options: [{ label: 'Local' }],
    question: 'Choose a workspace',
  });
  const setupTask = normalizeDynamic('setup-task-invalid', 'setup_codex_step', {
    step: 'task',
  });
  for (const invalidResolution of [
    () => buildCodexInteractionResponse(command, null),
    () => buildCodexInteractionResponse(command, { action: 42 }),
    () => buildCodexInteractionResponse(command, {
      action: 'accept_with_exec_policy_amendment',
      execPolicyAmendment: null,
    }),
    () => buildCodexInteractionResponse(questions, {
      action: 'submit',
      answers: { known: [42] },
    }),
    () => buildCodexInteractionResponse(elicitation, {
      action: 'accept',
      content: Number.POSITIVE_INFINITY,
    }),
    () => buildCodexInteractionResponse(permissions, {
      action: 'grant',
      permissions: null,
      scope: 'turn',
    }),
    () => buildCodexInteractionResponse(dynamicOption, {
      action: 'submit',
      selectedOptions: [42],
      freeformAnswer: null,
    }),
    () => buildCodexInteractionResponse(setupTask, {
      action: 'submit',
      answers: { first_task: { answers: [42] } },
    }),
  ]) {
    assert.throws(invalidResolution, hasCode('codex_interaction_invalid_resolution'));
  }
});

function normalize(method, requestId, params) {
  return normalizeCodexInteractionRequest({
    ...common,
    method,
    params: {
      itemId: 'item-1',
      providerSessionId: common.providerSessionId,
      turnId: common.turnId,
      ...params,
    },
    requestId,
  }, { modelRequestId: 'model-request-1', sessionId: 'session.canonical' });
}

function normalizeDynamic(requestId, tool, argumentsValue) {
  return normalizeCodexInteractionRequest(
    dynamicRequest(requestId, tool, argumentsValue),
    { modelRequestId: 'model-request-1', sessionId: 'session.canonical' },
  );
}

function dynamicRequest(requestId, tool, argumentsValue) {
  return {
    ...common,
    method: 'item/tool/call',
    params: {
      arguments: argumentsValue,
      callId: `call-${tool}`,
      namespace: null,
      providerSessionId: common.providerSessionId,
      tool,
      turnId: common.turnId,
    },
    requestId,
  };
}

function hasCode(code) {
  return (error) => error?.code === code;
}
