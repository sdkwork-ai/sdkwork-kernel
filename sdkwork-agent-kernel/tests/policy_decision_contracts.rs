// WORKSPACE-PATH:allow-fixture: fixtures name a foreign checkout root, drive, or home directory to exercise path handling, so the literal is the value under assertion rather than a binding this build resolves
use sdkwork_agent_kernel::{
    KernelEvent, KernelEventRedaction, KernelEventSeverity, KernelEventSource, PolicyCategory,
    PolicyDecision, PolicyDecisionConstraint, PolicyDecisionValue, PolicyProvider, PolicyRequest,
    PolicySubject, ProviderHealth, SideEffectLevel,
};

#[test]
fn policy_request_carries_typed_category_subject_resource_context_and_side_effect() {
    let request = PolicyRequest::new("policy-request.1", "host.process.execute", "cargo test")
        .with_category(PolicyCategory::HostProcessExecute)
        .with_subject(PolicySubject::new("user.1", "tenant.1").with_role("developer"))
        .with_action("execute")
        .with_session("session.1")
        .with_task("task.1")
        .with_run("run.1")
        .with_side_effect_level(SideEffectLevel::SideEffectful)
        .with_context("working_directory", "workspace")
        .with_redaction(KernelEventRedaction::Internal);

    assert_eq!(
        request.typed_category,
        Some(PolicyCategory::HostProcessExecute)
    );
    assert_eq!(request.category, "host.process.execute");
    assert_eq!(request.subject.as_ref().unwrap().subject_id, "user.1");
    assert_eq!(request.subject.as_ref().unwrap().roles, ["developer"]);
    assert_eq!(request.action.as_deref(), Some("execute"));
    assert_eq!(request.session_id.as_deref(), Some("session.1"));
    assert_eq!(request.task_id.as_deref(), Some("task.1"));
    assert_eq!(request.run_id.as_deref(), Some("run.1"));
    assert_eq!(
        request.side_effect_level,
        Some(SideEffectLevel::SideEffectful)
    );
    assert_eq!(
        request.context_value("working_directory"),
        Some("workspace")
    );
    assert_eq!(
        request.redaction_classification,
        KernelEventRedaction::Internal
    );
}

#[test]
fn policy_category_maps_standard_strings_and_product_namespaces() {
    assert_eq!(PolicyCategory::ModelInvoke.as_str(), "model.invoke");
    assert_eq!(
        PolicyCategory::HostFilesystemWrite.as_str(),
        "host.filesystem.write"
    );
    assert_eq!(
        PolicyCategory::ProductSpecific("birdcoder.session.share".to_string()).as_str(),
        "birdcoder.session.share"
    );
}

#[test]
fn policy_decision_carries_safe_reason_constraints_expiry_and_audit_requirement() {
    let decision = PolicyDecision::allow(
        "policy-decision.1",
        "policy-request.1",
        "provider.policy.fake",
    )
    .with_safe_reason("Allowed inside workspace")
    .with_created_at("2026-05-27T12:00:00Z")
    .with_expires_at("2026-05-27T12:05:00Z")
    .with_constraint(PolicyDecisionConstraint::new("workspace_root", "D:/repo"))
    .require_audit();

    assert_eq!(
        decision.safe_reason.as_deref(),
        Some("Allowed inside workspace")
    );
    assert_eq!(decision.created_at.as_deref(), Some("2026-05-27T12:00:00Z"));
    assert_eq!(decision.expires_at.as_deref(), Some("2026-05-27T12:05:00Z"));
    assert_eq!(decision.constraints[0].key, "workspace_root");
    assert!(decision.audit_required);
    assert!(decision.is_allow());
}

#[test]
fn deny_decision_exposes_safe_reason_without_leaking_internal_detail() {
    let decision = PolicyDecision::deny(
        "policy-decision.1",
        "policy-request.1",
        "provider.policy.fake",
        "host.path.denied",
    )
    .with_safe_reason("Path is outside the allowed workspace");

    assert_eq!(decision.decision, PolicyDecisionValue::Deny);
    assert_eq!(decision.reason_code, "host.path.denied");
    assert_eq!(
        decision.safe_reason.as_deref(),
        Some("Path is outside the allowed workspace")
    );
}

#[test]
fn policy_decision_maps_to_kernel_event_with_audit_and_context() {
    let decision = PolicyDecision::needs_approval(
        "policy-decision.1",
        "policy-request.1",
        "provider.policy.fake",
        "destructive_action",
    )
    .with_safe_reason("Terminal command needs approval")
    .require_audit();
    let request = PolicyRequest::new("policy-request.1", "code.terminal.run", "cargo test")
        .with_session("session.1")
        .with_task("task.1")
        .with_redaction(KernelEventRedaction::Internal);

    let event = decision.to_event("event.policy.1", &request);

    assert_eq!(event.event_type, "agent.policy.needs_approval");
    assert_eq!(event.source, KernelEventSource::Policy);
    assert_eq!(event.severity, KernelEventSeverity::Warn);
    assert_eq!(event.session_id.as_deref(), Some("session.1"));
    assert_eq!(event.task_id.as_deref(), Some("task.1"));
    assert_eq!(
        event.redaction_classification,
        KernelEventRedaction::Internal
    );
    assert_eq!(
        event.payload_schema.as_deref(),
        Some("sdkwork.agent.policy.decision.v1")
    );
    assert!(event.payload.contains("audit_required=true"));
}

#[test]
fn defer_decision_is_not_allow_or_approval() {
    let decision = PolicyDecision::defer(
        "policy-decision.defer",
        "policy-request.1",
        "provider.policy.fake",
        "policy_provider_unavailable",
    )
    .with_safe_reason("Policy provider is unavailable");

    assert_eq!(decision.decision, PolicyDecisionValue::Defer);
    assert!(!decision.is_allow());
    assert!(!decision.is_needs_approval());
}

#[test]
fn policy_decision_event_for_deny_uses_warn_and_deny_event_type() {
    let request = PolicyRequest::new(
        "policy-request.1",
        "host.filesystem.write",
        "workspace/out.txt",
    );
    let event = PolicyDecision::deny(
        "policy-decision.1",
        "policy-request.1",
        "provider.policy.fake",
        "write_denied",
    )
    .to_event("event.policy.1", &request);

    assert_eq!(event.event_type, "agent.policy.denied");
    assert_eq!(event.severity, KernelEventSeverity::Warn);
}

#[test]
fn policy_decision_event_for_allow_uses_info_and_allow_event_type() {
    let request = PolicyRequest::new("policy-request.1", "model.invoke", "model.chat");
    let event = PolicyDecision::allow(
        "policy-decision.1",
        "policy-request.1",
        "provider.policy.fake",
    )
    .to_event("event.policy.1", &request);

    assert_eq!(event.event_type, "agent.policy.allowed");
    assert_eq!(event.severity, KernelEventSeverity::Info);
}

#[test]
fn policy_decision_event_payload_is_stable_key_value_data() {
    let request = PolicyRequest::new("policy-request.1", "tool.invoke", "tool.echo");
    let event = PolicyDecision::deny(
        "policy-decision.1",
        "policy-request.1",
        "provider.policy.fake",
        "tool_disabled",
    )
    .with_safe_reason("Tool is disabled")
    .require_audit()
    .to_event("event.policy.1", &request);

    assert!(event.payload.contains("decision_id=policy-decision.1"));
    assert!(event.payload.contains("request_id=policy-request.1"));
    assert!(event.payload.contains("decision=deny"));
    assert!(event.payload.contains("reason_code=tool_disabled"));
    assert!(event.payload.contains("safe_reason=Tool is disabled"));
    assert!(event.payload.contains("audit_required=true"));
}

#[test]
fn keep_kernel_event_import_reachable_for_policy_event_contracts() {
    let event: KernelEvent = PolicyDecision::allow(
        "policy-decision.1",
        "policy-request.1",
        "provider.policy.fake",
    )
    .to_event(
        "event.policy.1",
        &PolicyRequest::new("policy-request.1", "model.invoke", "model.chat"),
    );

    assert_eq!(event.event_id, "event.policy.1");
}

#[test]
fn policy_provider_explain_extracts_reason_from_decision() {
    let provider = FakePolicyProvider;
    let decision = PolicyDecision::deny(
        "decision.1",
        "request.1",
        "provider.policy.fake",
        "tool_disabled",
    )
    .with_safe_reason("Tool is disabled for this tenant");

    let explanation = provider.explain(&decision).expect("explain succeeds");

    assert_eq!(explanation.decision_id, "decision.1");
    assert_eq!(
        explanation.human_readable,
        "Tool is disabled for this tenant"
    );
}

#[test]
fn policy_provider_record_decision_is_no_op_by_default() {
    let provider = FakePolicyProvider;
    let decision = PolicyDecision::allow("decision.1", "request.1", "provider.policy.fake");

    provider
        .record_decision(&decision)
        .expect("record_decision succeeds");
}

struct FakePolicyProvider;

impl PolicyProvider for FakePolicyProvider {
    fn evaluate(
        &self,
        request: PolicyRequest,
    ) -> sdkwork_agent_kernel::KernelResult<PolicyDecision> {
        Ok(PolicyDecision::allow(
            "decision.fake",
            &request.policy_request_id,
            "provider.policy.fake",
        ))
    }

    fn health(&self) -> ProviderHealth {
        ProviderHealth::available()
    }
}
