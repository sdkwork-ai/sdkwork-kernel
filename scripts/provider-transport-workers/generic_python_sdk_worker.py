#!/usr/bin/env python3
"""SDKWork agent provider transport worker for Python-backed providers.

For Hermes (`tui_gateway` package) this worker is a full JSON-RPC client of the
Hermes TUI gateway process (`python -m tui_gateway.entry`), the exact channel
used by the Hermes desktop and TUI applications:

    session.create / session.resume -> prompt.submit -> event stream
      message.start / message.delta / message.complete
      reasoning.delta / tool.start / tool.complete / status.update
      approval.request / clarify.request / sudo.request / secret.request
        (blocking interactions resolved through approval.respond / clarify.respond)

The worker never reads Hermes state files directly; it speaks the gateway's
official JSON-RPC protocol (stdio) exactly like the desktop app UI does.
"""
import argparse
import importlib.util
import json
import os
import collections
import queue
import subprocess
import sys
import threading
import time

# TUI gateway event types (desktop app wire protocol, tui_gateway/server.py).
EVENT_MESSAGE_START = "message.start"
EVENT_MESSAGE_DELTA = "message.delta"
EVENT_MESSAGE_COMPLETE = "message.complete"
EVENT_REASONING_DELTA = "reasoning.delta"
EVENT_REASONING_AVAILABLE = "reasoning.available"
EVENT_THINKING_DELTA = "thinking.delta"
EVENT_TOOL_START = "tool.start"
EVENT_TOOL_COMPLETE = "tool.complete"
EVENT_STATUS_UPDATE = "status.update"
EVENT_APPROVAL_REQUEST = "approval.request"
EVENT_CLARIFY_REQUEST = "clarify.request"
EVENT_SUDO_REQUEST = "sudo.request"
EVENT_SECRET_REQUEST = "secret.request"
EVENT_TERMINAL_READ_REQUEST = "terminal.read.request"
EVENT_ERROR = "error"

# Default time (seconds) a blocking provider interaction waits for a kernel
# resolution before the worker rejects it so the turn can finish.
INTERACTION_RESOLUTION_TIMEOUT_S = 300

_STREAM_CHUNK = "stream.chunk"
_STREAM_EVENT = "stream.event"
_STREAM_DONE = "stream.done"
_INVOKE_DONE = "invoke.done"


_STDOUT_LOCK = threading.Lock()


def write_response(response):
    with _STDOUT_LOCK:
        sys.stdout.write(json.dumps(response) + "\n")
        sys.stdout.flush()


def write_stream_frame(request_id, frame):
    """Write one JSON-RPC response whose result is a stream frame.

    Streaming uses the same request id for every frame; the Rust transport
    treats chunk/event frames as non-terminal and stops at stream.done.
    """
    write_response(
        {
            "jsonrpc": "2.0",
            "id": request_id,
            "result": frame,
        }
    )


def probe_module(module_name):
    spec = importlib.util.find_spec(module_name)
    return {"resolved": spec is not None}


class PendingInteraction:
    """One blocking TUI gateway interaction awaiting a kernel resolution."""

    def __init__(self, model_request_id, provider_request_id, session_id,
                 stored_session_id, kind, category, provider_turn_id, turn_id):
        self.model_request_id = model_request_id
        self.provider_request_id = provider_request_id
        self.session_id = session_id
        self.stored_session_id = stored_session_id
        self.provider_turn_id = provider_turn_id
        self.turn_id = turn_id
        self.kind = kind
        self.category = category
        self.resolved = threading.Event()
        self.resolution = None
        self.responded = False


class HermesTuiGatewayClient:
    """JSON-RPC client for the Hermes TUI gateway (official desktop channel).

    One gateway process is shared by all kernel requests. Request/response
    correlation uses JSON-RPC ids; server->client events are routed to the
    session that owns them, matching the desktop app's transport model.
    """

    def __init__(self):
        self._process = None
        self._next_request_id = 1
        self._request_lock = threading.Lock()
        self._responses = {}  # request_id -> queue.Queue
        self._interactions = {}  # (model_request_id, provider_request_id) -> PendingInteraction
        self._interactions_lock = threading.Lock()
        self._session_events = {}  # session_id -> queue.Queue of event params
        self._session_events_lock = threading.Lock()
        self._shutdown = False

    # ------------------------------------------------------------------ base

    def request(self, method, params, timeout_ms):
        """Send one JSON-RPC request and block for its correlated response."""
        process = self._ensure_process()
        with self._request_lock:
            request_id = self._next_request_id
            self._next_request_id += 1
            response_queue = queue.Queue(maxsize=1)
            self._responses[request_id] = response_queue
        request = {
            "jsonrpc": "2.0",
            "id": request_id,
            "method": method,
            "params": params or {},
        }
        try:
            process.stdin.write(json.dumps(request) + "\n")
            process.stdin.flush()
        except (BrokenPipeError, OSError) as error:
            with self._request_lock:
                self._responses.pop(request_id, None)
            self.close()
            raise RuntimeError("Hermes TUI gateway request pipe is unavailable") from error

        try:
            response = response_queue.get(timeout=timeout_ms / 1000)
        except queue.Empty as error:
            with self._request_lock:
                self._responses.pop(request_id, None)
            raise TimeoutError(
                "Hermes TUI gateway request timed out: {0}".format(method)
            ) from error
        finally:
            with self._request_lock:
                self._responses.pop(request_id, None)
        if "error" in response:
            raise RuntimeError(
                "Hermes TUI gateway rejected {0}: {1}".format(
                    method, response["error"].get("message", str(response["error"]))
                )
            )
        return response.get("result")

    def session_create(self, cwd, model, provider):
        params = {"cwd": cwd} if cwd else {}
        if model:
            params["model"] = model
        if provider:
            params["provider"] = provider
        result = self.request("session.create", params, 60000)
        session_id = _result_string(result, "session_id")
        stored_session_id = _result_string(result, "stored_session_id")
        if not session_id:
            raise RuntimeError("Hermes session.create returned no session_id")
        return session_id, stored_session_id

    def session_resume(self, stored_session_id):
        result = self.request(
            "session.resume", {"session_id": stored_session_id, "lazy": False}, 60000
        )
        session_id = _result_string(result, "session_id")
        if not session_id:
            raise RuntimeError("Hermes session.resume returned no session_id")
        messages = result.get("messages") if isinstance(result, dict) else None
        return session_id, messages or []

    def submit_prompt(self, session_id, text, on_event):
        """Submit one prompt and drain events until message.complete/error.

        `on_event` receives (event_type, payload, session_id) and may raise to
        abort the turn (e.g. a kernel sink failure).
        """
        result = self.request(
            "prompt.submit", {"session_id": session_id, "text": text}, 60000
        )
        if not isinstance(result, dict) or result.get("status") != "streaming":
            raise RuntimeError("Hermes prompt.submit did not start streaming")

        event_queue = self._session_event_queue(session_id)
        complete = {"text": None, "status": None, "usage": None, "error": None}
        deadline = time.monotonic() + 3600
        while complete["text"] is None and complete["error"] is None:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError("Hermes TUI gateway turn timed out")
            try:
                params = event_queue.get(timeout=remaining)
            except queue.Empty as error:
                raise TimeoutError("Hermes TUI gateway turn timed out") from error
            event_type = params.get("type")
            payload = params.get("payload") or {}
            if event_type == EVENT_MESSAGE_COMPLETE:
                text = payload.get("text")
                if isinstance(text, str):
                    complete["text"] = text
                    complete["status"] = payload.get("status")
                    complete["usage"] = payload.get("usage")
                else:
                    complete["error"] = "Hermes message.complete carried no text"
                on_event(event_type, payload, session_id)
                break
            if event_type == EVENT_ERROR:
                message = payload.get("message", "Hermes TUI gateway error")
                complete["error"] = message
                on_event(event_type, payload, session_id)
                break
            on_event(event_type, payload, session_id)
        if complete["error"] is not None:
            raise RuntimeError(complete["error"])
        return complete["text"], complete["usage"], complete["status"]

    def list_sessions(self):
        result = self.request("session.list", {}, 60000)
        sessions = result.get("sessions") if isinstance(result, dict) else None
        return sessions if isinstance(sessions, list) else []

    def interrupt(self, session_id):
        self.request("session.interrupt", {"session_id": session_id}, 60000)

    def respond_interaction(self, interaction, resolution):
        """Resolve one blocking gateway interaction with the kernel's decision."""
        method, params = hermes_respond_params(interaction, resolution)
        self.request(method, params, 60000)
        with self._interactions_lock:
            key = (interaction.model_request_id, str(interaction.provider_request_id))
            self._interactions.pop(key, None)

    def close(self):
        self._shutdown = True
        process = self._process
        self._process = None
        if process and process.poll() is None:
            try:
                process.terminate()
            except OSError:
                pass

    # ------------------------------------------------------------ internals

    def _ensure_process(self):
        if self._process and self._process.poll() is None:
            return self._process

        self.close()
        process = subprocess.Popen(
            [sys.executable, "-u", "-m", "tui_gateway.entry"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            # Piped (not DEVNULL): the drain thread keeps a bounded tail for
            # crash diagnostics while preventing a full pipe from deadlocking
            # the gateway process.
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
        )
        self._process = process
        self._shutdown = False
        self._stderr_tail = collections.deque(maxlen=64)
        threading.Thread(target=self._read_frames, args=(process,), daemon=True).start()
        threading.Thread(
            target=self._drain_stderr,
            args=(process,),
            daemon=True,
        ).start()
        return process

    def _drain_stderr(self, process):
        if process.stderr is None:
            return
        # Consuming every line keeps the pipe empty (no deadlock); the deque
        # bound holds retained diagnostics constant.
        for line in process.stderr:
            self._stderr_tail.append(line.rstrip()[:512])

    def stderr_tail(self):
        tail = getattr(self, "_stderr_tail", None)
        if not tail:
            return ""
        return "\n".join(tail)

    def _session_event_queue(self, session_id):
        with self._session_events_lock:
            event_queue = self._session_events.get(session_id)
            if event_queue is None:
                event_queue = queue.Queue()
                self._session_events[session_id] = event_queue
            return event_queue

    def _read_frames(self, process):
        if process.stdout is None:
            return
        for line in process.stdout:
            try:
                frame = json.loads(line)
            except json.JSONDecodeError:
                continue
            if not isinstance(frame, dict):
                continue
            if frame.get("method") == "event":
                self._dispatch_event(frame.get("params") or {})
                continue
            request_id = frame.get("id")
            if request_id is None:
                continue
            with self._request_lock:
                response_queue = self._responses.get(request_id)
            if response_queue is not None:
                response_queue.put(frame)

    def _dispatch_event(self, params):
        event_type = params.get("type")
        session_id = params.get("session_id")
        payload = params.get("payload") or {}
        if not session_id:
            return
        event_queue = self._session_event_queue(session_id)
        event_queue.put(params)


class InteractionCoordinator:
    """Holds the active turn's pending interactions so a kernel resolution
    arriving from another thread can unlock the prompt event loop."""

    def __init__(self):
        self._interactions = {}
        self._lock = threading.Lock()

    def register(self, interaction):
        with self._lock:
            key = (interaction.model_request_id, str(interaction.provider_request_id))
            self._interactions[key] = interaction
        return interaction

    def find(self, model_request_id, provider_request_id):
        with self._lock:
            return self._interactions.get(
                (model_request_id, str(provider_request_id))
            )

    def remove(self, model_request_id, provider_request_id):
        with self._lock:
            return self._interactions.pop(
                (model_request_id, str(provider_request_id)), None
            )


_coordinator = InteractionCoordinator()


def hermes_interaction_kind(event_type):
    return {
        EVENT_APPROVAL_REQUEST: "approval",
        EVENT_CLARIFY_REQUEST: "question_set",
        EVENT_SUDO_REQUEST: "sudo",
        EVENT_SECRET_REQUEST: "secret",
        EVENT_TERMINAL_READ_REQUEST: "terminal_read",
    }.get(event_type, "unknown")


def hermes_respond_params(interaction, resolution):
    """Map a kernel interaction resolution to the TUI gateway respond call."""
    kind = interaction.kind
    if kind == "approval":
        choice = resolution.get("choice") or resolution.get("decision") or "deny"
        if isinstance(choice, bool):
            choice = "allow" if choice else "deny"
        choice = str(choice).lower()
        if choice not in ("allow", "deny"):
            choice = "deny"
        return "approval.respond", {
            "session_id": interaction.session_id,
            "request_id": interaction.provider_request_id,
            "choice": choice,
        }
    if kind == "question_set":
        answer = resolution.get("answer")
        if not isinstance(answer, str):
            raise RuntimeError("clarify resolution requires a string answer")
        return "clarify.respond", {
            "request_id": interaction.provider_request_id,
            "answer": answer,
        }
    # sudo / secret / terminal_read: only explicit values are forwarded;
    # anything else rejects the interaction so the turn can proceed safely.
    if kind == "sudo":
        value = resolution.get("password") or resolution.get("value")
        if not isinstance(value, str):
            raise RuntimeError("sudo resolution requires a password value")
        return "sudo.respond", {
            "request_id": interaction.provider_request_id,
            "password": value,
        }
    if kind == "secret":
        value = resolution.get("value")
        if not isinstance(value, str):
            raise RuntimeError("secret resolution requires a value")
        return "secret.respond", {
            "request_id": interaction.provider_request_id,
            "value": value,
        }
    if kind == "terminal_read":
        value = resolution.get("text")
        if not isinstance(value, str):
            raise RuntimeError("terminal_read resolution requires text")
        return "terminal.read.respond", {
            "request_id": interaction.provider_request_id,
            "text": value,
        }
    raise RuntimeError("unsupported Hermes interaction kind: {0}".format(kind))


def kernel_event_frame(model_request_id, event_type, payload, sequence):
    return {
        "event": _STREAM_EVENT,
        "model_request_id": model_request_id,
        "kernel_event": {
            "event_id": "event.{0}.{1}".format(model_request_id, sequence),
            "event_type": event_type,
            "event_version": "1.0.0",
            "occurred_at": _now_iso(),
            "source": _kernel_event_source(event_type),
            "severity": "info",
            "run_id": model_request_id,
            "correlation_id": model_request_id,
            "redaction_classification": "tenant_sensitive",
            "payload_schema": "sdkwork.agent.provider_stream_event.v1",
            "payload": payload,
            "replay": False,
        },
    }


def _kernel_event_source(event_type):
    if event_type in ("agent.policy.paused", "agent.message.paused", "agent.interaction.resolved"):
        return "policy"
    if event_type.startswith("agent.tool.") or event_type == "agent.tool.streamed":
        return "tool"
    if event_type.startswith("agent.model.") or event_type.startswith("agent.message."):
        return "model"
    return "provider"


def _kernel_event_type(event_type):
    if event_type == EVENT_MESSAGE_START:
        return "agent.turn.started"
    if event_type == EVENT_MESSAGE_COMPLETE:
        return "agent.turn.completed"
    if event_type == EVENT_MESSAGE_DELTA:
        return "agent.message.updated"
    if event_type in (EVENT_REASONING_DELTA, EVENT_REASONING_AVAILABLE, EVENT_THINKING_DELTA):
        return "agent.model.streamed"
    if event_type == EVENT_TOOL_START:
        return "agent.tool.started"
    if event_type == EVENT_TOOL_COMPLETE:
        return "agent.tool.completed"
    if event_type == EVENT_STATUS_UPDATE:
        return "agent.provider.updated"
    if event_type == EVENT_ERROR:
        return "agent.runtime.failed"
    return "agent.provider.updated"


def _now_iso():
    try:
        from datetime import datetime, timezone

        return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")
    except Exception:  # pragma: no cover - datetime is always available
        return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


def _result_string(result, key):
    if isinstance(result, dict):
        value = result.get(key)
        if isinstance(value, str) and value.strip():
            return value.strip()
    return ""


def _result_int(result, key):
    if isinstance(result, dict):
        value = result.get(key)
        if isinstance(value, (int, float)):
            return int(value)
    return 0


# ---------------------------------------------------------------------------
# Session helpers used by both invoke and streaming paths.

def _ensure_hermes_session(client, operation, stored_session_id):
    """Create or resume a Hermes session for one model turn.

    Returns (live_session_id, stored_session_id). The provider session id
    surfaced to the kernel is always the persistent stored session key; the
    live 8-hex session id changes on every resume.
    """
    working_directory = operation.get("working_directory")
    model_id = operation.get("model_id")
    if stored_session_id:
        live_session_id, _messages = client.session_resume(stored_session_id)
        return live_session_id, stored_session_id
    live_session_id, new_stored_id = client.session_create(
        working_directory, model_id, None
    )
    return live_session_id, new_stored_id


def _payload_provider_session_id(payload, stored_session_id):
    session_id = payload.get("session_id") or payload.get("sessionId")
    if isinstance(session_id, str) and session_id.strip():
        return session_id.strip()
    return stored_session_id


def _emit_kernel_event(model_request_id, event_type, payload, sequence, stored_session_id, request_id=None):
    enriched = dict(payload or {})
    enriched.setdefault("providerSessionId", stored_session_id)
    enriched.setdefault("sessionId", stored_session_id)
    frame = kernel_event_frame(model_request_id, event_type, enriched, sequence)
    if request_id is not None:
        write_stream_frame(request_id, frame)
    else:
        write_response(frame)


def _stream_chunk(model_request_id, content, sequence, stored_session_id, request_id=None):
    frame = {
        "event": _STREAM_CHUNK,
        "sequence": sequence,
        "content": content,
        "model_request_id": model_request_id,
        "provider_session_id": stored_session_id,
    }
    if request_id is not None:
        write_stream_frame(request_id, frame)
    else:
        write_response(frame)


def _resolve_pending_interaction(client, model_request_id, provider_request_id, timeout_s):
    """Block until the kernel resolves one interaction (or reject on timeout)."""
    interaction = _coordinator.find(model_request_id, provider_request_id)
    if interaction is None:
        return None
    if not interaction.resolved.wait(timeout_s):
        _coordinator.remove(model_request_id, provider_request_id)
        try:
            # Reject the blocking request so the Hermes turn can continue.
            client.request(
                "approval.respond" if interaction.kind == "approval" else "clarify.respond",
                (
                    {
                        "session_id": interaction.session_id,
                        "request_id": interaction.provider_request_id,
                        "choice": "deny",
                    }
                    if interaction.kind == "approval"
                    else {
                        "request_id": interaction.provider_request_id,
                        "answer": "",
                    }
                ),
                60000,
            )
        except Exception:
            pass
        return None
    return interaction.resolution


def _run_hermes_turn(client, operation, emit_stream, request_id=None):
    """Execute one Hermes turn through the official gateway channel.

    Returns the aggregated assistant text (or emits stream frames when
    `emit_stream` is set) plus the persistent provider session id.
    """
    model_request_id = operation.get("model_request_id")
    stored_session_id = (
        operation.get("provider_session_id") or operation.get("providerSessionId") or ""
    ).strip() or None
    live_session_id, stored_session_id = _ensure_hermes_session(
        client, operation, stored_session_id
    )
    prompt = resolve_model_prompt(operation)
    if not prompt.strip():
        raise RuntimeError("Hermes prompt text is blank")

    chunks = []
    sequence = [0]

    def on_event(event_type, payload, session_id):
        sequence[0] += 1
        if emit_stream:
            if event_type == EVENT_MESSAGE_DELTA:
                text = payload.get("text")
                if isinstance(text, str) and text:
                    _stream_chunk(
                        model_request_id, text, sequence[0], stored_session_id, request_id
                    )
            elif event_type == EVENT_MESSAGE_COMPLETE:
                _emit_kernel_event(
                    model_request_id,
                    "agent.turn.completed",
                    payload,
                    sequence[0],
                    stored_session_id,
                    request_id,
                )
            else:
                _emit_kernel_event(
                    model_request_id,
                    _kernel_event_type(event_type),
                    payload,
                    sequence[0],
                    stored_session_id,
                    request_id,
                )
        else:
            if event_type == EVENT_MESSAGE_DELTA:
                text = payload.get("text")
                if isinstance(text, str):
                    chunks.append(text)
        if event_type in (
            EVENT_APPROVAL_REQUEST,
            EVENT_CLARIFY_REQUEST,
            EVENT_SUDO_REQUEST,
            EVENT_SECRET_REQUEST,
            EVENT_TERMINAL_READ_REQUEST,
        ):
            interaction = PendingInteraction(
                model_request_id=model_request_id,
                provider_request_id=payload.get("request_id"),
                session_id=session_id,
                stored_session_id=stored_session_id,
                kind=hermes_interaction_kind(event_type),
                category="approval" if event_type == EVENT_APPROVAL_REQUEST else "user_input",
                provider_turn_id="",
                turn_id=operation.get("turn_id") or "",
            )
            _coordinator.register(interaction)
            _emit_kernel_event(
                model_request_id,
                "agent.policy.paused"
                if interaction.category == "approval"
                else "agent.message.paused",
                {
                    "providerSessionId": stored_session_id,
                    "interaction": {
                        "schemaVersion": 1,
                        "interactionId": str(interaction.provider_request_id),
                        "sessionId": session_id,
                        "category": interaction.category,
                        "kind": interaction.kind,
                        "request": payload,
                        "correlation": {
                            "modelRequestId": model_request_id,
                            "providerId": "hermes",
                            "providerRequestId": interaction.provider_request_id,
                            "providerSessionId": stored_session_id,
                            "protocolMethod": event_type,
                        },
                    },
                },
                sequence[0],
                stored_session_id,
                request_id,
            )
            # Wait for the kernel resolution (or reject after timeout).
            resolution = _resolve_pending_interaction(
                client,
                model_request_id,
                interaction.provider_request_id,
                INTERACTION_RESOLUTION_TIMEOUT_S,
            )
            if resolution is not None:
                try:
                    client.respond_interaction(interaction, resolution)
                    interaction.responded = True
                except Exception as error:
                    _emit_kernel_event(
                        model_request_id,
                        "agent.interaction.resolved",
                        {"error": str(error), "providerSessionId": stored_session_id},
                        sequence[0],
                        stored_session_id,
                        request_id,
                    )
            _emit_kernel_event(
                model_request_id,
                "agent.interaction.resolved",
                {
                    "providerSessionId": stored_session_id,
                    "interactionId": str(interaction.provider_request_id),
                    "resolved": interaction.responded,
                },
                sequence[0],
                stored_session_id,
                request_id,
            )

    text, usage, status = client.submit_prompt(live_session_id, prompt, on_event)
    if status == "failed" or status == "error":
        raise RuntimeError("Hermes turn failed with status {0}".format(status))
    if not (text or "").strip():
        raise RuntimeError("Hermes turn completed without assistant content")
    return text, usage, stored_session_id


# ---------------------------------------------------------------------------
# Operation handlers aligned with the kernel SdkRuntimeOperation surface.

def invoke_hermes_tui_gateway(operation):
    timeout_ms = operation.get("timeout_ms") or 300000
    if not isinstance(timeout_ms, int) or timeout_ms <= 0:
        return _hermes_failure(
            operation, "timeout_ms must be a positive integer"
        )
    try:
        text, usage, stored_session_id = _run_hermes_turn(
            hermes_tui_gateway_client(), operation, emit_stream=False
        )
    except TimeoutError:
        return _hermes_failure(operation, "Hermes TUI gateway model request timed out")
    except RuntimeError as error:
        return _hermes_failure(operation, str(error))
    return {
        "ok": True,
        "mode": "sdk_live",
        "messages": [text],
        "finish_reason": "stop",
        "package": "tui_gateway",
        "gateway_method": "prompt.submit",
        "provider_session_id": stored_session_id,
        "model_request_id": operation.get("model_request_id"),
        "diagnostics": [
            "sdk_runtime_mode=sdk_live",
            "sdk_runtime_provider_session_id={0}".format(stored_session_id or ""),
            "gateway_method=prompt.submit",
        ],
    }


def invoke_hermes_tui_gateway_stream(operation, request_id=None):
    model_request_id = operation.get("model_request_id")
    stored_session_id = None
    try:
        _text, _usage, stored_session_id = _run_hermes_turn(
            hermes_tui_gateway_client(), operation, emit_stream=True, request_id=request_id
        )
    except TimeoutError as error:
        write_stream_frame(
            request_id,
            {
                "event": _STREAM_DONE,
                "finish_reason": "error",
                "model_request_id": model_request_id,
                "provider_session_id": stored_session_id,
                "error": str(error),
            },
        )
        return
    except RuntimeError as error:
        write_stream_frame(
            request_id,
            {
                "event": _STREAM_DONE,
                "finish_reason": "error",
                "model_request_id": model_request_id,
                "provider_session_id": stored_session_id,
                "error": str(error),
            },
        )
        return
    write_stream_frame(
        request_id,
        {
            "event": _STREAM_DONE,
            "finish_reason": "stop",
            "model_request_id": model_request_id,
            "provider_session_id": stored_session_id,
        },
    )


def invoke_hermes_session_create(operation):
    client = hermes_tui_gateway_client()
    working_directory = operation.get("working_directory")
    model_id = operation.get("model_id")
    try:
        live_session_id, stored_session_id = client.session_create(
            working_directory, model_id, None
        )
    except TimeoutError:
        return _hermes_failure(operation, "Hermes session.create timed out")
    except RuntimeError as error:
        return _hermes_failure(operation, str(error))
    return {
        "ok": True,
        "mode": "sdk_live",
        "provider_session_id": stored_session_id,
        "session_id": live_session_id,
        "package": "tui_gateway",
        "gateway_method": "session.create",
        "model_request_id": operation.get("model_request_id"),
    }


def invoke_hermes_session_list(operation):
    client = hermes_tui_gateway_client()
    try:
        sessions = client.list_sessions()
    except TimeoutError:
        return _hermes_failure(operation, "Hermes session.list timed out")
    except RuntimeError as error:
        return _hermes_failure(operation, str(error))
    items = []
    for session in sessions:
        if not isinstance(session, dict):
            continue
        stored_id = _result_string(session, "id")
        items.append(
            {
                "provider_session_id": stored_id,
                "title": _result_string(session, "title"),
                "preview": _result_string(session, "preview"),
                "created_at": _result_string(session, "started_at"),
                "message_count": _result_int(session, "message_count"),
                "source": _result_string(session, "source"),
                "metadata": {},
            }
        )
    return {
        "ok": True,
        "mode": "sdk_live",
        "items": items,
        "package": "tui_gateway",
        "gateway_method": "session.list",
        "model_request_id": operation.get("model_request_id"),
    }


def invoke_hermes_session_history(operation):
    stored_session_id = (
        operation.get("provider_session_id") or operation.get("providerSessionId") or ""
    ).strip()
    if not stored_session_id:
        return _hermes_failure(operation, "session history requires a provider session id")
    client = hermes_tui_gateway_client()
    try:
        _live_session_id, messages = client.session_resume(stored_session_id)
    except TimeoutError:
        return _hermes_failure(operation, "Hermes session.resume timed out")
    except RuntimeError as error:
        return _hermes_failure(operation, str(error))
    items = []
    for message in messages:
        if not isinstance(message, dict):
            continue
        role = _result_string(message, "role") or "user"
        content = message.get("content")
        if isinstance(content, str):
            text = content
        elif isinstance(content, list):
            text = "".join(
                part.get("text", "")
                for part in content
                if isinstance(part, dict) and isinstance(part.get("text"), str)
            )
        else:
            text = ""
        items.append(
            {
                "provider_message_id": _result_string(message, "id")
                or "message.{0}.{1}".format(stored_session_id, len(items)),
                "provider_session_id": stored_session_id,
                "role": role,
                "parts": [{"part_id": str(len(items)), "kind": "text", "text": text}],
                "created_at": _result_string(message, "timestamp"),
                "metadata": {},
            }
        )
    return {
        "ok": True,
        "mode": "sdk_live",
        "items": items,
        "package": "tui_gateway",
        "gateway_method": "session.resume",
        "model_request_id": operation.get("model_request_id"),
    }


def invoke_hermes_session_interrupt(operation):
    stored_session_id = (
        operation.get("provider_session_id") or operation.get("providerSessionId") or ""
    ).strip()
    client = hermes_tui_gateway_client()
    if stored_session_id:
        try:
            live_session_id, _messages = client.session_resume(stored_session_id)
        except Exception:
            live_session_id = None
    else:
        live_session_id = None
    if live_session_id:
        try:
            client.interrupt(live_session_id)
        except Exception as error:
            return _hermes_failure(operation, str(error))
    return {
        "ok": True,
        "mode": "sdk_live",
        "status": "applied" if live_session_id else "no_op",
        "package": "tui_gateway",
        "gateway_method": "session.interrupt",
        "model_request_id": operation.get("model_request_id"),
    }


def _hermes_failure(operation, message):
    return {
        "ok": False,
        "mode": "sdk_live_failed",
        "operation": "model_chat",
        "error": message,
        "model_request_id": operation.get("model_request_id"),
    }


def resolve_model_prompt(operation):
    wire_messages = operation.get("wire_messages")
    if isinstance(wire_messages, list) and wire_messages:
        user_messages = [
            entry
            for entry in wire_messages
            if isinstance(entry, dict) and entry.get("role") == "user"
        ]
        message = user_messages[-1] if user_messages else wire_messages[-1]
        content = message.get("content") if isinstance(message, dict) else None
        if isinstance(content, str):
            return content
        if isinstance(content, list):
            parts = []
            for part in content:
                if not isinstance(part, dict):
                    continue
                if part.get("type") == "text" and isinstance(part.get("text"), str):
                    parts.append(part["text"])
                elif isinstance(part.get("content"), str):
                    parts.append(part["content"])
            if parts:
                return "".join(parts)
    return "\n".join(operation.get("messages") or [])


def _hermes_session_control(operation):
    op = operation.get("operation", operation)
    if op == "session_interrupt":
        return invoke_hermes_session_interrupt(operation)
    return {
        "ok": False,
        "mode": "sdk_live_failed",
        "operation": op,
        "error": "Hermes TUI gateway does not implement {0}".format(op),
        "model_request_id": operation.get("model_request_id"),
    }


def _invoke_hermes_turn_request(request_id, operation, op):
    """Runs one Hermes turn on a worker thread (see handle_request)."""
    if op == "model_chat_stream":
        invoke_hermes_tui_gateway_stream(operation, request_id=request_id)
        return
    result = invoke_hermes_tui_gateway(operation)
    write_response({"jsonrpc": "2.0", "id": request_id, "result": result})


def respond_to_pending_interaction(params):
    """Handle `sdkwork/serverRequest.respond` from the kernel runtime."""
    model_request_id = params.get("model_request_id") or params.get("modelRequestId")
    provider_request_id = params.get("provider_request_id") or params.get("providerRequestId")
    if not model_request_id or provider_request_id is None:
        return {
            "ok": False,
            "error": "respond requires model_request_id and provider_request_id",
        }
    interaction = _coordinator.find(model_request_id, provider_request_id)
    if interaction is None:
        return None
    resolution = params.get("resolution")
    if not isinstance(resolution, dict):
        return {
            "ok": False,
            "error": "resolution must be an object",
        }
    interaction.resolution = resolution
    interaction.resolved.set()
    return {
        "ok": True,
        "model_request_id": model_request_id,
        "provider_session_id": interaction.stored_session_id,
        "provider_request_id": provider_request_id,
        "interaction_kind": interaction.kind,
        "status": "responded",
    }


# ---------------------------------------------------------------------------
# Generic worker surface shared with the TypeScript worker protocol.

def matches_truthy(value):
    return value.strip().lower() in ("1", "true", "yes", "on")


def matches_falsy(value):
    return value.strip().lower() in ("0", "false", "no", "off")


def production_kernel_profile():
    environment = os.environ.get("SDKWORK_KERNEL_ENVIRONMENT", "").strip().lower()
    if environment in ("production", "prod"):
        return True
    profile = os.environ.get("SDKWORK_KERNEL_PROFILE_ID", "").strip().lower()
    return profile.endswith(".production")


def mock_provider_invocation_allowed():
    override = os.environ.get("SDKWORK_KERNEL_ALLOW_MOCK_PROVIDERS", "")
    if production_kernel_profile():
        return matches_truthy(override) if override else False
    if override:
        return not matches_falsy(override) or matches_truthy(override)
    return True


def fail_closed_synthetic_operation(operation_name, package_name, package_probe, model_request_id=None):
    package_resolved = package_probe["resolved"]
    if package_resolved:
        error = (
            f"{operation_name} requires a live provider SDK implementation "
            "and mock fallback is disabled"
        )
    else:
        error = (
            f"official sdk package is not resolved for {operation_name} "
            f"and mock fallback is disabled: {package_name}"
        )
    return {
        "ok": False,
        "mode": "sdk_live_failed",
        "package": package_name,
        "package_resolved": package_resolved,
        "operation": operation_name,
        "error": error,
        "model_request_id": model_request_id,
    }


_hermes_tui_gateway = None


def hermes_tui_gateway_client():
    global _hermes_tui_gateway
    if _hermes_tui_gateway is None:
        _hermes_tui_gateway = HermesTuiGatewayClient()
    return _hermes_tui_gateway


def handle_capability_invoke(params, package_name):
    operation = params.get("operation") or {}
    if isinstance(operation, dict):
        op = operation.get("operation", operation)
    else:
        op = operation

    package_probe = probe_module(package_name)

    if op == "ping":
        return {
            "ok": True,
            "backend": "python_process",
            "package": package_name,
            "package_resolved": package_probe["resolved"],
        }

    if package_name == "tui_gateway":
        if op == "session_create":
            return invoke_hermes_session_create(operation)
        if op == "session_list":
            return invoke_hermes_session_list(operation)
        if op == "session_history":
            return invoke_hermes_session_history(operation)
        if op == "session_interrupt":
            return invoke_hermes_session_interrupt(operation)
        if op == "session_compact" or op == "session_fork":
            return _hermes_session_control(operation)
        if op == "model_chat":
            return invoke_hermes_tui_gateway(operation)
        if op == "model_chat_stream":
            return {"ok": True, "mode": "streaming"}  # frames are written directly

    if op == "session_create":
        if not mock_provider_invocation_allowed():
            return fail_closed_synthetic_operation(op, package_name, package_probe)
        return {
            "ok": True,
            "mode": "sdk_probe" if package_probe["resolved"] else "stub",
            "agent_id": operation.get("agent_id"),
            "user_ref": operation.get("user_ref"),
            "package": package_name,
        }

    if op == "model_chat":
        if not mock_provider_invocation_allowed():
            return fail_closed_synthetic_operation(
                op,
                package_name,
                package_probe,
                operation.get("model_request_id"),
            )
        messages = operation.get("messages") or []
        prompt = "\n".join(messages)
        prefix = f"[{package_name}]" if package_probe["resolved"] else f"[{package_name} stub]"
        return {
            "ok": True,
            "mode": "sdk_probe" if package_probe["resolved"] else "stub",
            "messages": [f"{prefix} {prompt}"],
            "finish_reason": "stop",
            "package": package_name,
            "model_request_id": operation.get("model_request_id"),
        }

    result = fail_closed_synthetic_operation(op, package_name, package_probe)
    result["mode"] = "unsupported_operation"
    result["error"] = (
        f"operation is not implemented by the official provider SDK adapter: {op}"
    )
    return result


def handle_request(request, package_name):
    method = request.get("method")
    request_id = request.get("id")

    if method == "sdkwork/ping":
        probe = probe_module(package_name)
        write_response(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "ok": True,
                    "backend": "python_process",
                    "package": package_name,
                    "package_resolved": probe["resolved"],
                },
            }
        )
        return

    if method == "sdkwork/serverRequest.respond":
        result = respond_to_pending_interaction(request.get("params") or {})
        write_response({"jsonrpc": "2.0", "id": request_id, "result": result})
        return

    if method == "sdkwork/capability.invoke":
        params = request.get("params") or {}
        operation = params.get("operation") or {}
        if isinstance(operation, dict):
            op = operation.get("operation", operation)
        else:
            op = operation
        if package_name == "tui_gateway" and (
            op == "model_chat" or op == "model_chat_stream"
        ):
            # Turns can block on gateway interactions (approval/clarify), so
            # they run on a worker thread while the main loop keeps serving
            # control requests such as sdkwork/serverRequest.respond.
            threading.Thread(
                target=_invoke_hermes_turn_request,
                args=(request_id, operation, op),
                daemon=True,
            ).start()
            return
        write_response(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": handle_capability_invoke(params, package_name),
            }
        )
        return

    write_response(
        {
            "jsonrpc": "2.0",
            "id": request_id,
            "error": {
                "code": -32601,
                "message": f"Method not found: {method}",
            },
        }
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--package", default="unknown")
    args = parser.parse_args()

    for line in sys.stdin:
        trimmed = line.strip()
        if not trimmed:
            continue
        try:
            request = json.loads(trimmed)
        except json.JSONDecodeError as error:
            write_response(
                {
                    "jsonrpc": "2.0",
                    "id": None,
                    "error": {
                        "code": -32700,
                        "message": f"Parse error: {error}",
                    },
                }
            )
            continue
        handle_request(request, args.package)


if __name__ == "__main__":
    main()
