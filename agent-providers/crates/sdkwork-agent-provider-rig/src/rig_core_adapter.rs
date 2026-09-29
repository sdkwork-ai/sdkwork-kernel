use std::{
    sync::{mpsc, Arc},
    time::Duration,
};

use rig_core::client::CompletionClient;
use rig_core::completion::{AssistantContent, CompletionModel};
use rig_core::providers::openai;
use sdkwork_agent_kernel::{
    HostProvider, KernelError, KernelErrorSource, KernelResult, KnowledgeDocument,
    KnowledgeSearchRequest, ModelRequest, ModelResponse, SecretRef,
};

use crate::{backend::RigBackendExecutor, ids, provider::RigKnowledgeProvider};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RigVectorSearchPlan {
    pub query: String,
    pub samples: u64,
}

#[derive(Debug, Clone, Default)]
pub struct RigCoreKnowledgeAdapter;

impl RigCoreKnowledgeAdapter {
    pub fn vector_search_plan(request: &KnowledgeSearchRequest) -> RigVectorSearchPlan {
        RigVectorSearchPlan {
            query: request.query.clone(),
            samples: request.top_k as u64,
        }
    }

    pub fn provider_from_documents(
        documents: impl IntoIterator<Item = KnowledgeDocument>,
    ) -> RigKnowledgeProvider {
        documents
            .into_iter()
            .fold(RigKnowledgeProvider::new(), |provider, document| {
                provider.with_document(document)
            })
    }
}

pub struct RigCoreOpenAiExecutor {
    host: Arc<dyn HostProvider + Send + Sync>,
    api_key_secret_ref: String,
    default_model_id: String,
    /// Custom OpenAI-compatible endpoint (`llm.rig.base_url`); `None` targets
    /// the vendor's default endpoint.
    base_url: Option<String>,
    runtime: Arc<tokio::runtime::Runtime>,
}

impl RigCoreOpenAiExecutor {
    pub fn new(
        host: Arc<dyn HostProvider + Send + Sync>,
        api_key_secret_ref: impl Into<String>,
        default_model_id: impl Into<String>,
        base_url: Option<String>,
    ) -> KernelResult<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|_| provider_unavailable())?;
        Ok(Self {
            host,
            api_key_secret_ref: api_key_secret_ref.into(),
            default_model_id: default_model_id.into(),
            base_url,
            runtime: Arc::new(runtime),
        })
    }
}

impl RigBackendExecutor for RigCoreOpenAiExecutor {
    fn invoke_model(&self, request: ModelRequest) -> KernelResult<ModelResponse> {
        let secret_ref = SecretRef::new(&self.api_key_secret_ref, "Rig OpenAI API key");
        let secret = self.host.resolve_secret(secret_ref)?;
        // `CompletionsClient` is the chat-completions client. Since rig-core
        // 0.42 the default `openai::Client` targets the `/v1/responses`
        // endpoint instead, which OpenAI-compatible vendors behind a custom
        // `llm.rig.base_url` (deepseek, qwen, …) do not serve.
        let mut builder = openai::CompletionsClient::builder().api_key(secret.expose_value());
        if let Some(base_url) = self.base_url.as_deref().filter(|value| !value.trim().is_empty())
        {
            builder = builder.base_url(base_url);
        }
        let client = builder.build().map_err(client_build_failure)?;
        // The catalog placeholder (`rig.default-chat`) is the session
        // binding's default model label, not a real upstream model: fall back
        // to the configured default model so a custom provider call never
        // sends the placeholder id upstream.
        let model_id = request
            .model_id
            .clone()
            .filter(|model_id| model_id != ids::DEFAULT_MODEL_ID)
            .unwrap_or_else(|| self.default_model_id.clone());
        let model = client.completion_model(&model_id);
        let prompt = request.effective_prompt_text();
        let timeout = Duration::from_millis(request.timeout_ms.unwrap_or(120_000));
        let (sender, receiver) = mpsc::sync_channel(1);
        let task = self.runtime.spawn(async move {
            let result = model
                .completion_request(prompt)
                .send()
                .await
                .map_err(completion_failure)
                .and_then(|response| {
                    let text = response
                        .choice
                        .iter()
                        .filter_map(|content| match content {
                            AssistantContent::Text(text) => Some(text.text.as_str()),
                            _ => None,
                        })
                        .collect::<String>();
                    (!text.is_empty())
                        .then_some(text)
                        .ok_or_else(empty_response)
                });
            let _ = sender.send(result);
        });
        let text = match receiver.recv_timeout(timeout) {
            Ok(Ok(text)) => text,
            Ok(Err(error)) => return Err(error),
            Err(_) => {
                task.abort();
                return Err(elapsed_timeout(timeout));
            }
        };

        Ok(
            ModelResponse::text(request.model_request_id, ids::MODEL_PROVIDER_ID, text)
                .with_finish_reason("stop"),
        )
    }
}

fn provider_unavailable() -> KernelError {
    KernelError::ProviderUnavailable {
        provider_id: ids::MODEL_PROVIDER_ID.to_string(),
    }
}

/// Maps a rig-core 0.42 [`CompletionError`] into a kernel provider error,
/// keeping the typed diagnostics the upgraded crate now carries (transport
/// request id, HTTP status) without copying the raw upstream body into the
/// kernel error surface. Transport-level failures stay retryable.
fn completion_failure(error: rig_core::completion::CompletionError) -> KernelError {
    let retryable = matches!(error, rig_core::completion::CompletionError::HttpError(_));
    let mut failure = KernelError::provider_error(
        "rig_completion_failed",
        format!("Rig chat-completions request failed: {error}"),
    )
    .with_provider(ids::MODEL_PROVIDER_ID)
    .with_safe_message("Rig model request failed")
    .with_retryable(retryable)
    .from_source(KernelErrorSource::Provider);
    if let Some(request_id) = error.provider_request_id() {
        failure = failure.with_detail("rig.provider_request_id", request_id.to_string());
    }
    if let Some(status) = error.provider_response_status() {
        failure = failure.with_detail("rig.response_status", status.as_u16().to_string());
    }
    failure
}

fn client_build_failure(error: rig_core::http_client::Error) -> KernelError {
    KernelError::provider_error(
        "rig_client_build_failed",
        format!("failed to build the Rig chat-completions client: {error}"),
    )
    .with_provider(ids::MODEL_PROVIDER_ID)
    .with_safe_message("Rig model backend is unavailable")
    .from_source(KernelErrorSource::Provider)
}

fn empty_response() -> KernelError {
    KernelError::provider_error(
        "rig_empty_response",
        "Rig chat-completions response contained no assistant text",
    )
    .with_provider(ids::MODEL_PROVIDER_ID)
    .with_safe_message("Rig model returned an empty response")
    .from_source(KernelErrorSource::Provider)
}

fn elapsed_timeout(timeout: Duration) -> KernelError {
    KernelError::timeout(format!(
        "Rig chat-completions request timed out after {timeout:?}"
    ))
    .with_provider(ids::MODEL_PROVIDER_ID)
    .with_safe_message("Rig model request timed out")
    .from_source(KernelErrorSource::Provider)
}

#[cfg(all(test, feature = "rig-core-adapter"))]
mod tests {
    use super::*;
    use sdkwork_agent_kernel::KernelErrorKind;

    #[test]
    fn completion_failure_preserves_rig_diagnostics_without_leaking_rig_types() {
        let failure = completion_failure(rig_core::completion::CompletionError::ProviderError(
            "upstream rejected the request".to_string(),
        ));

        assert_eq!(failure.kind(), KernelErrorKind::ProviderError);
        assert_eq!(failure.code(), "rig_completion_failed");
        assert!(failure.message().contains("upstream rejected the request"));
        assert_eq!(failure.provider_id(), Some(ids::MODEL_PROVIDER_ID));
        assert_eq!(failure.safe_message(), "Rig model request failed");
        assert!(!failure.retryable());
        let KernelError::Structured { info } = &failure else {
            panic!("completion_failure must return a structured kernel error");
        };
        assert!(!info
            .details
            .iter()
            .any(|(key, _)| key == "rig.provider_request_id"));
    }

    #[test]
    fn transport_failures_stay_retryable() {
        let failure = completion_failure(rig_core::completion::CompletionError::HttpError(
            rig_core::http_client::Error::NoHeaders,
        ));

        assert!(failure.retryable());
    }

    #[test]
    fn elapsed_timeout_maps_to_kernel_timeout_kind() {
        let failure = elapsed_timeout(Duration::from_millis(5_000));

        assert_eq!(failure.kind(), KernelErrorKind::Timeout);
        assert_eq!(failure.provider_id(), Some(ids::MODEL_PROVIDER_ID));
        assert_eq!(failure.safe_message(), "Rig model request timed out");
    }
}
