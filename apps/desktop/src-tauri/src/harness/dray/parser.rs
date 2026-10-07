use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A Lathe RPC event. The protocol is intentionally modeled with `Value` for
/// message and tool payloads because extensions can register arbitrary schemas.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentRpcEvent {
    RequestUsage {
        usage: AgentRpcUsage,
        model: String,
    },
    RequestError {
        status: u16,
        code: String,
        message: String,
        #[serde(rename = "requestId")]
        request_id: String,
    },
    Session {
        version: u32,
        id: String,
        timestamp: String,
        cwd: String,
        #[serde(default, rename = "parentSession")]
        parent_session: Option<String>,
    },
    AgentStart,
    AgentEnd {
        #[serde(default)]
        messages: Vec<Value>,
        #[serde(default, rename = "willRetry")]
        will_retry: bool,
    },
    AgentSettled,
    TurnStart,
    TurnEnd {
        #[serde(default)]
        message: Value,
        #[serde(default, rename = "toolResults")]
        tool_results: Vec<Value>,
    },
    MessageStart {
        message: Value,
    },
    MessageUpdate {
        #[serde(default)]
        usage: AgentRpcUsage,
        #[serde(rename = "assistantMessageEvent")]
        assistant_message_event: AssistantMessageEvent,
    },
    MessageEnd {
        message: Value,
    },
    ToolExecutionStart {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        args: Value,
    },
    ToolExecutionUpdate {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        args: Value,
        #[serde(default, rename = "partialResult")]
        partial_result: Value,
    },
    ToolExecutionEnd {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        #[serde(default)]
        result: Value,
        #[serde(default, rename = "isError")]
        is_error: bool,
    },
    QueueUpdate {
        #[serde(default)]
        steering: Vec<String>,
        #[serde(default, rename = "followUp")]
        follow_up: Vec<String>,
    },
    CompactionStart {
        reason: String,
    },
    CompactionEnd {
        reason: String,
        #[serde(default)]
        result: Option<Value>,
        #[serde(default)]
        aborted: bool,
        #[serde(default, rename = "willRetry")]
        will_retry: bool,
        #[serde(default, rename = "errorMessage")]
        error_message: Option<String>,
    },
    EntryAppended {
        entry: Value,
    },
    SessionInfoChanged {
        #[serde(default)]
        name: Option<String>,
    },
    ThinkingLevelChanged {
        level: String,
    },
    AutoRetryStart {
        attempt: u32,
        #[serde(rename = "maxAttempts")]
        max_attempts: u32,
        #[serde(rename = "delayMs")]
        delay_ms: u64,
        #[serde(rename = "errorMessage")]
        error_message: String,
    },
    AutoRetryEnd {
        success: bool,
        attempt: u32,
        #[serde(default, rename = "finalError")]
        final_error: Option<String>,
    },
    BashExecutionUpdate {
        #[serde(default)]
        id: Option<String>,
        delta: String,
    },
    ExtensionUiRequest {
        id: String,
        method: String,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        message: Option<String>,
        #[serde(default)]
        options: Vec<String>,
        #[serde(default)]
        placeholder: Option<String>,
        #[serde(default)]
        prefill: Option<String>,
        #[serde(default, rename = "notifyType")]
        notify_type: Option<String>,
        #[serde(default, rename = "statusKey")]
        status_key: Option<String>,
        #[serde(default, rename = "statusText")]
        status_text: Option<String>,
        #[serde(default, rename = "widgetKey")]
        widget_key: Option<String>,
        #[serde(default, rename = "widgetLines")]
        widget_lines: Option<Vec<String>>,
        #[serde(default, rename = "widgetPlacement")]
        widget_placement: Option<String>,
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        text: Option<String>,
    },
    ExtensionError {
        #[serde(rename = "extensionPath")]
        extension_path: String,
        event: String,
        error: String,
    },
    Response {
        #[serde(default)]
        id: Option<String>,
        command: String,
        success: bool,
        #[serde(default)]
        data: Option<Value>,
        #[serde(default)]
        error: Option<String>,
    },
    #[serde(other)]
    Unrecognized,
}

/// Usage attached to Lathe's streaming assistant updates and final messages.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentRpcUsage {
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_write: Option<u64>,
    pub reasoning: Option<u64>,
    pub total_tokens: Option<u64>,
    pub cost: Option<AgentRpcCost>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentRpcCost {
    pub input: Option<f64>,
    pub output: Option<f64>,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
    pub total: Option<f64>,
}

/// Delta events nested inside Lathe's `message_update` record.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantMessageEvent {
    Start,
    TextStart {
        #[serde(rename = "contentIndex")]
        content_index: u32,
    },
    TextDelta {
        #[serde(rename = "contentIndex")]
        content_index: u32,
        delta: String,
    },
    TextEnd {
        #[serde(rename = "contentIndex")]
        content_index: u32,
        #[serde(default)]
        content: String,
    },
    ThinkingStart {
        #[serde(rename = "contentIndex")]
        content_index: u32,
    },
    ThinkingDelta {
        #[serde(rename = "contentIndex")]
        content_index: u32,
        delta: String,
    },
    ThinkingEnd {
        #[serde(rename = "contentIndex")]
        content_index: u32,
        #[serde(default)]
        content: String,
    },
    ToolcallStart {
        #[serde(rename = "contentIndex")]
        content_index: u32,
        #[serde(default)]
        id: Option<String>,
        #[serde(default, rename = "toolName")]
        tool_name: Option<String>,
    },
    ToolcallDelta {
        #[serde(rename = "contentIndex")]
        content_index: u32,
        delta: String,
    },
    ToolcallEnd {
        #[serde(rename = "contentIndex")]
        content_index: u32,
        #[serde(default, rename = "toolCall")]
        tool_call: Value,
    },
    Done {
        reason: String,
        message: Value,
    },
    Error {
        reason: String,
        error: Value,
    },
    #[serde(other)]
    Unrecognized,
}

/// Parses one newline-delimited Lathe RPC record.
pub fn parse_line(line: &str) -> Result<AgentRpcEvent> {
    Ok(serde_json::from_str(line)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_extension_tool_lifecycle() {
        let start = parse_line(
            r#"{"type":"tool_execution_start","toolCallId":"call-1","toolName":"finder","args":{"query":"find the event pipeline"}}"#,
        )
        .unwrap();
        assert!(matches!(
            start,
            AgentRpcEvent::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args
            } if tool_call_id == "call-1"
                && tool_name == "finder"
                && args["query"] == "find the event pipeline"
        ));

        let end = parse_line(
            r#"{"type":"tool_execution_end","toolCallId":"call-1","toolName":"finder","result":{"content":[{"type":"text","text":"found it"}],"details":{"current":"Finder explored"}},"isError":false}"#,
        )
        .unwrap();
        assert!(matches!(
            end,
            AgentRpcEvent::ToolExecutionEnd { is_error: false, result, .. }
                if result["content"][0]["text"] == "found it"
        ));
    }

    #[test]
    fn parses_streaming_tool_call_updates() {
        let event = parse_line(
            r#"{"type":"message_update","usage":{"input":12,"output":3,"reasoning":2,"totalTokens":15,"cost":{"total":0.1}},"assistantMessageEvent":{"type":"toolcall_start","contentIndex":1,"id":"call-1","toolName":"background_command"}}"#,
        )
        .unwrap();

        assert!(matches!(
            event,
            AgentRpcEvent::MessageUpdate {
                usage: AgentRpcUsage { input: Some(12), reasoning: Some(2), .. },
                assistant_message_event: AssistantMessageEvent::ToolcallStart {
                    content_index: 1,
                    id: Some(id),
                    tool_name: Some(name),
                }
            } if id == "call-1" && name == "background_command"
        ));
    }

    #[test]
    fn unknown_records_do_not_break_the_stream() {
        assert!(matches!(
            parse_line(r#"{"type":"future_dray_event","value":42}"#).unwrap(),
            AgentRpcEvent::Unrecognized
        ));
    }

    #[test]
    fn parses_extension_ui_requests_without_assuming_the_payload() {
        let event = parse_line(
            r#"{"type":"extension_ui_request","id":"ui-1","method":"select","title":"Pick one","options":["a","b"]}"#,
        )
        .unwrap();

        assert!(matches!(
            event,
            AgentRpcEvent::ExtensionUiRequest { id, method, options, .. }
                if id == "ui-1" && method == "select" && options == ["a", "b"]
        ));
    }
}
