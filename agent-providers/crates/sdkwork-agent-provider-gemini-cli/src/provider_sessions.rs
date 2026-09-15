use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use sdkwork_agent_kernel::{
    AgentMessage, AgentMessageRole, AgentPart, AgentSession, KernelError, KernelResult,
};
use sdkwork_agent_provider_core::{
    sort_sessions_by_updated_at, ProviderSessionHistoryBudget, ProviderSessionHistoryLimits,
    SessionAdapter,
};
use serde_json::{Map, Value};

use crate::{GeminiCliAdapter, GeminiConversationRecord};

pub fn discover_gemini_cli_provider_sessions() -> KernelResult<Vec<AgentSession>> {
    let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) else {
        return Ok(Vec::new());
    };
    let temp_path = PathBuf::from(home).join(".gemini").join("tmp");
    if !temp_path.is_dir() {
        return Ok(Vec::new());
    }
    read_gemini_cli_provider_sessions(&temp_path)
}

pub fn discover_gemini_cli_provider_session_messages(
    session_id: &str,
) -> KernelResult<Vec<AgentMessage>> {
    let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) else {
        return Ok(Vec::new());
    };
    let temp_path = PathBuf::from(home).join(".gemini").join("tmp");
    if !temp_path.is_dir() {
        return Ok(Vec::new());
    }
    read_gemini_cli_provider_session_messages(&temp_path, session_id)
}

pub fn read_gemini_cli_provider_sessions(temp_path: &Path) -> KernelResult<Vec<AgentSession>> {
    let mut records = HashMap::<String, (GeminiConversationRecord, bool)>::new();
    for (path, project_root, parent_session_id) in gemini_session_files(temp_path)? {
        let Some(conversation) = read_gemini_conversation(&path)? else {
            continue;
        };
        let record =
            gemini_conversation_record(&conversation, project_root.as_deref(), parent_session_id)?;
        let is_jsonl = path.extension().and_then(|value| value.to_str()) == Some("jsonl");
        match records.get(&record.session_id) {
            Some((existing, existing_is_jsonl))
                if !is_newer_gemini_record(&record, is_jsonl, existing, *existing_is_jsonl) => {}
            _ => {
                records.insert(record.session_id.clone(), (record, is_jsonl));
            }
        }
    }
    let mut sessions = records
        .into_values()
        .map(|(record, _)| GeminiCliAdapter::new().to_agent_session(&record))
        .collect::<KernelResult<Vec<_>>>()?;
    sort_sessions_by_updated_at(&mut sessions);
    Ok(sessions)
}

pub fn read_gemini_cli_provider_session_messages(
    temp_path: &Path,
    session_id: &str,
) -> KernelResult<Vec<AgentMessage>> {
    if session_id.trim().is_empty() {
        return Err(KernelError::validation(
            "Gemini provider session id must not be empty",
        ));
    }
    let mut selected: Option<(GeminiConversation, bool)> = None;
    for (path, _, _) in gemini_session_files(temp_path)? {
        let Some(conversation) = read_gemini_conversation(&path)? else {
            continue;
        };
        if conversation
            .metadata
            .get("sessionId")
            .and_then(Value::as_str)
            == Some(session_id)
        {
            let is_jsonl = path.extension().and_then(|value| value.to_str()) == Some("jsonl");
            match selected.as_ref() {
                Some((existing, existing_is_jsonl))
                    if !is_newer_gemini_conversation(
                        &conversation,
                        is_jsonl,
                        existing,
                        *existing_is_jsonl,
                    ) => {}
                _ => selected = Some((conversation, is_jsonl)),
            }
        }
    }
    selected.map_or_else(
        || Ok(Vec::new()),
        |(conversation, _)| gemini_agent_messages(&conversation, session_id),
    )
}

#[derive(Debug)]
struct GeminiConversation {
    metadata: Map<String, Value>,
    messages: Vec<Value>,
}

fn read_gemini_conversation(path: &Path) -> KernelResult<Option<GeminiConversation>> {
    match path.extension().and_then(|value| value.to_str()) {
        Some("json") => read_legacy_gemini_conversation(path),
        Some("jsonl") => read_jsonl_gemini_conversation(path),
        _ => Ok(None),
    }
}

fn read_legacy_gemini_conversation(path: &Path) -> KernelResult<Option<GeminiConversation>> {
    ProviderSessionHistoryBudget::new(ProviderSessionHistoryLimits::default())
        .validate_file_size(path)?;
    let serialized = fs::read_to_string(path).map_err(gemini_inventory_error)?;
    let Ok(Value::Object(mut metadata)) = serde_json::from_str::<Value>(&serialized) else {
        return Ok(None);
    };
    let messages = metadata
        .remove("messages")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    Ok(Some(GeminiConversation { metadata, messages }))
}

fn is_newer_gemini_record(
    candidate: &GeminiConversationRecord,
    candidate_is_jsonl: bool,
    existing: &GeminiConversationRecord,
    existing_is_jsonl: bool,
) -> bool {
    candidate
        .last_updated
        .as_deref()
        .unwrap_or_default()
        .cmp(existing.last_updated.as_deref().unwrap_or_default())
        .then_with(|| candidate_is_jsonl.cmp(&existing_is_jsonl))
        .is_gt()
}

fn is_newer_gemini_conversation(
    candidate: &GeminiConversation,
    candidate_is_jsonl: bool,
    existing: &GeminiConversation,
    existing_is_jsonl: bool,
) -> bool {
    optional_string(&candidate.metadata, "lastUpdated")
        .as_deref()
        .unwrap_or_default()
        .cmp(
            optional_string(&existing.metadata, "lastUpdated")
                .as_deref()
                .unwrap_or_default(),
        )
        .then_with(|| candidate_is_jsonl.cmp(&existing_is_jsonl))
        .is_gt()
}

fn read_jsonl_gemini_conversation(path: &Path) -> KernelResult<Option<GeminiConversation>> {
    let mut budget = ProviderSessionHistoryBudget::new(ProviderSessionHistoryLimits::default());
    budget.validate_file_size(path)?;
    let file = File::open(path).map_err(gemini_inventory_error)?;
    let mut metadata = Map::new();
    let mut messages = Vec::new();
    let mut message_indexes = HashMap::new();

    for line in BufReader::new(file).lines() {
        let line = line.map_err(gemini_inventory_error)?;
        budget.record_source(line.len())?;
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(rewind_to) = value.get("$rewindTo").and_then(Value::as_str) {
            if let Some(index) = message_indexes.get(rewind_to).copied() {
                messages.truncate(index);
                rebuild_message_indexes(&messages, &mut message_indexes);
            } else {
                messages.clear();
                message_indexes.clear();
            }
            continue;
        }
        if let Some(set) = value.get("$set").and_then(Value::as_object) {
            if let Some(replacement_messages) = set.get("messages").and_then(Value::as_array) {
                messages.clear();
                message_indexes.clear();
                for message in replacement_messages {
                    upsert_gemini_message(
                        message.clone(),
                        &mut messages,
                        &mut message_indexes,
                        &mut budget,
                    )?;
                }
            }
            for (key, value) in set {
                if key != "messages" {
                    metadata.insert(key.clone(), value.clone());
                }
            }
            continue;
        }
        if value.get("id").and_then(Value::as_str).is_some() {
            upsert_gemini_message(value, &mut messages, &mut message_indexes, &mut budget)?;
            continue;
        }
        if value.get("sessionId").and_then(Value::as_str).is_some()
            && value.get("projectHash").and_then(Value::as_str).is_some()
        {
            if let Some(initial_messages) = value.get("messages").and_then(Value::as_array) {
                for message in initial_messages {
                    upsert_gemini_message(
                        message.clone(),
                        &mut messages,
                        &mut message_indexes,
                        &mut budget,
                    )?;
                }
            }
            if let Some(initial_metadata) = value.as_object() {
                for (key, value) in initial_metadata {
                    if key != "messages" {
                        metadata.insert(key.clone(), value.clone());
                    }
                }
            }
        }
    }

    if metadata.get("sessionId").and_then(Value::as_str).is_none() {
        return Ok(None);
    }
    Ok(Some(GeminiConversation { metadata, messages }))
}

fn upsert_gemini_message(
    message: Value,
    messages: &mut Vec<Value>,
    message_indexes: &mut HashMap<String, usize>,
    budget: &mut ProviderSessionHistoryBudget,
) -> KernelResult<()> {
    let Some(message_id) = message
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
    else {
        return Ok(());
    };
    if let Some(index) = message_indexes.get(&message_id).copied() {
        messages[index] = message;
    } else {
        budget.record_message()?;
        message_indexes.insert(message_id, messages.len());
        messages.push(message);
    }
    Ok(())
}

fn rebuild_message_indexes(messages: &[Value], indexes: &mut HashMap<String, usize>) {
    indexes.clear();
    for (index, message) in messages.iter().enumerate() {
        if let Some(message_id) = message.get("id").and_then(Value::as_str) {
            indexes.insert(message_id.to_string(), index);
        }
    }
}

fn gemini_conversation_record(
    conversation: &GeminiConversation,
    project_root: Option<&str>,
    parent_session_id: Option<String>,
) -> KernelResult<GeminiConversationRecord> {
    let session_id = required_string(&conversation.metadata, "sessionId")?;
    let first_user_message = conversation
        .messages
        .iter()
        .find(|message| message.get("type").and_then(Value::as_str) == Some("user"))
        .and_then(gemini_content_text);
    let model = conversation.messages.iter().rev().find_map(|message| {
        message
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    let directories = conversation
        .metadata
        .get("directories")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<Vec<_>>();
    let cwd = project_root
        .map(str::to_string)
        .or_else(|| directories.first().cloned());
    let title = conversation
        .metadata
        .get("summary")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or(first_user_message);
    let memory_scratchpad = conversation
        .metadata
        .get("memoryScratchpad")
        .filter(|value| !value.is_null())
        .map(Value::to_string);

    Ok(GeminiConversationRecord {
        session_id,
        start_time: optional_string(&conversation.metadata, "startTime"),
        last_updated: optional_string(&conversation.metadata, "lastUpdated"),
        messages: conversation
            .messages
            .iter()
            .filter_map(|message| {
                message
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect(),
        summary: optional_string(&conversation.metadata, "summary"),
        kind: optional_string(&conversation.metadata, "kind").unwrap_or_else(|| "main".to_string()),
        memory_scratchpad,
        model,
        title,
        cwd,
        workspace_roots: directories,
        parent_session_id,
    })
}

fn gemini_agent_messages(
    conversation: &GeminiConversation,
    session_id: &str,
) -> KernelResult<Vec<AgentMessage>> {
    conversation
        .messages
        .iter()
        .filter_map(|value| gemini_agent_message(value, session_id))
        .collect()
}

fn gemini_agent_message(value: &Value, session_id: &str) -> Option<KernelResult<AgentMessage>> {
    let message_id = value.get("id")?.as_str()?.trim();
    if message_id.is_empty() {
        return None;
    }
    let message_type = value.get("type")?.as_str()?;
    let role = match message_type {
        "user" => AgentMessageRole::User,
        "gemini" => AgentMessageRole::Agent,
        "info" | "warning" | "error" => AgentMessageRole::Adapter,
        _ => return None,
    };
    let mut parts = Vec::new();
    if let Some(content) = gemini_content_text(value).filter(|text| !text.trim().is_empty()) {
        let mut part = if message_type == "error" {
            AgentPart::error(format!("{message_id}.content"), "gemini_error", content)
        } else {
            AgentPart::text(format!("{message_id}.content"), content)
        };
        part = part
            .from_provider("gemini-cli")
            .with_metadata("gemini.content_type", message_type);
        parts.push(part);
    }
    if message_type == "gemini" {
        parts.extend(gemini_thought_parts(value, message_id));
        parts.extend(gemini_tool_call_parts(value, message_id));
    }
    if parts.is_empty() {
        parts.push(
            AgentPart::json(format!("{message_id}.record"), value.to_string())
                .from_provider("gemini-cli")
                .with_metadata("gemini.content_type", message_type),
        );
    }
    let mut message = AgentMessage::new(message_id, role, parts)
        .for_session(session_id)
        .with_metadata("gemini.message_type", message_type);
    if let Some(timestamp) = value.get("timestamp").and_then(Value::as_str) {
        message = message.created_at(timestamp);
    }
    Some(Ok(message))
}

fn gemini_thought_parts(value: &Value, message_id: &str) -> Vec<AgentPart> {
    value
        .get("thoughts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, thought)| {
            let subject = thought.get("subject").and_then(Value::as_str);
            let description = thought.get("description").and_then(Value::as_str);
            let text = match (subject, description) {
                (Some(subject), Some(description)) => format!("{subject}\n\n{description}"),
                (Some(subject), None) => subject.to_string(),
                (None, Some(description)) => description.to_string(),
                (None, None) => return None,
            };
            Some(
                AgentPart::text(format!("{message_id}.thought.{index}"), text)
                    .from_provider("gemini-cli")
                    .with_metadata("gemini.content_type", "thought"),
            )
        })
        .collect()
}

fn gemini_tool_call_parts(value: &Value, message_id: &str) -> Vec<AgentPart> {
    value
        .get("toolCalls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, tool_call)| {
            let tool_call_id = tool_call.get("id").and_then(Value::as_str)?;
            let tool_name = tool_call
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("tool");
            let status = tool_call
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("pending");
            let has_result = tool_call
                .get("result")
                .is_some_and(|result| !result.is_null())
                || matches!(
                    status,
                    "success" | "completed" | "error" | "failed" | "cancelled"
                );
            let mut part =
                AgentPart::tool_call_ref(format!("{message_id}.tool.{index}"), tool_call_id)
                    .with_name(tool_name)
                    .from_provider("gemini-cli")
                    .with_metadata("gemini.content_type", "tool_call")
                    .with_metadata("gemini.status", status)
                    .with_metadata(
                        "gemini.has_result",
                        if has_result { "true" } else { "false" },
                    );
            part.json = Some(tool_call.to_string());
            Some(part)
        })
        .collect()
}

fn gemini_content_text(value: &Value) -> Option<String> {
    let content = value
        .get("displayContent")
        .or_else(|| value.get("content"))?;
    match content {
        Value::String(text) => Some(text.to_string()),
        Value::Array(parts) => {
            let text = parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("");
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    }
}

/// One Gemini session file: absolute path, optional project key, and
/// optional chat key.
type GeminiSessionFile = (PathBuf, Option<String>, Option<String>);

fn gemini_session_files(temp_path: &Path) -> KernelResult<Vec<GeminiSessionFile>> {
    let mut files = Vec::new();
    for project_entry in fs::read_dir(temp_path).map_err(gemini_inventory_error)? {
        let project_path = project_entry.map_err(gemini_inventory_error)?.path();
        if !project_path.is_dir() {
            continue;
        }
        let chats_path = project_path.join("chats");
        if !chats_path.is_dir() {
            continue;
        }
        let project_root = read_project_root(&project_path)?;
        collect_gemini_chat_files(&chats_path, project_root.as_deref(), None, &mut files)?;
    }
    Ok(files)
}

fn collect_gemini_chat_files(
    directory: &Path,
    project_root: Option<&str>,
    parent_session_id: Option<&str>,
    files: &mut Vec<GeminiSessionFile>,
) -> KernelResult<()> {
    for entry in fs::read_dir(directory).map_err(gemini_inventory_error)? {
        let path = entry.map_err(gemini_inventory_error)?.path();
        if path.is_dir() {
            let nested_parent = path
                .file_name()
                .and_then(|value| value.to_str())
                .or(parent_session_id);
            collect_gemini_chat_files(&path, project_root, nested_parent, files)?;
            continue;
        }
        if matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("json" | "jsonl")
        ) {
            files.push((
                path,
                project_root.map(str::to_string),
                parent_session_id.map(str::to_string),
            ));
        }
    }
    Ok(())
}

fn read_project_root(project_path: &Path) -> KernelResult<Option<String>> {
    let root_path = project_path.join(".project_root");
    if !root_path.is_file() {
        return Ok(None);
    }
    let root = fs::read_to_string(root_path).map_err(gemini_inventory_error)?;
    Ok((!root.trim().is_empty()).then(|| root.trim().to_string()))
}

fn required_string(metadata: &Map<String, Value>, key: &str) -> KernelResult<String> {
    optional_string(metadata, key)
        .ok_or_else(|| KernelError::validation(format!("Gemini conversation is missing {key}")))
}

fn optional_string(metadata: &Map<String, Value>, key: &str) -> Option<String> {
    metadata
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn gemini_inventory_error(error: std::io::Error) -> KernelError {
    KernelError::provider_error("gemini_cli_provider_session_inventory", error.to_string())
}

#[cfg(test)] // WORKSPACE-PATH:allow-fixture-block: this module is the file's #[cfg(test)] unit-test fixture data
mod tests {
    use super::*;

    #[test]
    fn reads_legacy_and_jsonl_sessions_with_project_roots() {
        let root = fixture_root("inventory");
        let legacy_project = root.join("legacy-project");
        let modern_project = root.join("modern-project");
        std::fs::create_dir_all(legacy_project.join("chats")).expect("legacy chats");
        std::fs::create_dir_all(modern_project.join("chats")).expect("modern chats");
        std::fs::write(legacy_project.join(".project_root"), "E:/work/legacy")
            .expect("legacy root");
        std::fs::write(modern_project.join(".project_root"), "E:/work/modern")
            .expect("modern root");
        std::fs::write(
            legacy_project.join("chats/session-legacy.json"),
            r#"{"sessionId":"legacy","projectHash":"legacy-project","startTime":"2026-07-01T00:00:00Z","lastUpdated":"2026-07-01T00:00:01Z","messages":[{"id":"legacy-user","timestamp":"2026-07-01T00:00:00Z","type":"user","content":"legacy prompt"}]}"#,
        )
        .expect("legacy conversation");
        std::fs::write(
            modern_project.join("chats/session-modern.jsonl"),
            concat!(
                "{\"sessionId\":\"modern\",\"projectHash\":\"modern-project\",\"startTime\":\"2026-07-01T00:00:00Z\",\"lastUpdated\":\"2026-07-01T00:00:02Z\"}\n",
                "{\"id\":\"modern-user\",\"timestamp\":\"2026-07-01T00:00:00Z\",\"type\":\"user\",\"content\":\"modern prompt\"}\n"
            ),
        )
        .expect("modern conversation");

        let sessions = read_gemini_cli_provider_sessions(&root).expect("Gemini sessions");

        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].session_id, "modern");
        assert_eq!(sessions[0].cwd.as_deref(), Some("E:/work/modern"));
        assert_eq!(sessions[1].session_id, "legacy");
        assert_eq!(sessions[1].title.as_deref(), Some("legacy prompt"));
        std::fs::remove_dir_all(root).expect("remove inventory fixture");
    }

    #[test]
    fn jsonl_replay_uses_latest_message_checkpoint_and_rewind_semantics() {
        let root = fixture_root("replay");
        let project = root.join("project");
        std::fs::create_dir_all(project.join("chats")).expect("chats");
        let path = project.join("chats/session-replay.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{\"sessionId\":\"replay\",\"projectHash\":\"project\",\"startTime\":\"2026-07-01T00:00:00Z\",\"lastUpdated\":\"2026-07-01T00:00:01Z\"}\n",
                "{\"id\":\"message-one\",\"timestamp\":\"2026-07-01T00:00:01Z\",\"type\":\"gemini\",\"content\":\"partial\"}\n",
                "{\"id\":\"message-two\",\"timestamp\":\"2026-07-01T00:00:02Z\",\"type\":\"user\",\"content\":\"discarded\"}\n",
                "{\"id\":\"message-one\",\"timestamp\":\"2026-07-01T00:00:03Z\",\"type\":\"gemini\",\"content\":\"final answer\"}\n",
                "{\"$rewindTo\":\"message-two\"}\n",
                "{\"$set\":{\"lastUpdated\":\"2026-07-01T00:00:04Z\",\"messages\":[{\"id\":\"checkpoint-user\",\"timestamp\":\"2026-07-01T00:00:00Z\",\"type\":\"user\",\"content\":\"prompt\"},{\"id\":\"checkpoint-agent\",\"timestamp\":\"2026-07-01T00:00:04Z\",\"type\":\"gemini\",\"content\":\"checkpoint final\",\"toolCalls\":[{\"id\":\"tool-1\",\"name\":\"read_file\",\"args\":{\"path\":\"README.md\"},\"result\":[],\"status\":\"success\",\"timestamp\":\"2026-07-01T00:00:04Z\"}] }]}}\n"
            ),
        )
        .expect("replay conversation");

        let messages =
            read_gemini_cli_provider_session_messages(&root, "replay").expect("Gemini messages");

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].message_id, "checkpoint-user");
        assert_eq!(messages[1].message_id, "checkpoint-agent");
        assert_eq!(
            messages[1].parts[0].text.as_deref(),
            Some("checkpoint final")
        );
        assert_eq!(messages[1].parts[1].tool_call_id.as_deref(), Some("tool-1"));
        assert_eq!(
            messages[1].parts[1].metadata_value("gemini.has_result"),
            Some("true")
        );
        std::fs::remove_dir_all(root).expect("remove replay fixture");
    }

    #[test]
    fn migrated_jsonl_wins_over_stale_legacy_json_for_inventory_and_history() {
        let root = fixture_root("migration");
        let project = root.join("project");
        std::fs::create_dir_all(project.join("chats")).expect("chats");
        std::fs::write(project.join(".project_root"), "E:/work/migrated").expect("project root");
        std::fs::write(
            project.join("chats/session-migrated.json"),
            r#"{"sessionId":"migrated","projectHash":"project","startTime":"2026-07-01T00:00:00Z","lastUpdated":"2026-07-01T00:00:01Z","messages":[{"id":"answer","timestamp":"2026-07-01T00:00:01Z","type":"gemini","content":"stale"}]}"#,
        )
        .expect("legacy conversation");
        std::fs::write(
            project.join("chats/session-migrated.jsonl"),
            concat!(
                "{\"sessionId\":\"migrated\",\"projectHash\":\"project\",\"startTime\":\"2026-07-01T00:00:00Z\",\"lastUpdated\":\"2026-07-01T00:00:02Z\"}\n",
                "{\"id\":\"answer\",\"timestamp\":\"2026-07-01T00:00:02Z\",\"type\":\"gemini\",\"content\":\"current\"}\n"
            ),
        )
        .expect("migrated conversation");

        let sessions = read_gemini_cli_provider_sessions(&root).expect("Gemini sessions");
        let messages =
            read_gemini_cli_provider_session_messages(&root, "migrated").expect("Gemini messages");

        assert_eq!(sessions.len(), 1);
        assert_eq!(
            sessions[0].updated_at.as_deref(),
            Some("2026-07-01T00:00:02Z")
        );
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].parts[0].text.as_deref(), Some("current"));
        std::fs::remove_dir_all(root).expect("remove migration fixture");
    }

    fn fixture_root(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("test clock")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "sdkwork-gemini-provider-session-{label}-{}-{nonce}",
            std::process::id()
        ))
    }
}
