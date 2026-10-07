//! Normalized, harness-agnostic event model.
//!
//! Every harness parses its own wire format, then maps it onto [`AgentEvent`].
//! The frontend, the on-disk log, and the session index only see this
//! vocabulary, so adding a harness means writing one mapper.
//!
//! # Log evolution rules
//!
//! Persisted `events.jsonl` outlives any single build, so:
//!
//! 1. Never remove, rename, or retype a shipped field — add alongside instead.
//! 2. Every field added from here on is `Option<T>` or `#[serde(default)]`, so
//!    new code reads old lines.
//! 3. Readers skip lines they cannot parse. Unknown payload kinds don't need
//!    that; they land in [`AgentEventPayload::Unrecognized`].

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

pub mod usage;

pub use usage::{ContextWindow, ModelUsage, RateLimit, Usage};

// `Harness` is a harness concept, not an event one; it lives in `crate::harness`
// and is used here only as a field type.
use crate::harness::Harness;

/// One normalized event: an envelope (who, when, what order, which conversation)
/// wrapping a [`payload`](Self::payload) (what happened).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "events.ts")]
#[serde(rename_all = "camelCase")]
pub struct AgentEvent {
    pub id: String,
    pub session_id: String,
    pub harness: Harness,
    /// Position in the session's event log, and the cursor for reconnecting a UI
    /// to a running session. One counter per session, shared by mapped stdout
    /// lines and events the app synthesizes itself, seeded from the persisted log
    /// on resume. Never sort by `ts` — most Lathe events omit it.
    pub seq: u64,
    pub ts: String,
    pub turn_id: Option<String>,
    pub payload: AgentEventPayload,
    /// `None` on the emitted path — raw lines are archived separately — but
    /// always populated for [`AgentEventPayload::Unknown`], which is useless
    /// without it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

/// What happened.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "events.ts")]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum AgentEventPayload {
    // ---------- session / turn lifecycle ----------
    /// Lathe emits one `init` per turn, not per session — the tool list
    /// grows between them as deferred tools load — so this carries whatever the
    /// turn was configured with. The first of a session is the session's.
    ///
    /// A turn is not the same as a prompt: the agent can open one for itself
    /// when continuing work.
    TurnStarted(SessionInfo),
    /// Not a session terminator — one arrives per completed turn.
    TurnCompleted {
        status: TurnStatus,
        stop_reason: Option<String>,
        final_text: Option<String>,
        usage: Option<Usage>,
        duration_ms: Option<u64>,
        /// The working tree as this turn finished, as a git tree id — the
        /// "after" side the changes panel diffs against once the turn is over.
        ///
        /// Without it the panel's head is always *now*, so an idle session
        /// keeps absorbing whatever later touches the same checkout — another
        /// session's turns, the user's editor — and describes them as this
        /// turn's work. Freezing the head here bounds the diff to the turn.
        ///
        /// Filled by the session layer, not the mapper: only it knows the
        /// session's cwd. `None` for a non-repo and for turns logged before
        /// the field existed, which the panel reads as "diff against now".
        #[serde(default)]
        head: Option<String>,
    },
    SettingsChanged(Settings),

    // ---------- conversation ----------
    UserMessage {
        text: String,
        #[serde(default)]
        images: Vec<ImageRef>,
        /// The working tree as it stood when this prompt was sent, as a git
        /// tree id — the "before" side the changes panel diffs against.
        ///
        /// Taken here rather than derived from the turn's own tool calls
        /// because those miss everything `Bash` does, and because an `Edit`
        /// carries only the fragment it replaced. A snapshot compares content,
        /// so several edits to one file — and any commit made mid-turn —
        /// collapse into the one net diff.
        ///
        /// `None` for a directory that isn't a repo, and for every prompt
        /// logged before this field existed. Both mean the same thing to the
        /// panel: nothing to show.
        #[serde(default)]
        baseline: Option<String>,
        /// Typed while a turn was already running, so the CLI folds it into that
        /// turn rather than starting a new one.
        ///
        /// Two transcript rules read this and both invert on it: a queued prompt
        /// does not abandon the tool calls open before it — it is not proof the
        /// turn stopped — and it does not cut a new turn, because the CLI answers
        /// it inside the running one and emits a single `result` for both.
        #[serde(default)]
        queued: bool,
        /// The Lathe session that relayed this prompt, when one did.
        ///
        /// `None` — the ordinary case — means the user typed it. Carried as a
        /// field rather than named in `text` because the transcript draws the
        /// sender, and anything drawn from prose the model can write itself is
        /// a thing the model can forge.
        #[serde(default)]
        from: Option<MessageSender>,
    },
    AssistantText {
        /// `Some` only when this content was also streamed, naming the preview
        /// it supersedes. `None` — the common case — means nothing was streamed and the
        /// event simply appends in `seq` order.
        #[serde(default)]
        block: Option<BlockRef>,
        text: String,
    },
    /// `encrypted` records that a reasoning step happened but its content is
    /// unreadable, which is how Lathe reports reasoning it won't disclose.
    Reasoning {
        #[serde(default)]
        block: Option<BlockRef>,
        text: String,
        #[serde(default)]
        encrypted: bool,
    },

    // ---------- streaming ----------
    /// Incremental content, superseded by the committed event for the same
    /// [`BlockRef`]. See [`DeltaEvent`].
    Delta(DeltaEvent),

    // ---------- tools ----------
    ToolCallStarted {
        call_id: String,
        /// The harness's own tool name, verbatim (`"Bash"`, `"apply_patch"`).
        name: String,
        tool_type: ToolType,
        /// Always an object. JSON-encoded argument strings are parsed here;
        /// unparseable input becomes `{"_unparsed": "…"}` rather than dropped.
        input: Value,
        /// Input that isn't JSON at all — Lathe's `custom_tool_call.input` is raw
        /// JS source.
        raw_input: Option<String>,
        title: Option<String>,
    },
    ToolCallCompleted {
        call_id: String,
        result: ToolResult,
    },
    /// Structured file changes. Lathe reports these first-class; Lathe
    /// does not, so its edits currently surface as ordinary
    /// [`ToolType::FileEdit`] calls.
    FileEdits {
        call_id: Option<String>,
        #[serde(default)]
        edits: Vec<FileEdit>,
    },

    // ---------- accounting / control ----------
    /// Debounce these in the mapper: harnesses emit token counts far more often
    /// than the figures meaningfully change.
    UsageUpdate(Usage),
    /// The plan's usage limit, emitted **only when there is something to act
    /// on** — the limit is reached, or requests have moved to usage billing. A
    /// session running comfortably under its limit reports the fact constantly
    /// and produces none of these.
    ///
    /// The status vocabulary is only partly known (`allowed` is the one value
    /// captured), so the wire's own strings are carried through rather than
    /// collapsed into a boolean the mapper would have to guess at.
    RateLimited {
        /// `allowed` is the steady state and never reaches here.
        status: Option<String>,
        /// When the window rolls over, RFC3339 — converted from the unix
        /// seconds the wire sends.
        resets_at: Option<String>,
        /// Which window. `five_hour` observed, and at least one longer window
        /// is believed to exist; not branched on anywhere.
        limit_type: Option<String>,
        /// Whether overage is available, which is what separates "blocked
        /// until it resets" from "still working, now billed as usage".
        overage_status: Option<String>,
        /// Requests are already being billed as usage rather than covered.
        #[serde(default)]
        using_overage: bool,
        /// Why overage isn't available — `org_level_disabled` observed.
        overage_disabled_reason: Option<String>,
    },
    /// The agent asking the user something in its own words. The harness is
    /// blocked until the app replies, so a consumer that renders it must offer
    /// a form rather than treating it as informational.
    QuestionsAsked {
        request_id: String,
        /// The call being held. Its own row is already in the transcript and
        /// will show the answers once it completes.
        tool_use_id: String,
        /// One to four, per the tool's own schema.
        questions: Vec<Question>,
    },
    /// Minted by the app after answering a question so the transient question
    /// card is retired in the live transcript.
    QuestionAnswered {
        request_id: String,
        tool_use_id: String,
    },

    Hook {
        name: String,
        event: String,
        phase: HookPhase,
        exit_code: Option<i32>,
        outcome: Option<String>,
    },
    /// A fire-and-forget notification emitted by a Lathe extension through its
    /// host UI API. It is live-only because it has no meaning after the process
    /// that produced it is gone.
    ExtensionNotification {
        message: String,
        level: String,
    },
    /// The harness has sent a request to the model and is waiting on its first
    /// token. Drives the working indicator and nothing else.
    ///
    /// Fires at the top of a turn *and* after every tool result, which is what
    /// makes it worth carrying: the gap after a tool call is where the
    /// transcript otherwise sits blank, and this marks its start within 30ms.
    /// Measured from here to the first `content_block_start`: ~1s for a text
    /// block, a 3s median (1.7–7.5s) for a thinking one.
    ModelRequestStarted,
    /// A compaction is under way. Drives a live indicator and nothing else —
    /// the counts only exist once it finishes.
    ContextCompactionStarted,
    /// A compaction finished, and the transcript before it no longer reaches the
    /// model. Both counts are optional so an unfamiliar wire shape still closes
    /// the indicator; the UI drops the saving rather than reporting a wrong one.
    ContextCompacted {
        /// `manual` or `auto`.
        trigger: Option<String>,
        pre_tokens: Option<u64>,
        post_tokens: Option<u64>,
        duration_ms: Option<u64>,
    },
    Error {
        source: ErrorSource,
        message: String,
        #[serde(default)]
        fatal: bool,
    },
    /// A line we parsed but could not classify. Surfacing these beats silently
    /// dropping them.
    Unknown {
        harness_type: String,
    },

    /// A payload `kind` this build doesn't know — a log written by a newer
    /// version. Produced by the deserializer, never a mapper; the envelope
    /// survives so the event keeps its place. Distinct from
    /// [`Unknown`](Self::Unknown), a harness line the mapper couldn't classify.
    #[serde(other)]
    Unrecognized,
}

impl AgentEventPayload {
    /// Every archived picture this payload points at. Both arms hold paths under
    /// `~/.dray/attachments/<session-id>/`, so anything moving a log between
    /// sessions has to repoint them — see
    /// [`copy_session_log`](crate::store::copy_session_log).
    pub fn images_mut(&mut self) -> &mut [ImageRef] {
        match self {
            Self::UserMessage { images, .. } => images,
            Self::ToolCallCompleted { result, .. } => &mut result.images,
            _ => &mut [],
        }
    }
}

/// One question from a [`QuestionsAsked`](AgentEventPayload::QuestionsAsked).
///
/// [`question`](Self::question) is both the prompt and the key its answer is
/// filed under, so the text has to survive the round trip unchanged — the
/// harness matches on it verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "events.ts")]
#[serde(rename_all = "camelCase")]
pub struct Question {
    pub question: String,
    /// A short chip label for the question — "Indentation", "Auth method".
    pub header: Option<String>,
    /// Whether several options may be picked, in which case the answer is one
    /// comma-separated string rather than a list.
    pub multi_select: bool,
    /// Two to four, per the tool's own schema. Never exhaustive: the harness
    /// promises the user a free-text box alongside them, and instructs the model
    /// not to offer an "Other" option because of it — so a renderer that shows
    /// only these takes an answer away.
    pub options: Vec<QuestionOption>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "events.ts")]
#[serde(rename_all = "camelCase")]
pub struct QuestionOption {
    /// What the user picks, and what travels back as the answer — the harness
    /// has no option ids, so the label is the value.
    pub label: String,
    pub description: Option<String>,
    /// Markdown the harness expects shown in a monospace box. Single-select
    /// questions only.
    pub preview: Option<String>,
}

/// How a turn ended. Lathe reports this as `is_error` on its result
/// event; Lathe live emits `turn.completed` (a failed turn is uncaptured so
/// far). A user-abort outcome likely deserves its own variant once one has been
/// captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "events.ts")]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    Success,
    Error,
}

/// Joins streamed content to its committed counterpart. A message is often
/// `[text, tool_use, …]` and each block arrives as its own event; Lathe's
/// committed events carry no index, so the mapper derives one by counting blocks
/// per `message_id` in arrival order.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export, export_to = "events.ts")]
#[serde(rename_all = "camelCase")]
pub struct BlockRef {
    pub message_id: String,
    pub index: u32,
}

/// Incremental content for a block.
///
/// **Deltas are a preview, never the source of truth**: the committed event for
/// the same [`BlockRef`] supersedes whatever they accumulated. Absent deltas are
/// the common case — Lathe emits none — so
/// consumers must render correctly without them.
/// Tagged on `delta`, not `type`: [`AgentEventPayload::Delta`] is a newtype
/// variant, so these fields flatten into the payload object alongside its own
/// `type` tag. Two tags of the same name serialize but never deserialize.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "events.ts")]
#[serde(
    tag = "delta",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum DeltaEvent {
    BlockStart {
        block: BlockRef,
        block_type: BlockType,
    },
    /// Carries *thinking* text too — the shapes are identical and the block's
    /// [`BlockStart`](Self::BlockStart) already said which kind it is, so a
    /// second variant would duplicate that fact.
    TextDelta {
        block: BlockRef,
        text: String,
    },
    /// A fragment of a tool call's JSON arguments, unparseable until every
    /// fragment for the block has been concatenated.
    InputDelta {
        block: BlockRef,
        partial_json: String,
    },
    BlockStop {
        block: BlockRef,
    },
}

/// A tool call's identity rides here rather than on [`BlockRef`], which stays a
/// cheap map key. It arrives before any arguments have streamed, so the UI can
/// label the call while [`DeltaEvent::InputDelta`] fragments are still landing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "events.ts")]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum BlockType {
    Text,
    Thinking,
    ToolUse { id: String, name: String },
}

/// A rendering hint — which icon and component to use. Nothing depends on this
/// for correctness, and [`ToolType::Other`] must always render acceptably.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "events.ts")]
#[serde(rename_all = "snake_case")]
pub enum ToolType {
    Shell,
    FileRead,
    FileEdit,
    Search,
    Web,
    Mcp,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "events.ts")]
#[serde(rename_all = "camelCase")]
pub struct ToolResult {
    /// Result content flattened to text; harnesses vary between a bare string
    /// and an array of blocks.
    pub text: String,
    /// Harnesses routinely omit the error flag on success, so this defaults to
    /// `false` rather than being treated as unknown.
    #[serde(default)]
    pub is_error: bool,
    /// The full result payload when the harness supplies a structured one.
    pub structured: Option<Value>,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u64>,
    /// Pictures the tool handed back — a `Read` of a screenshot, an MCP tool
    /// that answers in images. Archived under `~/.dray/attachments` before the
    /// event is written, so this carries a path and never the bytes: the CLI
    /// sends the same image twice on one line and a session of screenshots was
    /// 12MB of base64 in a 14MB log.
    #[serde(default)]
    pub images: Vec<ImageRef>,
}

/// Who relayed a prompt into this session, for a `user_message` the user did
/// not type.
///
/// Both fields are needed and neither substitutes for the other: the title is
/// what the reader recognizes — it is what the sidebar shows — and the id is
/// what the transcript navigates to when they click it.
///
/// Persisted, unlike most of what the app synthesizes: the transcript is
/// replayed from the log, so attribution that lived only in memory would be
/// gone on the next open.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "events.ts")]
#[serde(rename_all = "camelCase")]
pub struct MessageSender {
    pub session_id: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "events.ts")]
#[serde(rename_all = "camelCase")]
pub struct FileEdit {
    pub path: String,
    pub change: FileChange,
    pub unified_diff: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "events.ts")]
#[serde(rename_all = "snake_case")]
pub enum FileChange {
    Add,
    Update,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "events.ts")]
#[serde(rename_all = "snake_case")]
pub enum HookPhase {
    Started,
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "events.ts")]
#[serde(rename_all = "snake_case")]
pub enum ErrorSource {
    /// OpenAI rejected or could not complete an inference request.
    Provider,
    /// The harness reported an error of its own.
    Harness,
    /// We failed to parse or map the line.
    Parser,
    /// The child process failed — spawn, stderr, unexpected exit.
    Process,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "events.ts")]
#[serde(rename_all = "camelCase")]
pub struct ImageRef {
    pub path: Option<String>,
    pub url: Option<String>,
    pub mime_type: Option<String>,
}

/// Session-level facts, known at startup.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "events.ts")]
#[serde(rename_all = "camelCase", default)]
pub struct SessionInfo {
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub harness_version: Option<String>,
    pub tools: Vec<String>,
    pub mcp_servers: Vec<McpServer>,
    pub settings: Option<Settings>,
}

/// Shared with the harness parsers rather than duplicated — the wire shape
/// matches, so they deserialize straight into this.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "events.ts")]
#[serde(rename_all = "camelCase")]
pub struct McpServer {
    pub name: String,
    /// Free-form: `connected`, `pending`, `needs-auth` observed, set undocumented.
    pub status: String,
}

/// Settings that can change mid-session, so they arrive as events rather than
/// living only on [`SessionInfo`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "events.ts")]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub model: Option<String>,
    pub sandbox: Option<String>,
    pub writable_roots: Vec<String>,
    pub network_access: Option<bool>,
    pub fast_mode: Option<String>,
}

/// Hand-rolled to avoid a date dependency for one display-only field; `seq`, not
/// `ts`, is the ordering key.
pub fn now_rfc3339() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();

    rfc3339(now.as_secs() as i64, now.subsec_millis())
}

/// Unix seconds → RFC3339, for wire fields carrying an epoch timestamp where
/// this model uses strings — Lathe's `resetsAt`, notably.
pub fn rfc3339_from_unix(secs: i64) -> String {
    rfc3339(secs, 0)
}

fn rfc3339(secs: i64, millis: u32) -> String {
    // Days since epoch → civil date, per Howard Hinnant's algorithm.
    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        y,
        m,
        d,
        secs_of_day / 3_600,
        (secs_of_day % 3_600) / 60,
        secs_of_day % 60,
        millis
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rule 2: new code reads old lines. Flags and collections deserialize when
    /// absent instead of failing the line.
    #[test]
    fn old_lines_without_defaulted_fields_still_parse() {
        let v: AgentEventPayload =
            serde_json::from_str(r#"{"type":"user_message","text":"hi"}"#).unwrap();
        assert!(matches!(
            v,
            // `baseline`, `queued` and `from` default too: every prompt logged
            // before each of those existed must not fail the line.
            AgentEventPayload::UserMessage { ref text, ref images, ref baseline, queued, ref from }
                if text == "hi" && images.is_empty() && baseline.is_none() && !queued
                    && from.is_none()
        ));

        let v: AgentEventPayload = serde_json::from_str(
            r#"{"type":"reasoning","block":{"messageId":"m","index":0},"text":"t"}"#,
        )
        .unwrap();
        assert!(matches!(
            v,
            AgentEventPayload::Reasoning {
                encrypted: false,
                ..
            }
        ));
    }

    /// Rule 1 corollary: old code reads new lines. Unknown fields are ignored,
    /// and an unknown payload kind degrades to `Unrecognized` instead of
    /// failing the whole line.
    #[test]
    fn new_lines_degrade_gracefully() {
        let v: AgentEventPayload = serde_json::from_str(
            r#"{"type":"turn_completed","status":"success","someFutureField":42}"#,
        )
        .unwrap();
        assert!(matches!(
            v,
            AgentEventPayload::TurnCompleted {
                status: TurnStatus::Success,
                ..
            }
        ));

        let v: AgentEventPayload =
            serde_json::from_str(r#"{"type":"from_the_future","payload":9001}"#).unwrap();
        assert!(matches!(v, AgentEventPayload::Unrecognized));
    }

    /// The nested tag-in-tag shape (`kind` outer, `type` inner) survives a
    /// round trip.
    #[test]
    fn delta_round_trips() {
        let d = AgentEventPayload::Delta(DeltaEvent::TextDelta {
            block: BlockRef {
                message_id: "m".into(),
                index: 0,
            },
            text: "he".into(),
        });
        let s = serde_json::to_string(&d).unwrap();
        assert!(s.contains(r#""type":"delta""#) && s.contains(r#""delta":"text_delta""#));
        let back: AgentEventPayload = serde_json::from_str(&s).unwrap();
        assert_eq!(s, serde_json::to_string(&back).unwrap());
    }
}
