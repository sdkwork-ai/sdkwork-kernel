// WORKSPACE-PATH:allow-fixture - this file is a test fixture that simulates a foreign
// checkout root, so the sdkwork-<name> segment below is the value under assertion rather
// than a binding to a real sibling checkout. PORTABILITY_SPEC.md section 5.2 governs it.
use sdkwork_agent_kernel::ModelProvider;
use sdkwork_agent_provider_hermes::HermesSdkIntegration;

/// Live smoke test: send one real message through the Hermes provider's
/// official TUI gateway channel (`python -m tui_gateway.entry`) and verify
/// the assistant reply comes back with a correlated provider session id.
///
/// Requires the installed `hermes-agent` package (tui_gateway module) and a
/// configured Hermes model provider (config.yaml model section).
#[test]
#[ignore = "requires the hermes-agent tui_gateway package and a live model provider"]
fn live_sdk_model_chat() {
    let integration = HermesSdkIntegration::bootstrap().expect("bootstrap hermes gateway");
    eprintln!("hermes_live_phase=bootstrap_complete");
    let models = integration.model.list_models();
    assert!(!models.is_empty(), "hermes gateway must publish models");
    let model_id = models[0].model_id.clone();
    eprintln!("hermes_live_phase=models model={model_id}");

    let request = sdkwork_agent_kernel::ModelRequest::new(
        format!("hermes-live-{}", std::process::id()),
        vec!["Reply with exactly one word: OK".to_string()],
    )
    .with_model_id(model_id.clone())
    .with_metadata(
        "sdkwork.agent_engine.working_directory",
        "E:/sdkwork-space/sdkwork-birdcoder",
    );
    let response = integration
        .model
        .invoke(request)
        .expect("live hermes invoke succeeds");
    eprintln!(
        "hermes_live_phase=invoke_complete status={:?} finish={:?} provider={}",
        response.status, response.finish_reason, response.provider_id,
    );
    eprintln!("hermes_live_phase=reply messages={:?}", response.messages);
    assert_eq!(
        response.status,
        sdkwork_agent_kernel::ModelStatus::Succeeded,
        "live hermes invoke must succeed: {response:?}"
    );
    assert!(
        response
            .messages
            .iter()
            .any(|message| !message.trim().is_empty()),
        "live hermes invoke must return an assistant reply"
    );
    let provider_session_id = response
        .diagnostics
        .iter()
        .find_map(|diagnostic| {
            let (key, value) = diagnostic.split_once('=')?;
            (key.trim() == "sdk_runtime_provider_session_id").then(|| value.trim().to_string())
        })
        .or_else(|| {
            response.diagnostics.iter().find_map(|diagnostic| {
                let (key, value) = diagnostic.split_once('=')?;
                (key.trim() == "sdk_runtime_session_id").then(|| value.trim().to_string())
            })
        });
    eprintln!(
        "hermes_live_phase=parity_ok provider_session_id={provider_session_id:?} messages={:?}",
        response.messages
    );
    assert!(
        provider_session_id.is_some(),
        "live hermes invoke must return a provider session id for conversation resumption"
    );

    // Second turn in the same provider session: the stored session key must be
    // accepted for session.resume and the model must reply again.
    let resumed_request = sdkwork_agent_kernel::ModelRequest::new(
        format!("hermes-live-resume-{}", std::process::id()),
        vec!["Reply with exactly one word: DONE".to_string()],
    )
    .with_model_id(model_id.clone())
    .for_provider_session(provider_session_id.expect("provider session id"));
    let resumed_response = integration
        .model
        .invoke(resumed_request)
        .expect("live hermes resume invoke succeeds");
    eprintln!(
        "hermes_live_phase=resume_complete status={:?} messages={:?}",
        resumed_response.status, resumed_response.messages
    );
    assert_eq!(
        resumed_response.status,
        sdkwork_agent_kernel::ModelStatus::Succeeded,
        "live hermes resume must succeed: {resumed_response:?}"
    );
    assert!(
        resumed_response
            .messages
            .iter()
            .any(|message| !message.trim().is_empty()),
        "live hermes resume must return an assistant reply"
    );
    eprintln!("hermes_live_phase=resume_parity_ok");
}
