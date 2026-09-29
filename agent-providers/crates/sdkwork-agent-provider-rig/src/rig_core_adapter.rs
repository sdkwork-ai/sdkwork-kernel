use std::{
    sync::{mpsc, Arc},
    time::Duration,
};

use rig_core::client::CompletionClient;
use rig_core::completion::{AssistantContent, CompletionModel};
use rig_core::providers::openai;
use sdkwork_agent_kernel::{
    HostProvider, KernelError, KernelResult, KnowledgeDocument, KnowledgeSearchRequest,
    ModelRequest, ModelResponse, SecretRef,
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
        let client = builder.build().map_err(|_| provider_unavailable())?;
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
                .map_err(|_| ())
                .and_then(|response| {
                    let text = response
                        .choice
                        .iter()
                        .filter_map(|content| match content {
                            AssistantContent::Text(text) => Some(text.text.as_str()),
                            _ => None,
                        })
                        .collect::<String>();
                    (!text.is_empty()).then_some(text).ok_or(())
                });
            let _ = sender.send(result);
        });
        let text = match receiver.recv_timeout(timeout) {
            Ok(Ok(text)) => text,
            Ok(Err(())) => return Err(provider_unavailable()),
            Err(_) => {
                task.abort();
                return Err(provider_unavailable());
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
