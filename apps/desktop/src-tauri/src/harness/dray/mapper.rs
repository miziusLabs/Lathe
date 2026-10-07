//! Lathe RPC events → normalized [`AgentEvent`](crate::events::AgentEvent)s.
//!
//! Lathe keeps messages and extension payloads deliberately open-ended. This
//! mapper therefore uses the stable lifecycle/tool envelopes for structure and
//! leaves extension arguments and results as JSON values for the UI.

use crate::{
    events::{
        now_rfc3339, AgentEvent, AgentEventPayload, BlockRef, BlockType, ContextWindow, DeltaEvent,
        ErrorSource, Question, QuestionOption, SessionInfo, ToolResult, ToolType, TurnStatus,
        Usage,
    },
    harness::{
        dray::parser::{AgentRpcEvent, AgentRpcUsage, AssistantMessageEvent},
        Harness,
    },
};
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicU64, Ordering::Relaxed},
        Arc, Mutex,
    },
};
use uuid::Uuid;

/// A Lathe extension dialog waiting for a response from the host UI.
#[derive(Debug, Clone)]
pub struct PendingUiRequest {
    pub id: String,
    pub method: String,
    pub question: String,
}

/// Pending Lathe extension dialogs shared by the stdout task and Tauri commands.
pub type PendingUiRequests = Arc<Mutex<HashMap<String, PendingUiRequest>>>;

impl PendingUiRequest {
    /// Converts a questionnaire answer into Lathe's extension UI response shape.
    pub fn response(&self, answers: &HashMap<String, String>) -> Value {
        let Some(answer) = answers.get(&self.question) else {
            return json!({
                "type": "extension_ui_response",
                "id": self.id,
                "cancelled": true,
            });
        };

        match self.method.as_str() {
            "select" => json!({
                "type": "extension_ui_response",
                "id": self.id,
                "value": answer,
            }),
            "confirm" => json!({
                "type": "extension_ui_response",
                "id": self.id,
                "confirmed": matches!(answer.to_ascii_lowercase().as_str(), "yes" | "true"),
            }),
            "input" | "editor" => json!({
                "type": "extension_ui_response",
                "id": self.id,
                "value": answer,
            }),
            _ => json!({
                "type": "extension_ui_response",
                "id": self.id,
                "cancelled": true,
            }),
        }
    }
}

/// Stateful mapper for one Lathe RPC child.
pub struct Mapper {
    seq: Arc<AtomicU64>,
    session_id: String,
    cwd: String,
    current_message_id: Option<String>,
    next_message_id: u64,
    streamed_blocks: HashSet<(String, u32)>,
    last_text: Option<String>,
    last_usage: Option<Usage>,
    last_stop_reason: Option<String>,
    last_turn_error: bool,
    pending_ui: PendingUiRequests,
}

impl Default for Mapper {
    fn default() -> Self {
        Self::new("dray-session", "")
    }
}

impl Mapper {
    /// Creates a mapper whose events are attributed to the Lathe session.
    pub fn new(session_id: impl Into<String>, cwd: impl Into<String>) -> Self {
        Self {
            seq: Arc::new(AtomicU64::new(0)),
            session_id: session_id.into(),
            cwd: cwd.into(),
            current_message_id: None,
            next_message_id: 0,
            streamed_blocks: HashSet::new(),
            last_text: None,
            last_usage: None,
            last_stop_reason: None,
            last_turn_error: false,
            pending_ui: PendingUiRequests::default(),
        }
    }

    /// Uses the session's shared event counter, including events synthesized by
    /// the session layer for prompts.
    pub fn with_seq(
        session_id: impl Into<String>,
        cwd: impl Into<String>,
        seq: Arc<AtomicU64>,
    ) -> Self {
        Self::with_seq_and_ui(session_id, cwd, seq, PendingUiRequests::default())
    }

    /// Uses a shared request map so Lathe extension dialogs can be answered by
    /// Tauri commands while the mapper remains synchronous.
    pub fn with_seq_and_ui(
        session_id: impl Into<String>,
        cwd: impl Into<String>,
        seq: Arc<AtomicU64>,
        pending_ui: PendingUiRequests,
    ) -> Self {
        let mut mapper = Self::new(session_id, cwd);
        mapper.seq = seq;
        mapper.pending_ui = pending_ui;
        mapper
    }

    /// Maps one Lathe record. A message can contain several content blocks, so a
    /// single wire line may produce several normalized events.
    pub fn map(&mut self, event: AgentRpcEvent) -> Result<Vec<AgentEvent>> {
        let payloads = match event {
            AgentRpcEvent::RequestUsage { usage, model } => vec![AgentEventPayload::UsageUpdate(
                map_usage(&usage, Some(&model)),
            )],
            AgentRpcEvent::RequestError {
                status,
                code,
                message,
                request_id,
            } => vec![AgentEventPayload::Error {
                source: ErrorSource::Provider,
                message: format!("OpenAI {status} ({code}): {message} [request {request_id}]"),
                fatal: false,
            }],
            AgentRpcEvent::Session { cwd, .. } => {
                if self.cwd.is_empty() {
                    self.cwd = cwd;
                }
                Vec::new()
            }
            AgentRpcEvent::AgentStart => {
                self.last_text = None;
                self.last_usage = None;
                self.last_stop_reason = None;
                self.last_turn_error = false;
                self.current_message_id = None;
                self.streamed_blocks.clear();
                vec![AgentEventPayload::TurnStarted(SessionInfo {
                    cwd: (!self.cwd.is_empty()).then(|| self.cwd.clone()),
                    ..SessionInfo::default()
                })]
            }
            AgentRpcEvent::AgentSettled => vec![AgentEventPayload::TurnCompleted {
                status: if self.last_turn_error {
                    TurnStatus::Error
                } else {
                    TurnStatus::Success
                },
                stop_reason: self.last_stop_reason.clone(),
                final_text: self.last_text.clone(),
                usage: self.last_usage.clone(),
                duration_ms: None,
                head: None,
            }],
            AgentRpcEvent::MessageStart { message } => {
                if role(&message) == Some("assistant") {
                    self.start_message();
                }
                Vec::new()
            }
            AgentRpcEvent::MessageUpdate {
                usage,
                assistant_message_event,
            } => {
                let mut out = Vec::new();
                let mapped_usage = map_usage(&usage, None);
                if !mapped_usage.is_empty() {
                    out.push(AgentEventPayload::UsageUpdate(mapped_usage));
                }
                if let Some(payload) = self.map_delta(assistant_message_event) {
                    out.push(payload);
                }
                out
            }
            AgentRpcEvent::TurnStart => vec![AgentEventPayload::ModelRequestStarted],
            AgentRpcEvent::Response {
                command,
                success,
                data,
                ..
            } if success && command == "get_session_stats" => map_session_stats(data.as_ref())
                .map(AgentEventPayload::UsageUpdate)
                .into_iter()
                .collect(),
            AgentRpcEvent::MessageEnd { message } => self.map_message_end(message),
            AgentRpcEvent::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
            } => vec![AgentEventPayload::ToolCallStarted {
                call_id: tool_call_id,
                name: tool_name.clone(),
                tool_type: tool_type(&tool_name),
                input: normalize_input(args),
                raw_input: None,
                title: None,
            }],
            AgentRpcEvent::ToolExecutionEnd {
                tool_call_id,
                tool_name: _,
                result,
                is_error,
            } => vec![AgentEventPayload::ToolCallCompleted {
                call_id: tool_call_id,
                result: map_tool_result(result, is_error),
            }],
            AgentRpcEvent::CompactionStart { .. } => {
                vec![AgentEventPayload::ContextCompactionStarted]
            }
            AgentRpcEvent::CompactionEnd {
                reason,
                result,
                aborted: _,
                will_retry: _,
                error_message: _,
            } => {
                let (pre_tokens, post_tokens) = result
                    .as_ref()
                    .map(|value| {
                        (
                            value.get("tokensBefore").and_then(Value::as_u64),
                            value.get("estimatedTokensAfter").and_then(Value::as_u64),
                        )
                    })
                    .unwrap_or((None, None));
                vec![AgentEventPayload::ContextCompacted {
                    trigger: Some(reason),
                    pre_tokens,
                    post_tokens,
                    duration_ms: None,
                }]
            }
            AgentRpcEvent::ExtensionUiRequest {
                id,
                method,
                title,
                message,
                options,
                placeholder,
                notify_type,
                ..
            } => self.map_extension_ui(
                id,
                method,
                title,
                message,
                options,
                placeholder,
                notify_type,
            ),
            AgentRpcEvent::ExtensionError {
                extension_path,
                event,
                error,
            } => vec![AgentEventPayload::Error {
                source: ErrorSource::Harness,
                message: format!("Lathe extension {extension_path} ({event}): {error}"),
                fatal: false,
            }],
            AgentRpcEvent::AutoRetryEnd {
                success: false,
                final_error,
                ..
            } => vec![AgentEventPayload::Error {
                source: ErrorSource::Harness,
                message: final_error.unwrap_or_else(|| "Lathe automatic retry failed.".into()),
                fatal: false,
            }],
            AgentRpcEvent::AgentEnd { .. }
            | AgentRpcEvent::TurnEnd { .. }
            | AgentRpcEvent::ToolExecutionUpdate { .. }
            | AgentRpcEvent::QueueUpdate { .. }
            | AgentRpcEvent::EntryAppended { .. }
            | AgentRpcEvent::SessionInfoChanged { .. }
            | AgentRpcEvent::ThinkingLevelChanged { .. }
            | AgentRpcEvent::AutoRetryStart { .. }
            | AgentRpcEvent::AutoRetryEnd { success: true, .. }
            | AgentRpcEvent::BashExecutionUpdate { .. }
            | AgentRpcEvent::Response { .. } => Vec::new(),
            AgentRpcEvent::Unrecognized => vec![AgentEventPayload::Unknown {
                harness_type: "dray".into(),
            }],
        };

        Ok(payloads
            .into_iter()
            .map(|payload| self.build(payload))
            .collect())
    }

    fn build(&self, payload: AgentEventPayload) -> AgentEvent {
        AgentEvent {
            id: Uuid::now_v7().to_string(),
            session_id: self.session_id.clone(),
            harness: Harness::Dray,
            seq: self.seq.fetch_add(1, Relaxed),
            ts: now_rfc3339(),
            turn_id: None,
            payload,
            raw: None,
        }
    }

    fn start_message(&mut self) -> String {
        let id = format!("dray-message-{}", self.next_message_id);
        self.next_message_id += 1;
        self.current_message_id = Some(id.clone());
        id
    }

    fn current_message(&mut self) -> String {
        self.current_message_id
            .clone()
            .unwrap_or_else(|| self.start_message())
    }

    fn block(&mut self, index: u32) -> BlockRef {
        BlockRef {
            message_id: self.current_message(),
            index,
        }
    }

    fn map_delta(&mut self, event: AssistantMessageEvent) -> Option<AgentEventPayload> {
        match event {
            AssistantMessageEvent::TextStart { content_index } => {
                let block = self.block(content_index);
                self.streamed_blocks
                    .insert((block.message_id.clone(), block.index));
                Some(AgentEventPayload::Delta(DeltaEvent::BlockStart {
                    block,
                    block_type: BlockType::Text,
                }))
            }
            AssistantMessageEvent::TextDelta {
                content_index,
                delta,
            } => Some(AgentEventPayload::Delta(DeltaEvent::TextDelta {
                block: self.block(content_index),
                text: delta,
            })),
            AssistantMessageEvent::TextEnd { content_index, .. } => {
                Some(AgentEventPayload::Delta(DeltaEvent::BlockStop {
                    block: self.block(content_index),
                }))
            }
            AssistantMessageEvent::ThinkingStart { content_index } => {
                let block = self.block(content_index);
                self.streamed_blocks
                    .insert((block.message_id.clone(), block.index));
                Some(AgentEventPayload::Delta(DeltaEvent::BlockStart {
                    block,
                    block_type: BlockType::Thinking,
                }))
            }
            AssistantMessageEvent::ThinkingDelta {
                content_index,
                delta,
            } => Some(AgentEventPayload::Delta(DeltaEvent::TextDelta {
                block: self.block(content_index),
                text: delta,
            })),
            AssistantMessageEvent::ThinkingEnd { content_index, .. } => {
                Some(AgentEventPayload::Delta(DeltaEvent::BlockStop {
                    block: self.block(content_index),
                }))
            }
            AssistantMessageEvent::ToolcallStart {
                content_index,
                id,
                tool_name,
            } => {
                let (Some(id), Some(name)) = (id, tool_name) else {
                    return None;
                };
                let block = self.block(content_index);
                self.streamed_blocks
                    .insert((block.message_id.clone(), block.index));
                Some(AgentEventPayload::Delta(DeltaEvent::BlockStart {
                    block,
                    block_type: BlockType::ToolUse { id, name },
                }))
            }
            AssistantMessageEvent::ToolcallDelta {
                content_index,
                delta,
            } => Some(AgentEventPayload::Delta(DeltaEvent::InputDelta {
                block: self.block(content_index),
                partial_json: delta,
            })),
            AssistantMessageEvent::ToolcallEnd { content_index, .. } => {
                Some(AgentEventPayload::Delta(DeltaEvent::BlockStop {
                    block: self.block(content_index),
                }))
            }
            AssistantMessageEvent::Start
            | AssistantMessageEvent::Done { .. }
            | AssistantMessageEvent::Error { .. }
            | AssistantMessageEvent::Unrecognized => None,
        }
    }

    fn map_extension_ui(
        &mut self,
        id: String,
        method: String,
        title: Option<String>,
        message: Option<String>,
        options: Vec<String>,
        placeholder: Option<String>,
        notify_type: Option<String>,
    ) -> Vec<AgentEventPayload> {
        if method == "notify" {
            return vec![AgentEventPayload::ExtensionNotification {
                message: message.or(title).unwrap_or_default(),
                level: notify_type.unwrap_or_else(|| "info".into()),
            }];
        }
        if !matches!(method.as_str(), "select" | "confirm" | "input" | "editor") {
            return Vec::new();
        }

        let question = match method.as_str() {
            "select" => title
                .or(message)
                .unwrap_or_else(|| "Choose an option".into()),
            "confirm" => message
                .or(title)
                .unwrap_or_else(|| "Confirm this action".into()),
            "input" | "editor" => title
                .or(message)
                .or(placeholder)
                .unwrap_or_else(|| "Enter a value".into()),
            _ => unreachable!(),
        };
        let choices = match method.as_str() {
            "select" => options,
            "confirm" => vec!["Yes".into(), "No".into()],
            _ => Vec::new(),
        };

        self.pending_ui
            .lock()
            .expect("Lathe UI request mutex poisoned")
            .insert(
                id.clone(),
                PendingUiRequest {
                    id: id.clone(),
                    method,
                    question: question.clone(),
                },
            );

        let tool_use_id = format!("dray-ui-{id}");
        vec![AgentEventPayload::QuestionsAsked {
            request_id: id,
            tool_use_id,
            questions: vec![Question {
                question,
                header: None,
                multi_select: false,
                options: choices
                    .into_iter()
                    .map(|label| QuestionOption {
                        label,
                        description: None,
                        preview: None,
                    })
                    .collect(),
            }],
        }]
    }

    fn map_message_end(&mut self, message: Value) -> Vec<AgentEventPayload> {
        if role(&message) != Some("assistant") {
            return Vec::new();
        }

        let message_id = self.current_message();
        let usage = message
            .get("usage")
            .and_then(|value| serde_json::from_value::<AgentRpcUsage>(value.clone()).ok())
            .map(|usage| map_usage(&usage, message.get("model").and_then(Value::as_str)));
        let text = content_text(message.get("content"));
        if !text.trim().is_empty() {
            self.last_text = Some(text);
        }
        self.last_usage = usage;
        self.last_stop_reason = message
            .get("stopReason")
            .and_then(Value::as_str)
            .map(str::to_string);
        self.last_turn_error =
            matches!(self.last_stop_reason.as_deref(), Some("error" | "aborted"));

        let Some(content) = message.get("content").and_then(Value::as_array) else {
            return Vec::new();
        };

        content
            .iter()
            .enumerate()
            .filter_map(|(index, block)| {
                let index = u32::try_from(index).ok()?;
                let block_ref = BlockRef {
                    message_id: message_id.clone(),
                    index,
                };
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        let text = block
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        // Tool-only responses reserve index zero for streamed text.
                        // An empty placeholder is not assistant output or a tool-group boundary.
                        if text.trim().is_empty() {
                            return None;
                        }
                        Some(AgentEventPayload::AssistantText {
                            block: self
                                .streamed_blocks
                                .contains(&(message_id.clone(), index))
                                .then_some(block_ref),
                            text: text.to_string(),
                        })
                    }
                    Some("thinking") => Some(AgentEventPayload::Reasoning {
                        block: self
                            .streamed_blocks
                            .contains(&(message_id.clone(), index))
                            .then_some(block_ref),
                        text: block
                            .get("thinking")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        encrypted: block
                            .get("redacted")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    }),
                    // The execution event is authoritative for tool arguments:
                    // extensions can mutate them in their `tool_call` hook after
                    // the assistant message has already been emitted.
                    Some("toolCall") | Some("image") | _ => None,
                }
            })
            .collect()
    }
}

fn role(message: &Value) -> Option<&str> {
    message.get("role").and_then(Value::as_str)
}

fn normalize_input(value: Value) -> Value {
    match value {
        Value::Object(_) => value,
        Value::String(raw) => json!({"_unparsed": raw}),
        other => json!({"_unparsed": other}),
    }
}

fn map_usage(wire: &AgentRpcUsage, model: Option<&str>) -> Usage {
    Usage {
        input_tokens: wire.input,
        output_tokens: wire.output,
        cached_input_tokens: wire.cache_read,
        cache_write_tokens: wire.cache_write,
        reasoning_tokens: wire.reasoning,
        total_tokens: wire.total_tokens,
        cost_usd: wire.cost.as_ref().and_then(|cost| cost.total),
        context_window: None,
        rate_limit: None,
        model: model.map(str::to_string),
        per_model: Vec::new(),
    }
}

/// Maps Lathe's `get_session_stats` response to the current conversation context.
/// This is separate from provider usage: `message_end.usage.totalTokens` is
/// cumulative model billing, while `contextUsage.tokens` includes the actual
/// messages currently retained in the context after tools and compaction.
fn map_session_stats(data: Option<&Value>) -> Option<Usage> {
    let context = data?.get("contextUsage")?;
    let used_tokens = context.get("tokens")?.as_u64()?;
    let max_tokens = context.get("contextWindow")?.as_u64()?;

    Some(Usage {
        context_window: Some(ContextWindow {
            used_tokens,
            max_tokens,
        }),
        ..Usage::default()
    })
}

fn content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| {
                (block.get("type").and_then(Value::as_str) == Some("text"))
                    .then(|| block.get("text").and_then(Value::as_str))
                    .flatten()
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

fn map_tool_result(mut result: Value, is_error: bool) -> ToolResult {
    let text = content_text(result.get("content"));
    let images = result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|block| {
            if block.get("type").and_then(Value::as_str) != Some("image") {
                return None;
            }
            let data = block.get("data").and_then(Value::as_str)?;
            let mime = block
                .get("mimeType")
                .and_then(Value::as_str)
                .unwrap_or("image/png");
            Some(crate::events::ImageRef {
                path: None,
                url: Some(format!("data:{mime};base64,{data}")),
                mime_type: Some(mime.to_string()),
            })
        })
        .collect();

    strip_image_data(&mut result);
    let exit_code = result
        .get("exitCode")
        .or_else(|| {
            result
                .get("details")
                .and_then(|value| value.get("exitCode"))
        })
        .and_then(Value::as_i64)
        .and_then(|code| i32::try_from(code).ok());
    let duration_ms = result
        .get("durationMs")
        .or_else(|| {
            result
                .get("details")
                .and_then(|value| value.get("durationMs"))
        })
        .and_then(Value::as_u64);
    let result_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    ToolResult {
        text,
        is_error: is_error || result_error,
        structured: (!result.is_null()).then_some(result),
        exit_code,
        duration_ms,
        images,
    }
}

fn strip_image_data(value: &mut Value) {
    let Some(content) = value.get_mut("content").and_then(Value::as_array_mut) else {
        return;
    };

    for block in content {
        if block.get("type").and_then(Value::as_str) == Some("image") {
            if let Some(object) = block.as_object_mut() {
                object.remove("data");
            }
        }
    }
}

/// Classifies Lathe's built-in tools while leaving extension names generic.
fn tool_type(name: &str) -> ToolType {
    match name {
        "bash" | "background_command" => ToolType::Shell,
        "read" => ToolType::FileRead,
        "write" | "edit" => ToolType::FileEdit,
        "web_search" => ToolType::Web,
        _ => ToolType::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::dray::parser;

    fn map_lines(lines: &[&str]) -> Vec<AgentEvent> {
        let mut mapper = Mapper::default();
        lines
            .iter()
            .flat_map(|line| mapper.map(parser::parse_line(line).unwrap()).unwrap())
            .collect()
    }

    #[test]
    fn maps_an_extension_tool_without_special_casing_its_schema() {
        let events = map_lines(&[
            r#"{"type":"agent_start"}"#,
            r#"{"type":"tool_execution_start","toolCallId":"call-1","toolName":"finder","args":{"query":"find the mapper"}}"#,
            r#"{"type":"tool_execution_end","toolCallId":"call-1","toolName":"finder","result":{"content":[{"type":"text","text":"found"}],"details":{"current":"Finder explored"}},"isError":false}"#,
            r#"{"type":"agent_settled"}"#,
        ]);

        assert!(matches!(
            &events[1].payload,
            AgentEventPayload::ToolCallStarted { call_id, name, input, tool_type, .. }
                if call_id == "call-1"
                    && name == "finder"
                    && *tool_type == ToolType::Other
                    && input["query"] == "find the mapper"
        ));
        assert!(matches!(
            &events[2].payload,
            AgentEventPayload::ToolCallCompleted { call_id, result }
                if call_id == "call-1" && result.text == "found" && !result.is_error
        ));
        assert!(matches!(
            events.last().map(|event| &event.payload),
            Some(AgentEventPayload::TurnCompleted {
                status: TurnStatus::Success,
                ..
            })
        ));
    }

    #[test]
    fn maps_session_stats_to_context_usage() {
        let events = map_lines(&[
            r#"{"id":"stats","type":"response","command":"get_session_stats","success":true,"data":{"contextUsage":{"tokens":12345,"contextWindow":272000,"percent":4.5}}}"#,
        ]);

        assert!(matches!(
            events.first().map(|event| &event.payload),
            Some(AgentEventPayload::UsageUpdate(usage))
                if usage.context_window == Some(ContextWindow {
                    used_tokens: 12345,
                    max_tokens: 272000,
                })
        ));
    }

    #[test]
    fn ignores_unsuccessful_session_stats_responses() {
        let events = map_lines(&[
            r#"{"id":"stats","type":"response","command":"get_session_stats","success":false}"#,
            r#"{"id":"other","type":"response","command":"get_state","success":true,"data":{}}"#,
        ]);

        assert!(events.is_empty());
    }

    #[test]
    fn maps_dray_streams_and_joins_the_committed_message() {
        let events = map_lines(&[
            r#"{"type":"agent_start"}"#,
            r#"{"type":"message_start","message":{"role":"assistant","content":[]}}"#,
            r#"{"type":"message_update","usage":{"input":10,"output":2,"reasoning":1,"totalTokens":12,"cost":{"total":0.25}},"assistantMessageEvent":{"type":"text_start","contentIndex":0}}"#,
            r#"{"type":"message_update","usage":{"input":10,"output":3,"reasoning":2,"totalTokens":13},"assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"hello"}}"#,
            r#"{"type":"message_update","usage":{"input":10,"output":3,"reasoning":2,"totalTokens":13},"assistantMessageEvent":{"type":"text_end","contentIndex":0,"content":"hello"}}"#,
            r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"hello"}],"model":"gpt-test","usage":{"input":10,"output":3,"reasoning":2,"totalTokens":13,"cost":{"total":0.25}},"stopReason":"stop"}}"#,
            r#"{"type":"agent_settled"}"#,
        ]);

        assert!(events.iter().any(|event| matches!(
            &event.payload,
            AgentEventPayload::Delta(DeltaEvent::TextDelta { text, .. }) if text == "hello"
        )));
        assert!(events.iter().any(|event| matches!(
            &event.payload,
            AgentEventPayload::AssistantText { block: Some(_), text } if text == "hello"
        )));
        assert!(events.iter().any(|event| matches!(
            &event.payload,
            AgentEventPayload::TurnCompleted { usage: Some(usage), final_text: Some(text), .. }
                if usage.cost_usd == Some(0.25) && text == "hello"
        )));
    }

    #[test]
    fn tool_only_messages_do_not_emit_assistant_text_boundaries() {
        let events = map_lines(&[
            r#"{"type":"message_start","message":{"role":"assistant"}}"#,
            r#"{"type":"message_update","assistantMessageEvent":{"type":"thinking_start","contentIndex":1}}"#,
            r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":""},{"type":"thinking","thinking":"Inspect next file"}],"stopReason":"toolUse"}}"#,
            r#"{"type":"tool_execution_start","toolCallId":"call-1","toolName":"read","args":{"path":"a.rs"}}"#,
            r#"{"type":"tool_execution_end","toolCallId":"call-1","toolName":"read","result":{"content":[]},"isError":false}"#,
            r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"  "}],"stopReason":"toolUse"}}"#,
            r#"{"type":"tool_execution_start","toolCallId":"call-2","toolName":"bash","args":{"command":"git status"}}"#,
        ]);
        assert!(!events
            .iter()
            .any(|event| matches!(event.payload, AgentEventPayload::AssistantText { .. })));
        assert!(events.iter().any(|event| matches!(&event.payload,
                AgentEventPayload::Reasoning { block: Some(block), .. } if block.index == 1)));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.payload, AgentEventPayload::ToolCallStarted { .. }))
                .count(),
            2
        );
    }

    #[test]
    fn maps_dray_edit_arguments_to_the_existing_file_edit_renderer() {
        let events = map_lines(&[
            r#"{"type":"tool_execution_start","toolCallId":"call-1","toolName":"edit","args":{"path":"src/lib.rs","edits":[{"oldText":"old","newText":"new"}]}}"#,
        ]);

        assert!(matches!(
            &events[0].payload,
            AgentEventPayload::ToolCallStarted { tool_type: ToolType::FileEdit, input, .. }
                if input["path"] == "src/lib.rs" && input["edits"][0]["oldText"] == "old"
        ));
    }

    #[test]
    fn maps_images_without_persisting_their_base64_in_structured_results() {
        let events = map_lines(&[
            r#"{"type":"tool_execution_end","toolCallId":"call-1","toolName":"read","result":{"content":[{"type":"image","data":"aGVsbG8=","mimeType":"image/png"}]},"isError":false}"#,
        ]);

        let AgentEventPayload::ToolCallCompleted { result, .. } = &events[0].payload else {
            panic!("expected a tool result");
        };
        assert_eq!(result.images.len(), 1);
        assert_eq!(result.text, "");
        assert!(result.structured.as_ref().unwrap()["content"][0]
            .get("data")
            .is_none());
    }

    #[test]
    fn maps_extension_notifications_without_treating_them_as_errors() {
        let events = map_lines(&[
            r#"{"type":"extension_ui_request","id":"notice-1","method":"notify","message":"Lathe extension is ready","notifyType":"info"}"#,
        ]);

        assert!(matches!(
            &events[0].payload,
            AgentEventPayload::ExtensionNotification { message, level }
                if message == "Lathe extension is ready" && level == "info"
        ));
    }

    #[test]
    fn maps_extension_ui_to_the_shared_questionnaire_response_path() {
        let pending = PendingUiRequests::default();
        let mut mapper =
            Mapper::with_seq_and_ui("session", "", Arc::new(AtomicU64::new(0)), pending.clone());
        let events = mapper
            .map(
                parser::parse_line(
                    r#"{"type":"extension_ui_request","id":"ui-1","method":"select","title":"Choose a backend","options":["local","remote"]}"#,
                )
                .unwrap(),
            )
            .unwrap();

        assert!(matches!(
            &events[0].payload,
            AgentEventPayload::QuestionsAsked { request_id, questions, .. }
                if request_id == "ui-1"
                    && questions[0].question == "Choose a backend"
                    && questions[0].options[1].label == "remote"
        ));

        let mut answers = HashMap::new();
        answers.insert("Choose a backend".to_string(), "remote".to_string());
        let response = pending
            .lock()
            .unwrap()
            .get("ui-1")
            .unwrap()
            .response(&answers);
        assert_eq!(response["value"], "remote");
        assert_eq!(response["id"], "ui-1");
    }

    #[test]
    fn classifies_dray_builtins() {
        assert_eq!(tool_type("read"), ToolType::FileRead);
        assert_eq!(tool_type("edit"), ToolType::FileEdit);
        assert_eq!(tool_type("background_command"), ToolType::Shell);
        assert_eq!(tool_type("web_search"), ToolType::Web);
        assert_eq!(tool_type("custom_extension_tool"), ToolType::Other);
    }
}
