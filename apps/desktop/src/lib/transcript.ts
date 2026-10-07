import type { AgentEvent, ToolResult } from "@/types/events";

/// Consecutive tool calls, possibly of different types, collapsed behind one
/// summary row. Assistant output ends a group; invisible lifecycle/result
/// events do not.
export type ToolGroup = {
  kind: "tool_group";
  /// The tool-call events, in `seq` order. Never fewer than two.
  calls: AgentEvent[];
  key: string;
};

/// Either a lone event or a collapsed run of tool calls.
export type WorkItem = AgentEvent | ToolGroup;

export type QuestionsAskedPayload = Extract<
  AgentEvent["payload"],
  { type: "questions_asked" }
>;

/// A questionnaire the agent is blocked on until the user answers.
export type PendingAsk = QuestionsAskedPayload;

export function isToolGroup(item: WorkItem): item is ToolGroup {
  return "kind" in item && item.kind === "tool_group";
}

export type Turn = {
  /// The user's prompt opening this turn, absent only for a transcript that
  /// starts mid-conversation.
  prompt: AgentEvent | null;
  /// Everything the agent did between the prompt and completion — tool calls,
  /// reasoning, and its intermediate messages. Consecutive
  /// tool calls arrive pre-collapsed into a `ToolGroup`.
  work: WorkItem[];
  /// The closing `turn_completed`, absent while the turn is still running.
  completed: AgentEvent | null;
  /// `turn_completed.finalText`, which is a verbatim copy of the turn's last
  /// `assistant_text` — so showing both would print the answer twice. A turn
  /// that closed without one (an interrupt) falls back to its last
  /// `assistant_text` directly, so a collapsed turn always ends on what the
  /// agent last said; still `null` when the turn produced no text at all.
  finalText: string | null;
  key: string;
};

/// Payload types that put something on screen — the complement of the
/// `return null` arms in [EventRow](../components/chat/EventRow.tsx). Keep the
/// two in step: this set decides what breaks a tool run and which existing row
/// a streaming tool preview may join.
const RENDERS = new Set([
  // Only ever reached by a *queued* prompt. An ordinary one is a turn's header
  // rather than its work, so it never enters `work` for this to be asked about.
  "user_message",
  "assistant_text",
  "tool_call_started",
  "file_edits",
  "error",
  "extension_notification",
  "context_compacted",
  "rate_limited",
]);

/// Whether an item draws a row. A group always does — it is built from tool
/// calls, which always draw. Exported so the live preview can locate the last
/// visible item without maintaining a second event-type list.
export function rendersWorkItem(item: WorkItem): boolean {
  return isToolGroup(item) || RENDERS.has(item.payload.type);
}

/// `seq` is the ordering key — most Lathe events carry no usable `ts`.
function bySeq(a: AgentEvent, b: AgentEvent) {
  return a.seq - b.seq;
}

type OpenTurn = Omit<Turn, "work"> & {
  /// Ungrouped while the turn is open; `groupTools` runs whenever the snapshot
  /// handed to the UI is built.
  work: AgentEvent[];
};

function groupableTool(event: AgentEvent): boolean {
  return event.payload.type === "tool_call_started";
}

/// Groups every uninterrupted run of two or more calls, regardless of type.
/// Assistant text and other visible rows end a run; result/lifecycle events are
/// transparent, so the normal started/completed/started sequence stays whole.
function groupTools(work: AgentEvent[]): WorkItem[] {
  const items: WorkItem[] = [];
  let run: AgentEvent[] = [];
  let held: AgentEvent[] = [];

  const flush = () => {
    if (run.length >= 2) {
      items.push({ kind: "tool_group", calls: run, key: `group-${run[0].id}` });
    } else {
      items.push(...run);
    }
    items.push(...held);
    run = [];
    held = [];
  };

  for (const event of work) {
    if (groupableTool(event)) {
      run.push(event);
      continue;
    }

    if (run.length > 0 && !rendersWorkItem(event)) {
      held.push(event);
      continue;
    }

    flush();
    items.push(event);
  }
  flush();
  return items;
}

/// Cuts the main thread into turns: each runs from a user prompt to the
/// `turn_completed` that closes it. The renderer places all intermediate work
/// in one disclosure and keeps the final answer outside it.
function groupTurns(events: AgentEvent[]): Turn[] {
  const turns: Turn[] = [];
  let current: OpenTurn | null = null;

  // A turn is only pushed once per transcript build, here, so grouping has one
  // source of truth for both live and settled turns.
  const close = (turn: OpenTurn) => {
    let finalText = turn.finalText;
    if (finalText === null && turn.completed !== null) {
      for (let i = turn.work.length - 1; i >= 0; i--) {
        const payload = turn.work[i].payload;
        if (payload.type === "assistant_text") {
          finalText = payload.text;
          break;
        }
      }
    }

    // The completion payload copies the final assistant message. Keep that
    // message permanently outside the disclosure—even after it is expanded—so
    // “everything except the final output” remains an honest boundary.
    const detailWork = groupTools(turn.work);
    if (finalText !== null && turn.completed !== null) {
      for (let i = detailWork.length - 1; i >= 0; i--) {
        const item = detailWork[i];
        if (isToolGroup(item)) continue;
        const payload = item.payload;
        if (payload.type === "assistant_text" && payload.text === finalText) {
          detailWork.splice(i, 1);
          break;
        }
      }
    }

    turns.push({ ...turn, finalText, work: detailWork });
  };

  const open = (prompt: AgentEvent | null, key: string): OpenTurn => ({
    prompt,
    work: [],
    completed: null,
    finalText: null,
    key,
  });

  for (const event of events) {
    // Usage updates drive indicators rather than transcript rows. In
    // particular, the context-stats response arrives after `turn_completed`;
    // letting it open a synthetic head turn would duplicate the Worked-for row.
    if (event.payload.type === "usage_update") continue;

    // Older native-agent logs include empty text placeholders between model
    // requests. They are not output, and must not split tools or hide the last
    // real assistant message when a completed turn needs a final-text fallback.
    if (event.payload.type === "assistant_text" && !event.payload.text.trim()) continue;

    // A queued prompt does not open a turn: it was typed into one already
    // running, and the CLI answers both inside it and emits a single
    // `turn_completed` for the pair. Cutting here would leave the first turn
    // permanently open and hand its remaining work to a second one. It falls
    // through to the bottom instead and renders as a row where it was typed.
    //
    // `current` still guards it — a queued prompt is only ever queued onto a
    // turn, but a log replayed from a truncation could start on one, and it
    // has to have somewhere to go.
    const inlineQueued = event.payload.type === "user_message" && event.payload.queued && current;

    if (event.payload.type === "user_message" && !inlineQueued) {
      if (current) close(current);
      current = open(event, event.id);
      continue;
    }

    // Events before any prompt still need a home — a resumed session replays
    // the log from wherever it was truncated.
    current ??= open(null, `head-${event.id}`);

    if (event.payload.type === "turn_completed") {
      current.completed = event;
      current.finalText = event.payload.finalText;
      close(current);
      current = null;
      continue;
    }

    current.work.push(event);
  }

  // The open trailing turn groups too, so a run collapses as it arrives rather
  // than only once the turn closes.
  if (current) close(current);
  return turns;
}

/// Stands in for the result a call will now never get.
///
/// Not an error: nothing went wrong with the call, the process it belonged to
/// stopped existing. Flagging it would tint the row red and spring it open on
/// load, which is a lot of noise for "this didn't finish".
const ABANDONED: ToolResult = {
  text: "No result — the session ended before this call finished.",
  isError: false,
  structured: null,
  exitCode: null,
  durationMs: null,
  images: [],
};

export function buildTranscript(
  source: AgentEvent[],
  /// Whether a child is actually running this session. A call with no result is
  /// only *pending* while something could still produce one; with the process
  /// gone it is abandoned, and rendering it as in-flight leaves a row shimmering
  /// forever. Most visible on `AskUserQuestion`, which blocks the harness until
  /// the app answers and so is the call most likely to be open at a quit — but
  /// it is true of any tool call caught mid-flight.
  live = false,
): {
  /// Events in `seq` order.
  events: AgentEvent[];
  /// The events, cut into user-prompt-to-turn-completed spans.
  turns: Turn[];
  resultByCallId: Map<string, ToolResult>;
  /// Questions still waiting on the user, oldest first. Lifted out of the turns
  /// so a request cannot be buried in a turn that collapses while it waits.
  pendingAsks: PendingAsk[];
} {
  const events = [...source].sort(bySeq);

  const resultByCallId = new Map<string, ToolResult>();
  // Calls with no result yet, and the ones a later event proved will never get
  // one. Only the second is decided during the walk — a result routinely lands
  // many events after its call, so "still open" is a running state, not a
  // verdict.
  const open = new Set<string>();
  const abandoned = new Set<string>();
  const asks: PendingAsk[] = [];
  const answered = new Set<string>();
  for (const event of events) {
    if (event.payload.type === "tool_call_started") {
      open.add(event.payload.callId);
    }
    if (event.payload.type === "tool_call_completed") {
      open.delete(event.payload.callId);
      resultByCallId.set(event.payload.callId, event.payload.result);
    }
    // A new prompt closes the book on everything before it: whatever the agent
    // was mid-way through, this turn is not going to finish it. Without this the
    // marks below would be undone by the next send — the session goes live
    // again, and a row abandoned at the last restart would start shimmering a
    // second time.
    //
    // A *queued* prompt proves the opposite. It was typed into a turn that was
    // already running, and the CLI folds it into that turn — so the calls open
    // in front of it are still live, and marking them here would stop a running
    // tool's row shimmering while it is genuinely still working.
    if (event.payload.type === "user_message" && !event.payload.queued) {
      for (const callId of open) abandoned.add(callId);
      open.clear();
    }
    if (event.payload.type === "questions_asked") {
      asks.push(event.payload);
    }
    if (event.payload.type === "question_answered") {
      answered.add(event.payload.requestId);
    }
  }

  const pendingAsks = asks.filter((ask) => !answered.has(ask.requestId));

  // Whatever is still open at the end of the log is only pending while something
  // could still produce a result. With no child running, nothing can.
  if (!live) for (const callId of open) abandoned.add(callId);

  // Applied last, and only where no real result exists.
  for (const callId of abandoned) {
    if (!resultByCallId.has(callId)) resultByCallId.set(callId, ABANDONED);
  }

  return {
    events,
    turns: groupTurns(events),
    resultByCallId,
    pendingAsks,
  };
}
