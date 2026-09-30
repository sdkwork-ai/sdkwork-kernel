use crate::protocol::{
    is_invoke_terminal_frame, is_session_activity_frame, is_stream_chunk_frame,
    is_stream_kernel_event_frame, is_stream_terminal_frame, stream_chunk_frame, stream_done_frame,
    stream_done_frame_with_completion, JsonRpcRequest, JsonRpcResponse,
    SDKWORK_CAPABILITY_INVOKE_METHOD, SDKWORK_PING_METHOD,
};
use sdkwork_agent_kernel::mock_provider_invocation_allowed_from_env;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStderr, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

/// Maximum encoded JSON-RPC request or response frame accepted from a worker.
///
/// This is deliberately lower than the HTTP body limit.  A worker must not be
/// able to turn one malformed line into an unbounded `String` allocation.
pub const MAX_IPC_FRAME_BYTES: usize = 8 * 1024 * 1024;
/// Maximum number of stream chunks in one operation.
pub const MAX_STREAM_BUFFER_CHUNKS: usize = 4096;
/// Maximum UTF-8 byte length of one stream chunk.
pub const MAX_STREAM_CHUNK_BYTES: usize = 256 * 1024;
/// Maximum aggregate UTF-8 bytes emitted by one stream operation.
pub const MAX_STREAM_TOTAL_BYTES: usize = 16 * 1024 * 1024;
/// Default number of concurrently leased provider workers per runtime.
pub const DEFAULT_PROVIDER_WORKER_CONCURRENCY: usize = 8;
/// Hard upper bound for the configured worker concurrency.
pub const MAX_PROVIDER_WORKER_CONCURRENCY: usize = 64;

/// Returns the bounded worker concurrency configured for provider processes.
pub fn provider_worker_concurrency_limit() -> usize {
    std::env::var("SDKWORK_PROVIDER_WORKER_MAX_CONCURRENCY")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .map(|value| value.clamp(1, MAX_PROVIDER_WORKER_CONCURRENCY))
        .unwrap_or(DEFAULT_PROVIDER_WORKER_CONCURRENCY)
}

/// Tracks stream count and byte budgets without retaining the stream payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamResourceBudget {
    chunks: usize,
    total_bytes: usize,
}

impl StreamResourceBudget {
    pub const fn new() -> Self {
        Self {
            chunks: 0,
            total_bytes: 0,
        }
    }

    pub fn record_chunk(&mut self, content: &str) -> Result<(), TransportError> {
        let bytes = content.len();
        if bytes > MAX_STREAM_CHUNK_BYTES {
            return Err(TransportError::new(format!(
                "worker stream chunk exceeded byte limit ({MAX_STREAM_CHUNK_BYTES})"
            )));
        }
        if self.chunks >= MAX_STREAM_BUFFER_CHUNKS {
            return Err(TransportError::new(format!(
                "worker stream exceeded chunk limit ({MAX_STREAM_BUFFER_CHUNKS})"
            )));
        }
        let total = self
            .total_bytes
            .checked_add(bytes)
            .ok_or_else(|| TransportError::new("worker stream byte budget overflow"))?;
        if total > MAX_STREAM_TOTAL_BYTES {
            return Err(TransportError::new(format!(
                "worker stream exceeded total byte limit ({MAX_STREAM_TOTAL_BYTES})"
            )));
        }
        self.chunks += 1;
        self.total_bytes = total;
        Ok(())
    }

    pub const fn chunks(&self) -> usize {
        self.chunks
    }

    pub const fn total_bytes(&self) -> usize {
        self.total_bytes
    }
}

impl Default for StreamResourceBudget {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportError {
    pub message: String,
}

impl TransportError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for TransportError {}

pub trait JsonRpcTransport {
    fn call(&self, method: &str, params: Option<Value>) -> Result<Value, TransportError>;

    /// Delivers one or more stream frames for capability operations that support
    /// incremental worker output. Default implementation expands a single buffered
    /// `call` response into chunk/done frames.
    fn call_streaming(
        &self,
        method: &str,
        params: Option<Value>,
        sink: &mut dyn FnMut(Value) -> Result<bool, TransportError>,
    ) -> Result<(), TransportError> {
        let result = self.call(method, params)?;
        expand_buffered_stream_payload(result, sink)
    }
}

/// Expands a buffered stream payload or passes through explicit stream frames.
pub fn expand_buffered_stream_payload<F>(
    payload: Value,
    mut on_frame: F,
) -> Result<(), TransportError>
where
    F: FnMut(Value) -> Result<bool, TransportError>,
{
    if is_stream_chunk_frame(&payload) {
        let content = payload
            .get("content")
            .or_else(|| payload.get("delta"))
            .and_then(Value::as_str)
            .unwrap_or("");
        StreamResourceBudget::new().record_chunk(content)?;
        on_frame(payload)?;
        return Ok(());
    }
    if is_stream_kernel_event_frame(&payload) {
        on_frame(payload)?;
        return Ok(());
    }
    if is_stream_terminal_frame(&payload) {
        on_frame(payload)?;
        return Ok(());
    }
    if payload.get("ok").and_then(Value::as_bool) == Some(false) {
        return Err(TransportError::new(
            payload
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("stream invoke failed"),
        ));
    }
    if let Some(chunks) = payload.get("chunks").and_then(Value::as_array) {
        if chunks.len() > MAX_STREAM_BUFFER_CHUNKS {
            return Err(TransportError::new(format!(
                "worker stream exceeded chunk limit ({MAX_STREAM_BUFFER_CHUNKS})"
            )));
        }
        let model_request_id = payload.get("model_request_id").and_then(Value::as_str);
        let mut budget = StreamResourceBudget::new();
        for (index, entry) in chunks.iter().enumerate() {
            let content = entry
                .get("content")
                .or_else(|| entry.get("delta"))
                .and_then(Value::as_str)
                .unwrap_or("");
            budget.record_chunk(content)?;
            let sequence = entry
                .get("sequence")
                .and_then(Value::as_u64)
                .unwrap_or(index as u64);
            let frame = stream_chunk_frame(sequence, content, model_request_id);
            if !on_frame(frame)? {
                return Ok(());
            }
        }
        let finish_reason = payload
            .get("finish_reason")
            .and_then(Value::as_str)
            .unwrap_or("stop");
        let provider_session_id = payload.get("provider_session_id").and_then(Value::as_str);
        on_frame(stream_done_frame_with_completion(
            finish_reason,
            model_request_id,
            provider_session_id,
        ))?;
        return Ok(());
    }
    if let Some(messages) = payload.get("messages").and_then(Value::as_array) {
        if messages.len() > MAX_STREAM_BUFFER_CHUNKS {
            return Err(TransportError::new(format!(
                "worker stream exceeded chunk limit ({MAX_STREAM_BUFFER_CHUNKS})"
            )));
        }
        let model_request_id = payload.get("model_request_id").and_then(Value::as_str);
        let mut budget = StreamResourceBudget::new();
        for (index, entry) in messages.iter().enumerate() {
            let Some(content) = entry.as_str() else {
                continue;
            };
            budget.record_chunk(content)?;
            let frame = stream_chunk_frame(index as u64, content, model_request_id);
            if !on_frame(frame)? {
                return Ok(());
            }
        }
        let provider_session_id = payload.get("provider_session_id").and_then(Value::as_str);
        on_frame(stream_done_frame_with_completion(
            "stop",
            model_request_id,
            provider_session_id,
        ))?;
        return Ok(());
    }
    if !on_frame(payload)? {
        return Ok(());
    }
    on_frame(stream_done_frame("stop"))?;
    Ok(())
}

pub struct InMemoryJsonRpcTransport {
    responses: HashMap<String, Value>,
}

impl InMemoryJsonRpcTransport {
    pub fn new() -> Self {
        Self {
            responses: HashMap::new(),
        }
    }

    pub fn stub_method(mut self, method: &str, result: Value) -> Self {
        self.responses.insert(method.to_string(), result);
        self
    }
}

impl Default for InMemoryJsonRpcTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl JsonRpcTransport for InMemoryJsonRpcTransport {
    fn call(&self, method: &str, _params: Option<Value>) -> Result<Value, TransportError> {
        self.responses
            .get(method)
            .cloned()
            .ok_or_else(|| TransportError::new(format!("no stub response for method {method}")))
    }
}

/// In-memory transport that mirrors worker capability dispatch for CI fallback paths.
pub struct PackageStubJsonRpcTransport {
    package_name: String,
    backend_kind: &'static str,
}

impl PackageStubJsonRpcTransport {
    pub fn new(package_name: impl Into<String>, backend_kind: &'static str) -> Self {
        Self {
            package_name: package_name.into(),
            backend_kind,
        }
    }
}

impl JsonRpcTransport for PackageStubJsonRpcTransport {
    fn call(&self, method: &str, params: Option<Value>) -> Result<Value, TransportError> {
        if !mock_provider_invocation_allowed_from_env() {
            if method == SDKWORK_CAPABILITY_INVOKE_METHOD {
                return Ok(fail_closed_provider_payload(
                    "mock provider fallback is disabled for this runtime profile",
                ));
            }
            return Err(TransportError::new(fail_closed_provider_message(
                "mock provider fallback is disabled for this runtime profile",
            )));
        }

        match method {
            SDKWORK_PING_METHOD => Ok(json!({
                "ok": true,
                "backend": self.backend_kind,
                "package": self.package_name,
                "package_resolved": false,
            })),
            SDKWORK_CAPABILITY_INVOKE_METHOD => {
                let operation = params
                    .as_ref()
                    .and_then(|value| value.get("operation"))
                    .cloned()
                    .unwrap_or(Value::Null);
                Ok(stub_capability_invoke_result(
                    &self.package_name,
                    self.backend_kind,
                    &operation,
                ))
            }
            _ => Err(TransportError::new(format!(
                "no stub response for method {method}"
            ))),
        }
    }
}

pub fn stub_capability_invoke_result(
    package_name: &str,
    backend_kind: &str,
    operation: &Value,
) -> Value {
    let op = operation
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or("unknown");

    if op == "ping" {
        return json!({
            "ok": true,
            "backend": backend_kind,
            "package": package_name,
            "package_resolved": false,
        });
    }

    if op == "session_create" {
        return json!({
            "ok": true,
            "mode": "stub",
            "agent_id": operation.get("agent_id"),
            "user_ref": operation.get("user_ref"),
            "package": package_name,
        });
    }

    if op == "model_chat" || op == "model_chat_stream" {
        let messages = operation
            .get("messages")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| entry.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let prompt = messages.join("\n");
        let wire_note = operation
            .get("wire_messages")
            .and_then(Value::as_array)
            .filter(|entries| !entries.is_empty())
            .map(|entries| format!(" wire_messages={}", entries.len()))
            .unwrap_or_default();
        let body = if op == "model_chat_stream" {
            let text = format!("[{package_name} stub] {prompt}{wire_note}");
            let chunks = text
                .split_whitespace()
                .enumerate()
                .map(|(sequence, word)| {
                    json!({
                        "sequence": sequence as u64,
                        "content": format!("{word} "),
                    })
                })
                .collect::<Vec<_>>();
            json!({
                "ok": true,
                "mode": "stub",
                "chunks": chunks,
                "finish_reason": "stop",
                "package": package_name,
                "model_request_id": operation.get("model_request_id"),
            })
        } else {
            json!({
                "ok": true,
                "mode": "stub",
                "messages": [format!("[{package_name} stub] {prompt}{wire_note}")],
                "finish_reason": "stop",
                "package": package_name,
                "model_request_id": operation.get("model_request_id"),
            })
        };
        return body;
    }

    if op == "tool_invoke" {
        return json!({
            "ok": true,
            "mode": "stub",
            "output": json!({
                "tool_id": operation.get("tool_id"),
                "arguments": operation.get("arguments"),
                "package": package_name,
            }).to_string(),
            "package": package_name,
            "tool_call_id": operation.get("tool_call_id"),
        });
    }

    json!({
        "ok": true,
        "mode": "unknown_operation",
        "operation": op,
        "package": package_name,
    })
}

pub struct StdioJsonRpcSession {
    stdin: Mutex<ChildStdin>,
    pending: Arc<Mutex<HashMap<String, PendingResponse>>>,
    next_id: AtomicU64,
    poisoned: Arc<AtomicBool>,
}

/// Drain the worker's stderr into a bounded tail buffer. Past the cap the
/// reader keeps consuming and discards excess, so the pipe never fills and a
/// chatty child cannot deadlock or grow memory unboundedly.
fn drain_worker_stderr(stderr: ChildStderr, tail: Arc<Mutex<Vec<u8>>>) {
    let mut reader = BufReader::new(stderr);
    let mut chunk = [0u8; 4096];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(read) => {
                if let Ok(mut tail) = tail.lock() {
                    let capacity = crate::worker_process::WORKER_STDERR_TAIL_BYTES;
                    if tail.len() + read > capacity {
                        // Keep the most recent bytes within the cap.
                        let overflow = tail.len() + read - capacity;
                        let keep_from = overflow.min(tail.len());
                        tail.drain(0..keep_from);
                    }
                    let append = read.min(capacity);
                    tail.extend_from_slice(&chunk[chunk.len() - append..]);
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
enum PendingResponseMode {
    Unary,
    Streaming,
}

struct PendingResponse {
    mode: PendingResponseMode,
    sender: mpsc::Sender<Result<JsonRpcResponse, TransportError>>,
}

struct PendingCall {
    request_id: String,
    pending: Arc<Mutex<HashMap<String, PendingResponse>>>,
}

impl Drop for PendingCall {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(&self.request_id);
        }
    }
}

impl StdioJsonRpcSession {
    pub fn spawn(
        mut command: Command,
    ) -> Result<(Self, Child, crate::worker_process::StderrTail), TransportError> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Piped (not null): stderr feeds a bounded tail buffer for crash
            // diagnostics while the drain thread keeps the pipe empty, so a
            // chatty worker can neither deadlock nor grow memory unboundedly.
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|error| TransportError::new(format!("failed to spawn worker: {error}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| TransportError::new("worker stdin unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| TransportError::new("worker stdout unavailable"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| TransportError::new("worker stderr unavailable"))?;
        let stderr_tail: crate::worker_process::StderrTail =
            Arc::new(Mutex::new(Vec::with_capacity(4096)));
        let drain_tail = stderr_tail.clone();
        if let Err(error) = thread::Builder::new()
            .name("sdkwork-provider-stderr-drain".to_string())
            .spawn(move || drain_worker_stderr(stderr, drain_tail))
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(TransportError::new(format!(
                "worker stderr drain spawn failed: {error}"
            )));
        }
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let poisoned = Arc::new(AtomicBool::new(false));
        let reader_pending = pending.clone();
        let reader_poisoned = poisoned.clone();
        if let Err(error) = thread::Builder::new()
            .name("sdkwork-provider-jsonrpc-reader".to_string())
            .spawn(move || {
                dispatch_worker_responses(BufReader::new(stdout), reader_pending, reader_poisoned)
            })
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(TransportError::new(format!(
                "worker response reader spawn failed: {error}"
            )));
        }
        Ok((
            Self {
                stdin: Mutex::new(stdin),
                pending,
                next_id: AtomicU64::new(1),
                poisoned,
            },
            child,
            stderr_tail,
        ))
    }

    fn next_id(&self) -> String {
        self.next_id.fetch_add(1, Ordering::Relaxed).to_string()
    }

    pub fn is_reusable(&self) -> bool {
        !self.poisoned.load(Ordering::Acquire)
    }

    fn poison(&self) {
        poison_pending_requests(
            &self.pending,
            &self.poisoned,
            TransportError::new("worker JSON-RPC session is unavailable"),
        );
    }

    fn register_request(
        &self,
        request_id: &str,
        mode: PendingResponseMode,
    ) -> Result<
        (
            mpsc::Receiver<Result<JsonRpcResponse, TransportError>>,
            PendingCall,
        ),
        TransportError,
    > {
        if !self.is_reusable() {
            return Err(TransportError::new(
                "worker JSON-RPC session is unavailable",
            ));
        }
        let (sender, receiver) = mpsc::channel();
        let mut pending = self
            .pending
            .lock()
            .map_err(|error| TransportError::new(format!("pending map lock failed: {error}")))?;
        if self.poisoned.load(Ordering::Acquire) {
            return Err(TransportError::new(
                "worker JSON-RPC session is unavailable",
            ));
        }
        if pending
            .insert(request_id.to_string(), PendingResponse { mode, sender })
            .is_some()
        {
            self.poisoned.store(true, Ordering::Release);
            return Err(TransportError::new(format!(
                "duplicate worker JSON-RPC request id: {request_id}"
            )));
        }
        drop(pending);
        Ok((
            receiver,
            PendingCall {
                request_id: request_id.to_string(),
                pending: self.pending.clone(),
            },
        ))
    }

    fn receive_response(
        &self,
        receiver: &mpsc::Receiver<Result<JsonRpcResponse, TransportError>>,
    ) -> Result<JsonRpcResponse, TransportError> {
        receiver.recv().map_err(|_| {
            self.poison();
            TransportError::new("worker response dispatcher closed")
        })?
    }

    fn write_request(&self, request: &JsonRpcRequest) -> Result<(), TransportError> {
        let encoded = serde_json::to_vec(request)
            .map_err(|error| TransportError::new(format!("encode request failed: {error}")))?;
        if encoded.len() > MAX_IPC_FRAME_BYTES {
            return Err(TransportError::new(format!(
                "worker request exceeded frame byte limit ({MAX_IPC_FRAME_BYTES})"
            )));
        }
        let mut stdin = self
            .stdin
            .lock()
            .map_err(|error| TransportError::new(format!("stdin lock failed: {error}")))?;
        stdin
            .write_all(&encoded)
            .and_then(|_| stdin.write_all(b"\n"))
            .and_then(|_| stdin.flush())
            .map_err(|error| {
                self.poison();
                TransportError::new(format!("worker write failed: {error}"))
            })
    }
}

fn dispatch_worker_responses(
    mut stdout: BufReader<std::process::ChildStdout>,
    pending: Arc<Mutex<HashMap<String, PendingResponse>>>,
    poisoned: Arc<AtomicBool>,
) {
    loop {
        let response = match read_worker_response(&mut stdout) {
            Ok(response) => response,
            Err(error) => {
                poison_pending_requests(&pending, &poisoned, error);
                return;
            }
        };
        let request_id = response.id.clone();
        let route = {
            let mut requests = match pending.lock() {
                Ok(requests) => requests,
                Err(error) => {
                    poisoned.store(true, Ordering::Release);
                    let _ = error;
                    return;
                }
            };
            let Some(request) = requests.get(&request_id) else {
                drop(requests);
                poison_pending_requests(
                    &pending,
                    &poisoned,
                    TransportError::new(format!(
                        "worker returned unknown or duplicate response id: {request_id}"
                    )),
                );
                return;
            };
            let terminal = response_is_terminal(request.mode, &response);
            let sender = request.sender.clone();
            if terminal {
                requests.remove(&request_id);
            }
            sender
        };
        if route.send(Ok(response)).is_err() {
            poison_pending_requests(
                &pending,
                &poisoned,
                TransportError::new(format!(
                    "worker response receiver closed for request id: {request_id}"
                )),
            );
            return;
        }
    }
}

fn response_is_terminal(mode: PendingResponseMode, response: &JsonRpcResponse) -> bool {
    match mode {
        PendingResponseMode::Unary => true,
        PendingResponseMode::Streaming => {
            if response.error.is_some() {
                return true;
            }
            let result = response.result.as_ref().unwrap_or(&Value::Null);
            !(is_session_activity_frame(result)
                || is_stream_kernel_event_frame(result)
                || is_stream_chunk_frame(result))
        }
    }
}

fn read_worker_response(
    stdout: &mut BufReader<std::process::ChildStdout>,
) -> Result<JsonRpcResponse, TransportError> {
    let mut frame = Vec::new();
    let bytes_read = {
        let mut limited = stdout.take((MAX_IPC_FRAME_BYTES + 1) as u64);
        limited
            .read_until(b'\n', &mut frame)
            .map_err(|error| TransportError::new(format!("worker read failed: {error}")))?
    };
    if bytes_read == 0 {
        return Err(TransportError::new("worker closed stdout"));
    }
    if frame.len() > MAX_IPC_FRAME_BYTES {
        return Err(TransportError::new(format!(
            "worker response exceeded frame byte limit ({MAX_IPC_FRAME_BYTES})"
        )));
    }
    if !frame.ends_with(b"\n") {
        return Err(TransportError::new(
            "worker response frame is not newline terminated",
        ));
    }
    let line = std::str::from_utf8(&frame[..frame.len() - 1]).map_err(|error| {
        TransportError::new(format!("worker response is not valid UTF-8: {error}"))
    })?;
    serde_json::from_str(line)
        .map_err(|error| TransportError::new(format!("decode worker response failed: {error}")))
}

fn poison_pending_requests(
    pending: &Arc<Mutex<HashMap<String, PendingResponse>>>,
    poisoned: &Arc<AtomicBool>,
    error: TransportError,
) {
    poisoned.store(true, Ordering::Release);
    let requests = match pending.lock() {
        Ok(mut requests) => requests
            .drain()
            .map(|(_, request)| request)
            .collect::<Vec<_>>(),
        Err(_) => return,
    };
    for request in requests {
        let _ = request.sender.send(Err(error.clone()));
    }
}

impl JsonRpcTransport for StdioJsonRpcSession {
    fn call(&self, method: &str, params: Option<Value>) -> Result<Value, TransportError> {
        let request = JsonRpcRequest::new(self.next_id(), method, params);
        let (receiver, _pending_call) =
            self.register_request(&request.id, PendingResponseMode::Unary)?;
        self.write_request(&request)?;
        let response = self.receive_response(&receiver)?;

        response
            .into_result()
            .map_err(|error| TransportError::new(error.message))
    }

    fn call_streaming(
        &self,
        method: &str,
        params: Option<Value>,
        sink: &mut dyn FnMut(Value) -> Result<bool, TransportError>,
    ) -> Result<(), TransportError> {
        let request_id = self.next_id();
        let request = JsonRpcRequest::new(request_id.clone(), method, params);
        let (receiver, _pending_call) =
            self.register_request(&request_id, PendingResponseMode::Streaming)?;
        self.write_request(&request)?;
        let mut budget = StreamResourceBudget::new();

        loop {
            let response = self.receive_response(&receiver)?;

            let result = response
                .into_result()
                .map_err(|error| TransportError::new(error.message))?;

            if is_session_activity_frame(&result) || is_stream_kernel_event_frame(&result) {
                let keep_streaming = match sink(result) {
                    Ok(keep_streaming) => keep_streaming,
                    Err(error) => {
                        self.poison();
                        return Err(error);
                    }
                };
                if !keep_streaming {
                    self.poison();
                    return Ok(());
                }
                continue;
            }
            if is_stream_chunk_frame(&result) {
                let content = result
                    .get("content")
                    .or_else(|| result.get("delta"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if let Err(error) = budget.record_chunk(content) {
                    self.poison();
                    return Err(error);
                }
                let keep_streaming = match sink(result) {
                    Ok(keep_streaming) => keep_streaming,
                    Err(error) => {
                        self.poison();
                        return Err(error);
                    }
                };
                if !keep_streaming {
                    self.poison();
                    return Ok(());
                }
                continue;
            }
            if is_stream_terminal_frame(&result) {
                sink(result)?;
                return Ok(());
            }
            if is_invoke_terminal_frame(&result) {
                sink(result)?;
                return Ok(());
            }

            expand_buffered_stream_payload(result, &mut *sink)?;
            return Ok(());
        }
    }
}

pub struct FailClosedJsonRpcTransport {
    reason: String,
}

impl FailClosedJsonRpcTransport {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl JsonRpcTransport for FailClosedJsonRpcTransport {
    fn call(&self, method: &str, _params: Option<Value>) -> Result<Value, TransportError> {
        if method == SDKWORK_CAPABILITY_INVOKE_METHOD {
            return Ok(fail_closed_provider_payload(&self.reason));
        }

        Err(TransportError::new(fail_closed_provider_message(
            &self.reason,
        )))
    }
}

fn fail_closed_provider_payload(reason: &str) -> Value {
    json!({
        "ok": false,
        "mode": "sdk_live_failed",
        "error": fail_closed_provider_message(reason),
    })
}

fn fail_closed_provider_message(reason: &str) -> String {
    format!("sdk backend unavailable in production profile: {reason}")
}

pub struct SharedJsonRpcTransport {
    inner: Arc<dyn JsonRpcTransport + Send + Sync>,
}

impl SharedJsonRpcTransport {
    pub fn new(inner: Arc<dyn JsonRpcTransport + Send + Sync>) -> Self {
        Self { inner }
    }
}

impl JsonRpcTransport for SharedJsonRpcTransport {
    fn call(&self, method: &str, params: Option<Value>) -> Result<Value, TransportError> {
        self.inner.call(method, params)
    }

    fn call_streaming(
        &self,
        method: &str,
        params: Option<Value>,
        sink: &mut dyn FnMut(Value) -> Result<bool, TransportError>,
    ) -> Result<(), TransportError> {
        self.inner.call_streaming(method, params, sink)
    }
}
