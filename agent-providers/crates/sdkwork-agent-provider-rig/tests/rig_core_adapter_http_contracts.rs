//! Wire-level contracts for the feature-gated `rig-core` chat-completions
//! adapter: the request it puts on the wire, and how it maps provider replies
//! and failures back onto kernel types.
//!
//! Every test drives the real `rig-core` HTTP stack against a loopback
//! listener, so the assertions cover the adapter's actual request and response
//! path rather than a stand-in for it. The listener speaks plain HTTP/1.1 and
//! is answered before the call returns, so no external network is required.

#![cfg(feature = "rig-core-adapter")]

use sdkwork_agent_kernel::{
    FilesystemRequest, FilesystemResult, HostProvider, KernelError, KernelErrorKind, KernelResult,
    ModelRequest, ModelStatus, NetworkRequest, NetworkResult, ProcessRequest, ProcessResult,
    ProviderHealth, ProviderManifest, ProviderSecretValue, SecretRef,
};
use sdkwork_agent_provider_rig::{ids, RigBackendExecutor, RigCoreOpenAiExecutor};
use std::{
    io::{ErrorKind as IoErrorKind, Read, Write},
    net::{Shutdown, TcpListener, TcpStream},
    sync::{mpsc, Arc},
    thread,
    time::{Duration, Instant},
};

/// A body shaped like an OpenAI-compatible chat-completions reply.
const CHAT_COMPLETION_OK: &str = r#"{"id":"chatcmpl-contract","object":"chat.completion","created":1,"model":"deepseek-chat","choices":[{"index":0,"message":{"role":"assistant","content":"Bonjour SDKWork"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":4,"total_tokens":7}}"#;

/// The same reply with no assistant text at all.
const CHAT_COMPLETION_EMPTY: &str = r#"{"id":"chatcmpl-empty","object":"chat.completion","created":1,"model":"deepseek-chat","choices":[{"index":0,"message":{"role":"assistant","content":""},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":0,"total_tokens":3}}"#;

/// A provider error envelope, as a vendor returns it with a 5xx status.
const PROVIDER_ERROR_BODY: &str =
    r#"{"error":{"message":"upstream overloaded","type":"server_error","code":"server_error"}}"#;

/// A host that resolves every secret reference to one fixed credential, so a
/// test can assert on the `Authorization` header the adapter sends.
struct StaticKeyHost {
    api_key: &'static str,
}

impl HostProvider for StaticKeyHost {
    fn provider_manifest(&self) -> ProviderManifest {
        ProviderManifest::new(
            "provider.host.rig-test",
            "host",
            "sdkwork-rig-test-host",
            "0.1.0",
            vec!["host.secrets".to_string()],
        )
    }

    fn health(&self) -> ProviderHealth {
        ProviderHealth::available()
    }

    fn filesystem(&self, _request: FilesystemRequest) -> KernelResult<FilesystemResult> {
        Err(KernelError::CapabilityMissing {
            capability_id: "host.filesystem".to_string(),
        })
    }

    fn process(&self, request: ProcessRequest) -> KernelResult<ProcessResult> {
        Ok(ProcessResult::exited(request.operation_id, 0, "", ""))
    }

    fn network(&self, _request: NetworkRequest) -> KernelResult<NetworkResult> {
        Err(KernelError::CapabilityMissing {
            capability_id: "host.network".to_string(),
        })
    }

    fn resolve_secret(&self, secret_ref: SecretRef) -> KernelResult<ProviderSecretValue> {
        Ok(ProviderSecretValue::new(
            secret_ref.secret_ref_id,
            self.api_key,
        ))
    }
}

/// What the adapter actually sent over the wire.
struct RecordedRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl RecordedRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

fn executor(base_url: &str, default_model_id: &str) -> RigCoreOpenAiExecutor {
    RigCoreOpenAiExecutor::new(
        Arc::new(StaticKeyHost {
            api_key: "test-key",
        }),
        "secret://rig/test",
        default_model_id,
        Some(base_url.to_string()),
    )
    .expect("rig-core executor builds")
}

/// A loopback OpenAI-compatible endpoint that answers `responses` in order and
/// hands every request it read to the test.
fn spawn_endpoint(
    responses: Vec<String>,
) -> (
    String,
    mpsc::Receiver<RecordedRequest>,
    thread::JoinHandle<()>,
) {
    let (base_url, listener, sender, receiver) = bind_endpoint();
    let handle = thread::spawn(move || {
        for response in responses {
            let (mut stream, _) = listener.accept().expect("accept adapter connection");
            let request = read_request(&mut stream).expect("read adapter request");
            let _ = sender.send(request);
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
            let _ = stream.shutdown(Shutdown::Both);
        }
    });
    (base_url, receiver, handle)
}

/// A loopback endpoint that records the request and then answers nothing, so a
/// test can cancel the call while it is airborne.
fn spawn_silent_endpoint() -> (
    String,
    mpsc::Receiver<RecordedRequest>,
    thread::JoinHandle<()>,
) {
    let (base_url, listener, sender, receiver) = bind_endpoint();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept adapter connection");
        let request = read_request(&mut stream).expect("read adapter request");
        let _ = sender.send(request);
        // Long enough for the test to abort the call, short enough that a
        // missing cancel still fails the call instead of hanging the suite.
        thread::sleep(Duration::from_millis(750));
        let _ = stream.shutdown(Shutdown::Both);
    });
    (base_url, receiver, handle)
}

fn bind_endpoint() -> (
    String,
    TcpListener,
    mpsc::Sender<RecordedRequest>,
    mpsc::Receiver<RecordedRequest>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback listener");
    let port = listener
        .local_addr()
        .expect("loopback listener address")
        .port();
    let (sender, receiver) = mpsc::channel();
    (
        format!("http://127.0.0.1:{port}/v1"),
        listener,
        sender,
        receiver,
    )
}

/// Reads one complete HTTP/1.1 request off `stream`.
fn read_request(stream: &mut TcpStream) -> std::io::Result<RecordedRequest> {
    let mut received = Vec::new();
    let mut chunk = [0u8; 2048];
    let header_end = loop {
        if let Some(position) = received.windows(4).position(|window| window == b"\r\n\r\n") {
            break position;
        }
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Err(std::io::Error::new(
                IoErrorKind::UnexpectedEof,
                "connection closed before the request headers arrived",
            ));
        }
        received.extend_from_slice(&chunk[..read]);
    };

    let head = String::from_utf8_lossy(&received[..header_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next().unwrap_or_default().split(' ');
    let method = request_line.next().unwrap_or_default().to_string();
    let path = request_line.next().unwrap_or_default().to_string();
    let mut headers = Vec::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }

    let content_length = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = received[header_end + 4..].to_vec();
    while body.len() < content_length {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }

    Ok(RecordedRequest {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

fn http_response(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// The decoded string value of the first `"key":"value"` pair in `body`,
/// tolerating whitespace around the colon so the assertion does not depend on
/// the encoder's spacing.
fn json_string_field(body: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let mut rest = body;
    loop {
        let at = rest.find(&needle)?;
        rest = &rest[at + needle.len()..];
        let Some(value) = rest.trim_start().strip_prefix(':') else {
            continue;
        };
        let Some(value) = value.trim_start().strip_prefix('"') else {
            continue;
        };
        let mut decoded = String::new();
        let mut chars = value.chars();
        while let Some(ch) = chars.next() {
            match ch {
                '\\' => {
                    if let Some(escaped) = chars.next() {
                        decoded.push(escaped);
                    }
                }
                '"' => return Some(decoded),
                other => decoded.push(other),
            }
        }
        return None;
    }
}

#[test]
fn adapter_posts_chat_completions_and_returns_assistant_text() {
    let (base_url, requests, server) =
        spawn_endpoint(vec![http_response("200 OK", CHAT_COMPLETION_OK)]);
    let executor = executor(&base_url, "deepseek-chat");

    let response = executor
        .invoke_model(ModelRequest::new(
            "model.request.http.1",
            vec!["hello rig".to_string()],
        ))
        .expect("chat completion succeeds");

    let request = requests
        .recv_timeout(Duration::from_secs(10))
        .expect("adapter sent a request");
    server.join().expect("endpoint thread");

    assert_eq!(request.method, "POST");
    assert_eq!(
        request.path, "/v1/chat/completions",
        "the adapter must stay on the chat-completions route that OpenAI-compatible vendors serve"
    );
    assert_eq!(
        request.header("authorization"),
        Some("Bearer test-key"),
        "the resolved secret is sent as the bearer credential"
    );
    assert_eq!(
        json_string_field(&request.body, "model").as_deref(),
        Some("deepseek-chat")
    );
    assert_eq!(
        json_string_field(&request.body, "content").as_deref(),
        Some("hello rig"),
        "the prompt reaches the wire as message content"
    );
    assert_ne!(
        json_string_field(&request.body, "stream").as_deref(),
        Some("true"),
        "the adapter calls the unary endpoint, not a streamed one"
    );

    assert_eq!(response.model_request_id, "model.request.http.1");
    assert_eq!(response.provider_id, ids::MODEL_PROVIDER_ID);
    assert_eq!(response.status, ModelStatus::Succeeded);
    assert_eq!(response.messages, vec!["Bonjour SDKWork".to_string()]);
    assert_eq!(response.finish_reason.as_deref(), Some("stop"));
}

#[test]
fn adapter_resolves_the_catalog_placeholder_and_honours_an_explicit_model_id() {
    let (base_url, requests, server) = spawn_endpoint(vec![
        http_response("200 OK", CHAT_COMPLETION_OK),
        http_response("200 OK", CHAT_COMPLETION_OK),
    ]);
    let executor = executor(&base_url, "deepseek-chat");

    executor
        .invoke_model(
            ModelRequest::new("model.request.http.2", vec!["one".to_string()])
                .with_model_id(ids::DEFAULT_MODEL_ID),
        )
        .expect("catalog placeholder call succeeds");
    executor
        .invoke_model(
            ModelRequest::new("model.request.http.3", vec!["two".to_string()])
                .with_model_id("qwen-plus"),
        )
        .expect("explicit model call succeeds");

    let placeholder = requests
        .recv_timeout(Duration::from_secs(10))
        .expect("placeholder request recorded");
    let explicit = requests
        .recv_timeout(Duration::from_secs(10))
        .expect("explicit request recorded");
    server.join().expect("endpoint thread");

    assert_eq!(
        json_string_field(&placeholder.body, "model").as_deref(),
        Some("deepseek-chat"),
        "the catalog placeholder must never reach the vendor as a model id"
    );
    assert_eq!(
        json_string_field(&explicit.body, "model").as_deref(),
        Some("qwen-plus"),
        "an explicit model id is forwarded unchanged"
    );
}

#[test]
fn adapter_maps_a_provider_error_reply_to_a_retryable_kernel_error() {
    let (base_url, requests, server) = spawn_endpoint(vec![http_response(
        "500 Internal Server Error",
        PROVIDER_ERROR_BODY,
    )]);
    let executor = executor(&base_url, "deepseek-chat");

    let error = executor
        .invoke_model(ModelRequest::new(
            "model.request.http.4",
            vec!["hello rig".to_string()],
        ))
        .expect_err("a 5xx provider reply fails the call");

    let request = requests
        .recv_timeout(Duration::from_secs(10))
        .expect("adapter sent a request");
    server.join().expect("endpoint thread");
    assert_eq!(request.method, "POST");

    assert_eq!(error.kind(), KernelErrorKind::ProviderError);
    assert_eq!(error.code(), "rig_completion_failed");
    assert_eq!(error.provider_id(), Some(ids::MODEL_PROVIDER_ID));
    assert_eq!(error.safe_message(), "Rig model request failed");
    assert!(
        error.retryable(),
        "a 5xx provider reply stays retryable through the kernel error"
    );
    assert!(
        !error.message().is_empty(),
        "the kernel error keeps a diagnostic message"
    );

    let KernelError::Structured { info } = &error else {
        panic!("completion_failure must return a structured kernel error");
    };
    assert!(
        info.details
            .iter()
            .any(|(key, value)| key == "rig.response_status" && value == "500"),
        "the provider HTTP status is preserved as a kernel detail"
    );
}

#[test]
fn adapter_rejects_a_reply_without_assistant_text() {
    let (base_url, requests, server) =
        spawn_endpoint(vec![http_response("200 OK", CHAT_COMPLETION_EMPTY)]);
    let executor = executor(&base_url, "deepseek-chat");

    let error = executor
        .invoke_model(ModelRequest::new(
            "model.request.http.5",
            vec!["hello rig".to_string()],
        ))
        .expect_err("a reply without assistant text fails the call");

    let request = requests
        .recv_timeout(Duration::from_secs(10))
        .expect("adapter sent a request");
    server.join().expect("endpoint thread");
    assert_eq!(request.method, "POST");

    assert_eq!(error.kind(), KernelErrorKind::ProviderError);
    assert_eq!(
        error.code(),
        "rig_empty_response",
        "a reply that carries no assistant text is reported as an empty response"
    );
    assert_eq!(error.provider_id(), Some(ids::MODEL_PROVIDER_ID));
}

#[test]
fn adapter_times_out_a_call_that_never_answers() {
    let (base_url, requests, server) = spawn_silent_endpoint();
    let executor = executor(&base_url, "deepseek-chat");

    let mut request = ModelRequest::new("model.request.http.timeout", vec!["slow".to_string()]);
    request.timeout_ms = Some(150);
    let started = Instant::now();
    let error = executor
        .invoke_model(request)
        .expect_err("an endpoint that never answers times out");

    requests
        .recv_timeout(Duration::from_secs(10))
        .expect("adapter put the call on the wire");
    server.join().expect("endpoint thread");

    assert_eq!(error.kind(), KernelErrorKind::Timeout);
    assert_eq!(error.provider_id(), Some(ids::MODEL_PROVIDER_ID));
    assert_eq!(error.safe_message(), "Rig model request timed out");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the call abandons the wire instead of waiting for the endpoint"
    );
}

#[test]
fn adapter_cancel_aborts_the_inflight_call_and_reports_cancelled() {
    let (base_url, requests, server) = spawn_silent_endpoint();
    let executor = Arc::new(executor(&base_url, "deepseek-chat"));

    let (sender, receiver) = mpsc::channel();
    let worker = {
        let executor = Arc::clone(&executor);
        thread::spawn(move || {
            let mut request =
                ModelRequest::new("model.request.http.cancel", vec!["hang".to_string()]);
            request.timeout_ms = Some(10_000);
            let _ = sender.send(executor.invoke_model(request));
        })
    };

    requests
        .recv_timeout(Duration::from_secs(10))
        .expect("adapter put the call on the wire");

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut cancelled = false;
    while Instant::now() < deadline {
        if executor
            .cancel_model("model.request.http.cancel")
            .expect("cancel reports a kernel result")
        {
            cancelled = true;
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(cancelled, "cancel must find the in-flight call");

    let response = receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("the call settles after cancel")
        .expect("a cancelled call is not an error");
    worker.join().expect("invoking thread");
    server.join().expect("endpoint thread");

    assert_eq!(response.model_request_id, "model.request.http.cancel");
    assert_eq!(response.status, ModelStatus::Cancelled);
    assert_eq!(response.finish_reason.as_deref(), Some("cancelled"));
    assert!(response.messages.is_empty());
}
