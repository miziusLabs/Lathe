import { useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { ArrowUp, CornerDownLeft, Paperclip, Square, X } from "lucide-react";

import AttachmentTray from "@/components/composer/AttachmentTray";
import FileMentionMenu from "@/components/composer/FileMentionMenu";
import PromptStashMenu from "@/components/composer/PromptStashMenu";
import SlashCommandMenu from "@/components/composer/SlashCommandMenu";
import PickerMenu from "@/components/composer/PickerMenu";
import {
  EFFORT_LABELS,
  filterEfforts,
  filterModels,
  modelKey,
  modelLabel,
} from "@/components/composer/ModelSelector";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { Kbd, KbdGroup } from "@/components/ui/kbd";
import {
  addAttachmentPaths,
  addPastedFile,
  addPastedImage,
  clearAttachments,
  pickAttachments,
  removeAttachment,
  useAttachments,
} from "@/hooks/useAttachments";
import { useDraft } from "@/hooks/useDraft";
import { useFileSearch } from "@/hooks/useFileSearch";
import { useHotkey } from "@/hooks/useHotkey";
import { stashedPromptsForProject, usePromptStash } from "@/hooks/usePromptStash";
import { SEGMENT_COLOR, highlightSegments, splitMention } from "@/lib/highlight";
import { applyMention, mentionSpan } from "@/lib/mention";
import {
  applyCommand,
  applyCommandArgument,
  drayCommands,
  filterCommands,
  filterCommandsByPrefix,
  parseSlashCommand,
  slashArgumentQuery,
  slashPrefix,
  slashQuery,
} from "@/lib/slash";
import { pastedFilePath } from "@/lib/paste";
import { PROJECT_DRAG_EVENT } from "@/lib/projectDrag";
import { IS_MAC } from "@/lib/platform";
import { cn } from "@/lib/utils";
import type {
  Effort,
  FileMatch,
  Model,
  QueuedMessage,
  SlashCommand,
} from "@/types/events";

type ChatInputProps = {
  /// `attachmentPaths` is what the tray held, as absolute paths. The backend
  /// re-reads each one — nothing but paths crosses the bridge, so a pinned
  /// screenshot is never uploaded twice.
  onSend: (message: string, attachmentPaths: string[], queueAfterTurn?: boolean) => void;
  /// Lathe-provided skills for the `$` picker. Empty until the backend's probe
  /// lands, and empty forever if it failed — Lathe commands remain available and
  /// text typed by hand still works.
  commands?: SlashCommand[];
  /// The shown model catalog and controls are also used by Lathe's `/model`,
  /// `/models`, and `/effort` commands, so those completions do not need a
  /// second picker path.
  models: Model[];
  modelId: Model["id"];
  agentModel: Model["agentModel"];
  effort: Effort | null;
  onModelChange: (modelId: Model["id"], effort: Effort | null, agentModel: Model["agentModel"]) => void;
  onNewSession: () => void;
  onSettle: () => void | Promise<void>;
  /// Where the `@` picker searches for files. Cloud sessions expose an empty
  /// host-side marker because their actual workspace stays inside Docker.
  cwd?: string | null;
  /// The project associated with this composer. Prompt stashes are scoped to
  /// this path; legacy stashes without a project are intentionally not migrated.
  projectPath?: string | null;
  /// Interrupts the running turn. Reachable while `busy` and the box is empty —
  /// with something typed Enter steers the active turn, while Ctrl+Enter queues
  /// a follow-up for after it completes.
  onStop?: () => void;
  /// Takes back the newest prompt still waiting on the CLI, resolving to it so
  /// its text can go back in the box. `null` when the flush got there first.
  onCancelQueued?: () => Promise<QueuedMessage | null>;
  /// How many prompts are waiting. Only decides whether Esc is bound — the rows
  /// themselves are drawn by the transcript, above this component.
  queuedCount?: number;
  /// Rendered outside the card — below it normally, above it on a new task. A
  /// node rather than the controls' own props, so this component keeps owning
  /// layout and measurement and nothing else.
  toolbar?: ReactNode;
  /// The "hand it back" actions, parked behind the card with a quarter of each
  /// button standing above it. A node for the toolbar's reason, and placed here
  /// rather than by the shell so it sits inside the same `max-w-3xl` column and
  /// against the card's own top edge — the card is what hides it, so nothing can
  /// come between them. Absent on a new task: there is no session to send into.
  handoff?: ReactNode;
  busy?: boolean;
  /// Which session's draft is in the box, and what the composer refocuses on
  /// when the user switches. `null` is the new task's own draft, not the
  /// absence of one.
  sessionId?: string | null;
  /// No session yet, so the composer stands alone mid-window. Nothing sits
  /// behind it to separate it from: the card drops its fill, border, and
  /// padding, the toolbar moves above — reading order runs settings first, then
  /// the box they apply to — and the send button gives way to a keyboard hint.
  isNewTask?: boolean;
  /// A backend failure, shown above the composer. Lives here rather than in the
  /// shell so it inherits the form's `max-w-3xl` column and lines up with the
  /// input; the transcript is the wrong home for it, since most of these fail
  /// before any session exists to have a transcript.
  error?: string | null;
  onDismissError?: () => void;
  /// A settled session takes no new turns, so the composer is replaced by the one
  /// control that can change that. Handled here rather than in the shell so the
  /// bar inherits the form's column and sits exactly where the card would.
  archived?: boolean;
  onUnarchive?: () => void;
};

const MAX_ROWS = 10;
// The empty state has no transcript above it to crowd, so the box can take a lot
// more of the window before it starts scrolling. Capped rather than unbounded
// because this composer is centered: past the window's height it would overflow
// off both ends at once, putting the wordmark past the top edge with nothing to
// scroll it back.
const NEW_TASK_MAX_ROWS = 20;

// Everything that decides where a glyph lands. The textarea and the overlay that
// colours a command inside it must agree on all of it exactly, or the two copies
// of the text drift apart and show as ghosting — so they share one constant
// rather than two matching class lists. The horizontal padding varies by state
// and is applied at both call sites alongside this.
const TEXT_BOX = "py-1 text-composer";

// The file is the source, so editing the logo needs no change here — but an
// <img> paints the file's own fill and this has to take the page's text color.
// So it is a mask over a `currentColor` background: the SVG supplies the shape,
// the CSS supplies the ink. Prefixed as well as not, for the older WebKit a
// Linux build runs on.
const WORDMARK_MASK = {
  maskImage: "url(/assets/lathe-logo.svg)",
  WebkitMaskImage: "url(/assets/lathe-logo.svg)",
  maskSize: "contain",
  WebkitMaskSize: "contain",
  maskRepeat: "no-repeat",
  WebkitMaskRepeat: "no-repeat",
  // `contain` + `left` is what makes the box tolerant of a redrawn logo: the
  // mark fits inside it at whatever aspect ratio the file has, rather than
  // being stretched to a ratio hardcoded here.
  maskPosition: "left",
  WebkitMaskPosition: "left",
} as const;

export default function ChatInput({
  onSend,
  commands = [],
  models,
  modelId,
  agentModel,
  effort,
  onModelChange,
  onNewSession,
  onSettle,
  cwd = null,
  projectPath = null,
  onStop,
  onCancelQueued,
  queuedCount = 0,
  toolbar,
  handoff,
  busy = false,
  sessionId = null,
  isNewTask = false,
  error = null,
  onDismissError,
  archived = false,
  onUnarchive,
}: ChatInputProps) {
  const [message, setMessage] = useDraft(sessionId);
  const [resizeTick, setResizeTick] = useState(0);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const cardRef = useRef<HTMLDivElement>(null);
  const mirrorRef = useRef<HTMLDivElement>(null);

  // Where the caret is, tracked so the picker can tell a command being typed
  // from a slash that has already been left behind.
  const [caret, setCaret] = useState(0);
  // Escape shuts the picker without clearing what was typed. Cleared again as
  // soon as the caret leaves the command, so the next `/` reopens it.
  const [dismissed, setDismissed] = useState(false);
  const [activeIndex, setActiveIndex] = useState(0);
  const [stashMenuOpen, setStashMenuOpen] = useState(false);
  // Set by a pick, applied once React has painted the new value — a controlled
  // textarea otherwise puts the caret at the end, which is wrong whenever the
  // completed command has arguments after it.
  const pendingCaretRef = useRef<number | null>(null);

  const attachments = useAttachments(sessionId);
  const { prompts: allStashedPrompts, stashPrompt, removePrompt } = usePromptStash();
  const stashedPrompts = useMemo(
    () => stashedPromptsForProject(allStashedPrompts, projectPath),
    [allStashedPrompts, projectPath],
  );
  // Set while the OS is dragging files over the window. Tauri intercepts the
  // native drop before the webview sees it, so there are no HTML drag events to
  // read here — `onDragDropEvent` is the only source, and it reports paths
  // rather than `File` handles, which is exactly what the backend wants anyway.
  const [dragging, setDragging] = useState(false);
  // HTML project drags also cross the Tauri webview, but must never become file
  // attachments. This ref lets the native listener ignore those events without
  // delaying or changing the ordinary file-drop path.
  const projectDraggingRef = useRef(false);

  // The runs that take a colour. Empty of anything but plain text most of the
  // time, which is what the overlay below checks before mounting at all.
  const segments = useMemo(() => highlightSegments(message), [message]);
  const highlighted = segments.some((segment) => segment.kind !== "text");

  // Lathe owns slash commands. Lathe only supplies skills, which remain useful as
  // `$` prompt completions without allowing Lathe's command registry to shape the
  // command surface.
  const availableCommands = useMemo(
    () => [...drayCommands(isNewTask), ...commands],
    [commands, isNewTask],
  );

  // Two modes, and the difference is deliberate. With nothing typed this is
  // browsing, so the list is grouped. Once there is a query it is searching,
  // and headers would hide matches behind section chrome, so the ranked list is
  // drawn flat.
  const query = slashQuery(message, caret);
  const prefix = slashPrefix(message, caret);
  const argument = slashArgumentQuery(message, caret, ["model", "models", "effort"]);
  const groups = useMemo(() => {
    if (query === null || prefix === null) return [];
    const matchingKind = filterCommandsByPrefix(availableCommands, prefix);
    const matches = query === "" ? matchingKind : filterCommands(matchingKind, query);
    return matches.length ? [{ label: null, items: matches }] : [];
  }, [availableCommands, prefix, query]);

  const selectedModel = models.find(
    (model) =>
      model.id === modelId &&
      (model.id !== "dray" ||
        (model.agentModel?.provider === agentModel?.provider && model.agentModel?.id === agentModel?.id)),
  );
  const modelMatches = useMemo(
    () =>
      (argument?.commandName === "model" || argument?.commandName === "models")
        ? filterModels(models, argument.query)
        : [],
    [argument, models],
  );
  const effortMatches = useMemo(
    () =>
      argument?.commandName === "effort"
        ? filterEfforts(argument.query, selectedModel?.efforts ?? [])
        : [],
    [argument, selectedModel?.efforts],
  );

  // The two pickers are mutually exclusive without needing to be arbitrated:
  // the caret sits in exactly one token, and a token opening with `/` at
  // position zero is not one opening with `@`. Kept as two independent reads so
  // neither has to know the other exists.
  const mention = mentionSpan(message, caret);
  const files = useFileSearch(cwd, mention?.query ?? null);

  // Flattened in render order, so arrowing through the list and drawing it
  // can't disagree about which row an index names.
  const commandMatches = useMemo(() => groups.flatMap((group) => group.items), [groups]);

  // Only the count is shared between the pickers — the lists themselves stay
  // separate all the way to the pick, so nothing has to be narrowed back out
  // of a union that `mention` already decided.
  const argumentMatches =
    argument?.commandName === "model" || argument?.commandName === "models"
      ? modelMatches
      : effortMatches;
  const stashMenuVisible =
    stashMenuOpen && message.trim().length === 0 && stashedPrompts.length > 0;
  const rowCount = stashMenuVisible
    ? stashedPrompts.length
    : mention
      ? files.length
      : argument
        ? argumentMatches.length
        : commandMatches.length;
  const menuOpen =
    stashMenuVisible ||
    (!dismissed &&
      rowCount > 0 &&
      (query !== null || mention !== null || argument !== null));
  // Clamped rather than trusted: both lists arrive asynchronously, so a list
  // that shrinks under an already-moved selection would otherwise index past
  // its end — and an undefined row only shows up as a crash on the keystroke
  // that picks it.
  const active = Math.min(activeIndex, Math.max(rowCount - 1, 0));

  // Keyed on the query text rather than on the span, which is a fresh object
  // every keystroke and would reset the selection on a bare cursor move.
  const mentionQuery = mention?.query ?? null;
  useEffect(() => {
    setActiveIndex(0);
    if (query === null && mentionQuery === null) setDismissed(false);
  }, [query, mentionQuery]);

  useEffect(() => {
    if (stashMenuVisible) setActiveIndex(0);
  }, [stashMenuVisible]);

  const pickStashedPrompt = (prompt: (typeof stashedPrompts)[number]) => {
    removePrompt(prompt.id);
    setStashMenuOpen(false);
    pendingCaretRef.current = prompt.text.length;
    setMessage(prompt.text);
    void addAttachmentPaths(sessionId, prompt.attachmentPaths ?? []);
    textareaRef.current?.focus();
  };

  const pickCommand = (command: SlashCommand) => {
    const next = applyCommand(message, command.name, command.isSkill, caret);
    pendingCaretRef.current = next.caret;
    setMessage(next.text);
    textareaRef.current?.focus();
  };

  const pickFile = (file: FileMatch) => {
    if (!mention) return;

    const next = applyMention(message, mention, file.path);
    pendingCaretRef.current = next.caret;
    setMessage(next.text);
    textareaRef.current?.focus();
  };

  const pickArgument = (value: string) => {
    if (!argument) return;

    const next = applyCommandArgument(message, argument.commandName, value, caret);
    pendingCaretRef.current = next.caret;
    setMessage(next.text);
    textareaRef.current?.focus();
  };

  /// The keyboard's way into whichever list is drawn. A click calls the same
  /// two functions directly, so the two routes cannot diverge.
  const pickRow = (index: number) => {
    if (stashMenuVisible) {
      const prompt = stashedPrompts[index];
      if (prompt) pickStashedPrompt(prompt);
      return;
    }

    if (mention) {
      const file = files[index];
      if (file) pickFile(file);
      return;
    }

    if (argument) {
      const value = argumentMatches[index];
      if (value) pickArgument(typeof value === "string" ? value : modelLabel(value));
      return;
    }

    const command = commandMatches[index];
    if (command) pickCommand(command);
  };

  useLayoutEffect(() => {
    const pending = pendingCaretRef.current;
    if (pending === null) return;
    pendingCaretRef.current = null;

    textareaRef.current?.setSelectionRange(pending, pending);
    setCaret(pending);
  }, [message]);

  // Grow to fit, then scroll. Height must be cleared before scrollHeight is read
  // or it reports the current height and the box can never shrink back down.
  useLayoutEffect(() => {
    const el = textareaRef.current;
    // Referenced rather than reached via `parentElement`, so the freeze below
    // keeps working as the composer's nesting changes.
    const card = cardRef.current;
    if (!el || !card) return;

    const style = getComputedStyle(el);
    const lineHeight = parseFloat(style.lineHeight) || 20;
    const chrome =
      parseFloat(style.paddingTop) +
      parseFloat(style.paddingBottom) +
      parseFloat(style.borderTopWidth) +
      parseFloat(style.borderBottomWidth);

    // Freeze the card while measuring: reading scrollHeight forces a layout with
    // the textarea at 0px, and if that phantom layout reaches the flex column the
    // chat pane momentarily grows and the browser clamps its scrollTop — the
    // transcript ratchets up a few pixels on every value change. The textarea's
    // own scroll position is clamped too, so keep it across the measurement or
    // the caret disappears as soon as the text reaches the row cap.
    const scrollTop = el.scrollTop;
    const scrollLeft = el.scrollLeft;
    card.style.height = `${card.offsetHeight}px`;
    el.style.height = "0px";
    // scrollHeight includes padding, so the row cap has to as well.
    const rows = isNewTask ? NEW_TASK_MAX_ROWS : MAX_ROWS;
    el.style.height = `${Math.min(el.scrollHeight, lineHeight * rows + chrome)}px`;
    el.scrollTop = scrollTop;
    el.scrollLeft = scrollLeft;
    if (mirrorRef.current) mirrorRef.current.scrollTop = el.scrollTop;
    card.style.height = "";
  }, [message, resizeTick, isNewTask]);

  useEffect(() => {
    const el = textareaRef.current;
    if (!el) return;

    el.focus();
    // The draft that just came back is text this composer has never had a caret
    // in, so the picker state left over from the session being switched away
    // from describes nothing here. Landing at the end is also where typing
    // resumes: a draft is an unfinished sentence.
    const end = el.value.length;
    el.setSelectionRange(end, end);
    setCaret(end);
    setDismissed(false);
    setStashMenuOpen(false);
  }, [sessionId]);

  // The sizing effect first measures against fallback font metrics, which can
  // clamp an empty box to the row cap; nothing re-measures until the next
  // keystroke, so the composer opens ten rows tall with a scrollbar. Waiting on
  // document.fonts.ready isn't enough — the promise can resolve a frame before
  // the new metrics reach layout, and that one shot is all it gets. Observing
  // the box re-measures whenever its size actually changes, font swap included.
  useEffect(() => {
    const el = textareaRef.current;
    if (!el) return;

    const observer = new ResizeObserver(() => setResizeTick((t) => t + 1));
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  // ⌥ as well as ⌘, so the chord can't collide with the webview's own ⌘O.
  useHotkey("o", () => void pickAttachments(sessionId), { alt: true });

  // The stash belongs to the app, not the current session. The same chord
  // restores when the input is empty, whether this is a New Task or follow-up;
  // a prompt with only attachments is still content worth stashing.
  useHotkey("s", () => {
    if (message.trim() || attachments.length) {
      stashPrompt(
        message,
        projectPath,
        attachments.map((attachment) => attachment.path),
      );
      setMessage("");
      clearAttachments(sessionId);
      setStashMenuOpen(false);
    } else {
      setStashMenuOpen(true);
    }
  });

  const insertPastedText = (text: string) => {
    const textarea = textareaRef.current;
    const value = textarea?.value ?? message;
    const start = textarea?.selectionStart ?? value.length;
    const end = textarea?.selectionEnd ?? start;
    const next = value.slice(0, start) + text + value.slice(end);
    const nextCaret = start + text.length;
    pendingCaretRef.current = nextCaret;
    setMessage(next);
    setCaret(nextCaret);
  };

  // What Esc does, wherever focus is. Held in a ref so the listener below can
  // register once and still read current state. Returns whether it consumed the
  // key, which is what decides if the webview ever sees it.
  //
  // Appended rather than assigned: whatever is half-typed here is the user's
  // too, and replacing it would trade one loss for another. Focus follows the
  // text back, since taking a prompt back is the start of editing it.
  const escapeRef = useRef<() => boolean>(() => false);
  escapeRef.current = () => {
    // Shuts the picker without clearing what was typed.
    if (stashMenuVisible) {
      setStashMenuOpen(false);
      return true;
    }

    if (menuOpen) {
      setDismissed(true);
      return true;
    }

    // Takes back the newest prompt still waiting on the CLI.
    if (onCancelQueued && queuedCount > 0) {
      const before = message;
      void onCancelQueued().then((cancelled) => {
        if (!cancelled) return;
        setMessage(before ? `${before}\n${cancelled.text}` : cancelled.text);
        textareaRef.current?.focus();
      });
      return true;
    }

    return false;
  };

  // Bound on the document, not on the textarea: on the textarea it only fired
  // while the box held focus, and every other press fell through to the webview,
  // where macOS reads a bare Esc as "leave fullscreen" — so the window resized
  // instead of cancelling. Swallowed only when it did something, or fullscreen
  // would lose its own exit for nothing.
  //
  // Bubble phase and skipped once handled, because Radix's layers listen in
  // capture and preventDefault when they dismiss — so an open dialog, menu or
  // lightbox spends the key before this sees it.
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      if (e.metaKey || e.ctrlKey || e.altKey || e.shiftKey) return;
      if (escapeRef.current()) e.preventDefault();
    };

    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, []);

  // The drop target is the whole window, not the card: a file aimed at the
  // composer while the transcript fills the screen would otherwise have to be
  // dropped on a 60px strip. The card is where the affordance is drawn, because
  // that is where the file is about to land.
  useEffect(() => {
    if (archived) return;

    let unlisten: (() => void) | null = null;
    let live = true;
    const onProjectDrag = (event: Event) => {
      const active = (event as CustomEvent<{ active: boolean }>).detail.active;
      projectDraggingRef.current = active;
      if (active) setDragging(false);
    };
    window.addEventListener(PROJECT_DRAG_EVENT, onProjectDrag);

    void getCurrentWebview()
      .onDragDropEvent((event) => {
        if (projectDraggingRef.current) return;

        // `enter` and `over` are one state here — the drag is over the window
        // and hasn't been dropped. Treating `enter` as anything else flashes
        // the overlay off for the frame between it and the first `over`.
        if (event.payload.type === "enter" || event.payload.type === "over") {
          setDragging(true);
        } else if (event.payload.type === "drop") {
          setDragging(false);
          void addAttachmentPaths(sessionId, event.payload.paths);
        } else {
          setDragging(false);
        }
      })
      .then((off) => {
        // The listener is registered asynchronously, so an unmount can land
        // first — drop it straight away rather than leaking a handler that
        // writes into a session this composer has already left.
        if (live) unlisten = off;
        else off();
      });

    return () => {
      live = false;
      window.removeEventListener(PROJECT_DRAG_EVENT, onProjectDrag);
      projectDraggingRef.current = false;
      unlisten?.();
    };
  }, [sessionId, archived]);

  // Enter steers the active turn; Ctrl+Enter queues a separate follow-up.
  const canSend = message.trim().length > 0 || attachments.length > 0;

  const runInternalCommand = (text: string): boolean => {
    const invocation = parseSlashCommand(text);
    if (!invocation) return false;

    const name = invocation.name.toLowerCase();
    if (name === "model" || name === "models") {
      const requested = invocation.args.toLowerCase();
      const next = models.find((model) =>
        [modelLabel(model), model.agentModel?.id, modelKey(model)]
          .filter((value): value is string => Boolean(value))
          .some((value) => value.toLowerCase() === requested),
      );
      if (next) onModelChange(next.id, null, next.agentModel);
      return true;
    }

    if (name === "effort") {
      const requested = invocation.args.toLowerCase();
      const next = selectedModel?.efforts.find(
        (level) => level === requested || EFFORT_LABELS[level].toLowerCase() === requested,
      );
      if (next) onModelChange(modelId, next, agentModel);
      return true;
    }

    if (name === "new") {
      if (!isNewTask) onNewSession();
      return true;
    }

    if (name === "settle") {
      if (!isNewTask) void onSettle();
      return true;
    }

    return false;
  };

  const submit = (queueAfterTurn = false) => {
    const trimmed = message.trim();
    // An attachment on its own is a real prompt — dropping a screenshot and
    // pressing Enter is asking about the screenshot.
    if (!trimmed && !attachments.length) return;

    // Internal commands change Lathe state and must never become Lathe prompts.
    // Skills and unknown slash text retain the ordinary send path.
    if (runInternalCommand(trimmed)) {
      setMessage("");
      clearAttachments(sessionId);
      return;
    }

    onSend(
      trimmed,
      attachments.map((a) => a.path),
      queueAfterTurn,
    );
    setMessage("");
    clearAttachments(sessionId);
  };

  // Returns before the form, so there is no disabled textarea to focus and no
  // submit path to reach at all — a disabled input still reads as "type here,
  // but not now", and this session isn't waiting on anything.
  //
  // After the hooks above, which must stay unconditional: settling the open
  // session swaps this in under a mounted composer.
  //
  // The live composer is a card plus a 34px toolbar row beneath it. Only the card
  // has a settled counterpart, so that row's height is held below as empty space:
  // `pb-4` + 34px. Without it the bar sits 34px lower than every other session's
  // composer and the transcript shifts down with it.
  if (archived) {
    return (
      <div className="px-4 pb-[3.125rem]">
        {/* `px-3 py-3` is the live card's own padding, so the button's right edge
            lands where the submit button's does. The label carries the textarea's
            extra `px-1` itself — inside the card those two sit on different
            edges, and matching only one of them is what reads as a shift. */}
        <div className="mx-auto flex max-w-3xl items-center justify-between gap-3 rounded-2xl border border-[oklch(1_0_0/6%)] bg-card px-3 py-3">
          <span className="px-1 text-composer text-muted-foreground">
            Unsettle this task to send a follow-up.
          </span>

          <Button variant="secondary" size="sm" onClick={onUnarchive}>
            Unsettle
          </Button>
        </div>
      </div>
    );
  }

  const textareaPlaceholder = isNewTask
    ? "Describe a task. @files. $skills and /commands."
    : menuOpen
      ? "Send follow-up"
      : "";
  const promptShortcutHint = !menuOpen && (isNewTask || !message.trim()) && (
    <div
      className={cn(
        "flex items-end gap-1",
        isNewTask
          ? "pt-2 text-ui text-muted-foreground/60"
          : "pointer-events-none absolute inset-0 px-1 pb-1 text-composer text-foreground/80",
      )}
    >
      Press
      <Kbd>
        <CornerDownLeft className="size-3" strokeWidth={2} />
      </Kbd>
      to {isNewTask ? "send" : "send follow up"}{" "}
      {(message.trim() || attachments.length > 0 || stashedPrompts.length > 0) && (
        <>
          <span>or</span>
          <KbdGroup>
            <Kbd>{IS_MAC ? "⌘" : "Ctrl"}</Kbd>
            <Kbd>S</Kbd>
          </KbdGroup>
          <span>to {message.trim() || attachments.length > 0 ? "stash" : "restore"}</span>
        </>
      )}
    </div>
  );

  return (
    <div className="px-4 pb-4">
      <form
        className="mx-auto max-w-3xl"
        onSubmit={(e) => {
          e.preventDefault();
          submit();
        }}
      >
        {/* Decoration, so it is hidden from assistive tech. Sits on the form
            edge like the toolbar and the text below it. Height-sized so the
            mark scales with the layout rather than with a viewBox nobody
            reading this file should have to hold in their head. */}
        {isNewTask && (
          <div
            aria-hidden
            style={WORDMARK_MASK}
            className="mb-4 h-10 w-full max-w-30 bg-current text-foreground/10"
          />
        )}

        {/* Above the toolbar in both states, so the failure reads before the
            controls rather than after them. `whitespace-pre-wrap` because these
            are raw messages from git and the CLI, which carry their own line
            breaks — flattening them runs the offending filenames together.

            The button is positioned out of flow and the first line cleared for
            it with `text-indent`, rather than floating it or giving it a flex
            column. Both of those reserve space per-line: a float clears after
            line one, so a message with its own `\n` breaks lands on three
            different left edges. This way every line shares one edge and only
            the first is inset. */}
        {error && (
          <div className="relative mb-2 px-1 text-ui break-words whitespace-pre-wrap text-destructive">
            {onDismissError && (
              <button
                type="button"
                onClick={onDismissError}
                aria-label="Dismiss error"
                className="absolute top-px left-1 rounded p-0.5 opacity-70 transition-opacity hover:opacity-100"
              >
                <X className="size-3.5" strokeWidth={2} />
              </button>
            )}
            {/* Matches the button's 14px glyph plus its padding and the gap.
                Applied inline: an arbitrary Tailwind value would work, but the
                number has to track the icon size above and reads clearer next
                to it. */}
            <span style={onDismissError ? { textIndent: "1.5rem" } : undefined} className="block">
              {error}
            </span>
          </div>
        )}

        {/* Pulled left by the toolbar's own `px-1` plus the ghost button's 6px
            icon inset, so the `+` glyph — not the button box — lands on the
            same edge as the text below it. */}
        {isNewTask && <div className="-ml-2.5 pb-1.5">{toolbar}</div>}

        {/* Directly above the card and with no gap: the row runs on past its own
            reserve and behind the card, which is the opaque thing that hides it.
            Anything between the two would show the buttons through the gap and
            leave them floating rather than tucked. */}
        {!isNewTask && handoff}

        {/* The ring lives on the card so the whole composer reads as one control.
            --input bakes in its own alpha, which makes Tailwind's /40-style opacity
            modifiers silently no-op, so both states set an explicit color.
            `relative` anchors the command picker, which opens upward out of the
            card rather than displacing anything as it filters.

            `bg-composer`, not `bg-card`, and that is a vibrancy fix rather than
            a colour change: the two tokens carry the same value, and only
            `--card` becomes a 5.5% veil on glass. A veil is right for a surface
            sitting *in* the page and wrong for one that has to hide something —
            and this card has the handoff row parked behind it, which showed
            straight through. Its own token rather than a borrowed one, so the
            reason lives in the palette beside the rule it is an exception to. */}
        <div
          ref={cardRef}
          className={cn(
            "relative rounded-2xl transition-colors",
            !isNewTask && "bg-composer shadow-sm",
          )}
        >
          {/* The toolbar sits above the input in the empty state, so the list
              opens downward — upward it would cover the controls it sits next
              to. Its surface still matches the corresponding follow-up picker. */}
          {menuOpen &&
            (stashMenuVisible ? (
              <PromptStashMenu
                prompts={stashedPrompts}
                activeIndex={active}
                onPick={pickStashedPrompt}
                onHover={setActiveIndex}
                placement={isNewTask ? "below" : "above"}
              />
            ) : mention ? (
              <FileMentionMenu
                files={files}
                activeIndex={active}
                onPick={pickFile}
                onHover={setActiveIndex}
                placement={isNewTask ? "below" : "above"}
              />
            ) : argument?.commandName === "model" || argument?.commandName === "models" ? (
              <PickerMenu
                groups={[{ label: null, items: modelMatches }]}
                label="Models"
                keyOf={modelKey}
                activeIndex={active}
                onPick={(model) => pickArgument(modelLabel(model))}
                onHover={setActiveIndex}
                placement={isNewTask ? "below" : "above"}
                surface="composer"
                renderItem={(model) => (
                  <>
                    <span className="shrink-0 font-medium">{modelLabel(model)}</span>
                    {model.id === modelId &&
                      (model.id !== "dray" ||
                        (model.agentModel?.provider === agentModel?.provider &&
                          model.agentModel?.id === agentModel?.id)) &&
                      effort && (
                        <span className="shrink-0 text-muted-foreground/60">
                          {EFFORT_LABELS[effort]}
                        </span>
                      )}
                    {model.agentModel && (
                      <span className="min-w-0 truncate text-muted-foreground">
                        {model.agentModel.provider}/{model.agentModel.id}
                      </span>
                    )}
                  </>
                )}
              />
            ) : argument?.commandName === "effort" ? (
              <PickerMenu
                groups={[{ label: null, items: effortMatches }]}
                label="Reasoning effort"
                keyOf={(level) => level}
                activeIndex={active}
                onPick={pickArgument}
                onHover={setActiveIndex}
                placement={isNewTask ? "below" : "above"}
                surface="composer"
                renderItem={(level) => <span className="font-medium">{EFFORT_LABELS[level]}</span>}
              />
            ) : (
              <SlashCommandMenu
                groups={groups}
                activeIndex={active}
                onPick={pickCommand}
                onHover={setActiveIndex}
                placement={isNewTask ? "below" : "above"}
              />
            ))}

          {/* Covers the card rather than replacing anything, so the text and
              the tray stay legible underneath and the box doesn't resize the
              moment a file crosses the window. Inert to pointer events — the
              drop is the OS's, and Tauri delivers it whatever is on top. */}
          {dragging && (
            <div className="pointer-events-none absolute inset-0 z-10 flex items-center justify-center gap-2 rounded-2xl border-2 border-muted-foreground/25 bg-background/70 text-ui text-muted-foreground">
              <Paperclip className="size-3.5" strokeWidth={2} />
              Drop to attach
            </div>
          )}

          {/* Inside the card and above the text, so an attachment reads as part
              of the message being composed rather than as a separate control.
              Padded on the same edges as the textarea below it. */}
          {attachments.length > 0 && (
            <div className={cn("pt-3", isNewTask ? "px-0" : "px-3")}>
              <AttachmentTray
                attachments={attachments}
                onRemove={(path) => removeAttachment(sessionId, path)}
              />
            </div>
          )}

          {/* The textarea and button share a height on one line, so bottom alignment
              keeps it centered on one line and anchored as the textarea grows. */}
          <div className={cn("flex items-end gap-1 py-2", isNewTask ? "px-0" : "px-3")}>
            <div className="relative min-w-0 flex-1">
              <textarea
                ref={textareaRef}
                rows={1}
                autoFocus
                value={message}
                // Kept in step with the overlay below, which cannot scroll itself.
                onScroll={(e) => {
                  const mirror = mirrorRef.current;
                  if (mirror) mirror.scrollTop = e.currentTarget.scrollTop;
                }}
                placeholder={textareaPlaceholder}
                onChange={(e) => {
                  setMessage(e.currentTarget.value);
                  setStashMenuOpen(false);
                  setCaret(e.currentTarget.selectionStart);
                }}
                onPaste={(e) => {
                  const clipboard = e.clipboardData;
                  // Tauri exposes the source path for files copied from a file
                  // manager on some platforms. Do not restrict this to images:
                  // the backend already knows how to send every other file as a
                  // path mention.
                  const fileWithPath = Array.from(clipboard.files).find(
                    (file) => Boolean((file as File & { path?: string }).path),
                  );
                  const filePath = fileWithPath
                    ? (fileWithPath as File & { path?: string }).path
                    : undefined;
                  const path =
                    filePath ??
                    pastedFilePath(
                      clipboard.getData("text/uri-list"),
                      clipboard.getData("text/plain"),
                    );

                  if (path) {
                    e.preventDefault();
                    const fallback = clipboard.getData("text/plain") || path;
                    void addAttachmentPaths(sessionId, [path])
                      .then((attached) => {
                        if (!attached) insertPastedText(fallback);
                      })
                      .catch(() => {
                        // A stale or unreadable file path is still ordinary
                        // clipboard text, so restore it instead of losing it.
                        insertPastedText(fallback);
                      });
                    return;
                  }

                  // Some webviews provide clipboard bytes without the original
                  // path. Preserve the image path's existing behavior and save
                  // other file types to a temporary path for the same tray/send
                  // pipeline.
                  const fileItem = Array.from(clipboard.items).find(
                    (item) => item.kind === "file",
                  );
                  const file = fileItem?.getAsFile();
                  if (!file) return;

                  e.preventDefault();
                  void file
                    .arrayBuffer()
                    .then((buffer) => {
                      const bytes = new Uint8Array(buffer);
                      if (file.type.startsWith("image/")) {
                        return addPastedImage(sessionId, bytes, file.type);
                      }
                      return addPastedFile(sessionId, bytes, file.name);
                    })
                    .catch((error) => console.error("[paste file]", error));
                }}
                // Fires for arrow keys, clicks, and drags alike, so the picker
                // follows the caret however it moved rather than only on typing.
                onSelect={(e) => setCaret(e.currentTarget.selectionStart)}
                onKeyDown={(e) => {
                  // Whichever picker is open owns these keys, and only while it
                  // is — Enter completes the highlighted row instead of sending,
                  // which is the one place the composer's usual rule gives way.
                  if (
                    e.key === "Enter" &&
                    e.ctrlKey &&
                    !e.metaKey &&
                    !e.altKey &&
                    !e.shiftKey &&
                    !e.nativeEvent.isComposing
                  ) {
                    e.preventDefault();
                    submit(busy);
                    return;
                  }

                  if (menuOpen && !e.nativeEvent.isComposing) {
                    if (e.key === "ArrowDown") {
                      e.preventDefault();
                      setActiveIndex((active + 1) % rowCount);
                      return;
                    }
                    if (e.key === "ArrowUp") {
                      e.preventDefault();
                      setActiveIndex((active - 1 + rowCount) % rowCount);
                      return;
                    }
                    if (e.key === "Enter" || e.key === "Tab") {
                      e.preventDefault();
                      pickRow(active);
                      return;
                    }
                  }

                  // Esc is not read here — it is the document listener's, so it
                  // works with the composer unfocused too.

                  // Shift+Enter is the only way to get a newline; plain Enter sends.
                  if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                    e.preventDefault();
                    submit();
                  }
                }}
                // `py-1` puts one line at 28px — the buttons' own height — so the
                // first row looks centered against them without being. `min-w-0`
                // moved to the wrapper, where it still stops one long unbroken
                // token setting the flex item's floor and pushing the buttons off
                // the row.
                className={cn(
                  "block w-full resize-none overflow-y-auto bg-transparent placeholder:text-transparent focus:outline-none",
                  TEXT_BOX,
                  isNewTask ? "px-0" : "px-1",
                  // Hands the glyphs to the overlay only while there is something
                  // to colour. Every other moment the textarea draws its own text
                  // as before, so the usual case keeps no dependency on the
                  // overlay rendering correctly.
                  highlighted ? "text-transparent caret-foreground" : "text-foreground",
                )}
              />

              {message.length === 0 && textareaPlaceholder && (
                <div
                  aria-hidden
                  className={cn(
                    "pointer-events-none absolute inset-0 flex items-end pb-1 text-composer text-muted-foreground",
                    isNewTask ? "px-0" : "px-1",
                  )}
                >
                  <span className="min-w-0">{textareaPlaceholder}</span>
                </div>
              )}

              {!isNewTask && !message.trim() && promptShortcutHint}

              {/* Draws the text the textarea is hiding, so a command or a file
                  mention can take a colour — a textarea has no way to style part
                  of its value. Painted *over* the textarea rather than under it,
                  so a selection band sits behind these glyphs instead of covering
                  them; the caret still shows, since it falls between them.

                  Mounted only alongside `text-transparent` above, and built from
                  the same segments the transcript renders, so neither the two
                  copies of the text nor the two surfaces can disagree about what
                  is coloured. `TEXT_BOX` and the padding are shared with the
                  textarea for the same reason — any drift shows up as doubled
                  text. */}
              {highlighted && (
                <div
                  ref={mirrorRef}
                  aria-hidden
                  className={cn(
                    "pointer-events-none absolute inset-0 overflow-hidden whitespace-pre-wrap break-words text-foreground",
                    TEXT_BOX,
                    isNewTask ? "px-0" : "px-1",
                  )}
                >
                  {segments.map((segment, i) => {
                    // Every glyph the textarea lays out has to be laid out here
                    // too, so a mention is dimmed rather than shortened — the
                    // transcript is where it collapses to the filename.
                    if (segment.kind === "mention") {
                      const { dir, name } = splitMention(segment.text);

                      return (
                        <span key={i} className={SEGMENT_COLOR.mention}>
                          <span className="opacity-45">{dir}</span>
                          {name}
                        </span>
                      );
                    }

                    return (
                      <span key={i} className={SEGMENT_COLOR[segment.kind]}>
                        {segment.text}
                      </span>
                    );
                  })}
                </div>
              )}
            </div>

            {/* Stop when the composer is empty during a turn; with text, Enter
                steers the active turn and Ctrl+Enter queues a separate follow-up.
                Stop is a button rather than a form submitter, so it cannot also
                send the draft. */}
            {!isNewTask &&
              (() => {
                const stopping = busy && !canSend;

                return (
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <Button
                        type={stopping ? "button" : "submit"}
                        size="icon-sm"
                        aria-label={stopping ? "Stop" : "Send"}
                        disabled={stopping ? !onStop : !canSend}
                        onClick={stopping ? onStop : undefined}
                        className="rounded-full"
                      >
                        {stopping ? (
                          <Square className="fill-current" />
                        ) : (
                          <ArrowUp strokeWidth={2} />
                        )}
                      </Button>
                    </TooltipTrigger>
                    <TooltipContent
                      side="top"
                      className={cn(
                        "max-w-none whitespace-nowrap",
                        busy && canSend && "flex-col items-start gap-1.5",
                      )}
                    >
                      {stopping ? (
                        "Stop"
                      ) : busy ? (
                        <>
                          <span className="flex items-center gap-1.5">
                            <Kbd>Enter</Kbd>
                            <span>to steer</span>
                          </span>
                          <span className="flex items-center gap-1.5">
                            <KbdGroup>
                              <Kbd>Ctrl</Kbd>
                              <Kbd>Enter</Kbd>
                            </KbdGroup>
                            <span>to queue</span>
                          </span>
                        </>
                      ) : (
                        <span className="flex items-center gap-1.5">
                          <Kbd>Enter</Kbd>
                          <span>to send</span>
                        </span>
                      )}
                    </TooltipContent>
                  </Tooltip>
                );
              })()}
          </div>
        </div>

        {isNewTask ? (
          // The new-task legend remains below the card; while a picker is open,
          // Enter completes the highlighted row rather than sending, so it stays
          // hidden.
          promptShortcutHint
        ) : (
          <div className="pt-1.5">{toolbar}</div>
        )}
      </form>
    </div>
  );
}
