# Lathe repository guide

Lathe is a Tauri 2 desktop application for running coding-agent sessions through a native chat UI. The agent is implemented in Rust in `packages/agent`, embedded in the desktop binary. ChatGPT OAuth provides access to the account-specific OpenAI model catalog. The frontend is React 19 + Vite 7 + Tailwind CSS 4; the backend is Rust and owns process/session management, persistence, Git/GitHub integration, file indexing, attachments, notifications, and local Git Worktree Sessions.

This file is the implementation map for agents working in this repository. Keep the user-facing overview in `README.md` concise; update this file when components, major behavior, or repository structure change.

## Repository layout

- `apps/desktop/` — the only application currently in the workspace.
- `apps/desktop/src/` — React frontend.
- `apps/desktop/src/components/` — application UI and shared UI primitives.
- `apps/desktop/src/hooks/` — stateful frontend behavior and Tauri data access.
- `apps/desktop/src/lib/` — pure helpers, transcript/diff parsing, presentation logic, and small platform integrations.
- `apps/desktop/src/types/events.ts` — generated Rust/TypeScript event and command types. `cargo test` regenerates it through `ts-rs`; avoid hand-maintaining generated definitions.
- `apps/desktop/src-tauri/src/` — Rust backend and Tauri command surface.
- `apps/desktop/scripts/` — Tauri launcher, Windows installer, and icon tooling.
- `apps/desktop/public/` — app assets, sounds, and logos.
- `packages/agent/` — standalone Rust runtime, streaming Responses transport, tools, process cleanup, skill discovery, and embedded prompts.

## Main product features

- Native desktop chat UI for Lathe coding-agent sessions.
- Multiple persistent sessions with search, unread/waiting/working state, pinning, settling/archiving, deletion, forking, and parent/child nesting.
- Local Sessions that run in a selected project checkout and Worktree Sessions that run in separate Git checkouts under `<repo>/.lathe/worktrees`.
- Project picker with attach, rename, delete-from-picker, manual ordering, and remembered selection.
- Git branch discovery and switching, including dirty-worktree handling before checkout.
- Account-specific OpenAI model catalog with model selection, reasoning/effort selection, configurable model cycling, and separate model/effort preferences for generated session titles.
- Rich transcript rendering for assistant text, user text, reasoning, tool calls, grouped tool calls, file edits, diffs, images, checkpoints, compaction, and structured question requests.
- Streaming assistant/tool output and live work indicators.
- Prompt queuing while a turn is already running, with cancellation/restoration of a queued prompt when still retractable.
- File attachments via picker or drag/drop, image previews, persistent archived result images, transcript thumbnails, and a keyboard-navigable image lightbox.
- `@file` fuzzy search backed by a warmed Rust file index.
- `/commands` and `$skills` discovered from `.agents/skills`, with search, source-aware grouping, aliases, and recent-command ranking.
- Context-window meter in the composer.
- Per-session draft preservation and focus restoration.
- Desktop notifications, in-app notices, dock/taskbar badge state, attention indicators, and notification/celebration sounds.
- Signed automatic updates from GitHub Releases, with background download and an install/restart notice.
- Turn-scoped Git snapshots so a completed turn's diff remains stable even if the checkout changes afterward.
- Git status handoff actions for Commit, Commit & push, Push, Create PR, and Draft PR.
- GitHub pull request markers and ready-to-merge notifications through `gh`; detailed PR viewing and mutations are not exposed in the UI.
- Sidebar PR markers and ready-to-merge notifications with polling/caching per repository.
- Syntax-highlighted Markdown/code and worker-backed diff rendering.
- Fixed One Dark Pro appearance and syntax highlighting, macOS vibrancy/titlebar integration, and cross-platform hotkeys.
- Safe quit flow that asks before exiting while sessions are active.

## Frontend entry points

- `src/main.tsx` — React bootstrap.
- `src/App.tsx` — top-level orchestration. Connects sessions, repository state, pull request marks, notices, settings, hotkeys, themes, and the composer.
- `src/App.css` — application-level layout and visual tokens.
- `src/styles/attention-glow.css` — attention/notification glow treatment.

## Application components

Top-level components in `src/components/`:

- `Avatar.tsx` — generic avatar/fallback rendering.
- `SessionAvatar.tsx` — session/project-aware avatar presentation.
- `Chat.tsx` — transcript list, follow-to-bottom behavior, streaming placement, and turn rendering.
- `ChatInput.tsx` — composer text input, send/stop behavior, command/file menus, queued-send behavior, error state, and attachment integration.
- `Sidebar.tsx` — task/session navigation, project grouping, search, nesting, session status, PR markers, pin/settle actions, row menus, and settings entry point.
- `PrStateIcon.tsx` — compact pull request state iconography.
- `SettingsDialog.tsx` — General, Providers, Models, and Updates tabs; account connection controls and profile picture, settled-session toggle, model-cycle configuration, and title-generation model configuration.
- `NoticeStack.tsx` — transient in-app notices.
- `UpdateNotice.tsx` — startup update check, background download progress, retry, and install/restart action.
- `QuitDialog.tsx` — quit confirmation for active work.
- `DiffWorkerPool.tsx` — shared worker/rendering pool for highlighted diffs.
- `FileIcon.tsx` — file-type icon selection.

### Composer components

Files in `src/components/composer/`:

- `ComposerToolbar.tsx` — attachment, project, worktree/local, branch, model, effort, and context controls.
- `ProjectSelector.tsx` — attach/select/reorder/rename/remove projects.
- `BranchSelector.tsx` — branch picker and dirty-worktree warning context.
- `BranchSwitchDialog.tsx` — branch-switch resolution when local changes need handling.
- `WorktreeToggle.tsx` — toggles local Worktree Session mode.
- `ModelSelector.tsx` — model and effort picker plus model-label/key helpers.
- `ContextMeter.tsx` — visual model-context usage meter.
- `AttachmentTray.tsx` — pending attachment previews/removal.
- `FileMentionMenu.tsx` — `@file` search results.
- `SlashCommandMenu.tsx` — `/command` and `$skill` picker.
- `PickerMenu.tsx` — reusable grouped/searchable picker surface.
- `HandoffRow.tsx` — post-work Git/PR actions.
- `handoffIcons.ts` — handoff action-to-icon mapping.

### Chat/transcript components

Files in `src/components/chat/`:

- `TurnBlock.tsx` — one user/assistant turn container.
- `UserMessage.tsx` — user prompt, file/image attachments, and prompt metadata.
- `AssistantMessage.tsx` — assistant Markdown/content blocks.
- `Reasoning.tsx` — collapsible reasoning presentation.
- `ToolCall.tsx` — completed tool call rendering.
- `StreamingToolCall.tsx` — in-flight tool call rendering.
- `ToolGroupRow.tsx` — compact grouped consecutive tool calls.
- `EventRow.tsx` — generic transcript event row.
- `FileEdits.tsx` — file-edit summaries from tool output.
- `DiffView.tsx` — inline edit/diff display.
- `CodeView.tsx` — highlighted code display.
- `Markdown.tsx` — Streamdown/Markdown renderer with code, links, and local-file link handling.
- `LinkDialog.tsx` — confirmation/details for links that need explicit handling.
- `ImageRow.tsx` — sent/returned image rows and overflow behavior.
- `ImageLightbox.tsx` — full-size image viewer with multi-image keyboard navigation.
- `QuestionRequest.tsx` — structured question/answer UI from the agent.
- `QueuedMessages.tsx` — queued follow-up prompts and cancellation.
- `CheckpointRail.tsx` — turn/checkpoint navigation rail.
- `CompactingIndicator.tsx` — context-compaction state.
- `WorkingIndicator.tsx` — active turn indicator.

### Layout and icon components

- `layout/AppShell.tsx` — sidebar and conversation/window shell.
- `layout/SessionHeader.tsx` — selected session title and branch context.
- `icons/PanelLeftIcon.tsx`, `GitBranchIcon.tsx` — custom chrome icons.

### Shared UI primitives

`src/components/ui/` contains the local shadcn/Radix-style primitives used by the application: `alert-dialog`, `alert`, `badge`, `button`, `card`, `collapsible`, `context-menu`, `dialog`, `dropdown-menu`, `input`, `kbd`, `questionnaire`, `scroll-area`, `separator`, `switch`, `textarea`, and `tooltip`.

## Frontend hooks

Files in `src/hooks/`:

- `useSessions.ts` — central frontend session state: session index/snapshots, model/project/branch/worktree controls, send/queue/interrupt/stop/respond/fork/detach/delete operations, backend event subscriptions, status/unread state, notices, and context/background-task derivation.
- `useWorkStatus.ts` — working tree, branch, upstream, default branch, and ahead/dirty state for handoff actions.
- `usePrMarks.ts` — per-repository cached PR markers for sidebar sessions.
- `usePrReady.ts` — announces PRs that become ready to merge.
- `useAttachments.ts` — composer attachment state and Tauri attachment reads.
- `useFileSearch.ts` — warms and queries the Rust fuzzy file index.
- `useSlashCommands.ts` — loads/caches native skills per working directory.
- `useRecentCommands.ts` — persists recent command/skill usage.
- `useComposerPrefs.ts` — persisted composer model/effort/worktree preferences.
- `useTitlePrefs.ts` — persisted title-generation model and effort.
- `useDraft.ts` — per-session unsent composer drafts.
- `useNotices.ts` — in-app notice state.
- `useDockBadge.ts` — dock/taskbar badge integration.
- `useTheme.ts` and `useCodeTheme.ts` — fixed One Dark Pro/dark appearance and shared syntax theme; no preferences, OS mode tracking, or switching APIs.
- `useHighlighter.ts` — shared syntax highlighter lifecycle.
- `useVibrancy.ts` — native window vibrancy behavior.
- `useFullscreen.ts` — native fullscreen state.
- `useHotkey.ts` and `useDoubleTap.ts` — keyboard shortcut helpers.
- `useLocalStorage.ts` — typed persisted browser storage state.

## Frontend library modules

Files in `src/lib/`:

- `transcript.ts` — converts raw backend events into renderable turns and result maps.
- `streaming.ts` — parses incremental stream payloads and reconstructs streamable content/tool data.
- `tools.ts` — tool-call classification/grouping helpers.
- `diff.ts` — edit/read extraction, diff sides, filenames, ranges, and line counts.
- `pr.ts` — sidebar pull-request selection, session-branch helpers, and ready-notice derivation.
- `handoff.ts` — derives available Commit/Push/PR handoff actions from Git state.
- `slash.ts` — command/skill invocation detection, ranking, grouping, parsing, and insertion.
- `mention.ts` and `highlight.ts` — `@file`, `$skill`, and command text segmentation/highlighting.
- `fileLinks.ts` — proxy/unwrap logic for local file links rendered through Markdown.
- `codePlugin.ts`, `codeTheme.ts`, `highlight.ts` — code rendering/highlight integration.
- `relay.ts` — frontend event relay helpers.
- `attention.ts` — session attention/unread presentation rules.
- `sessionOrder.test.ts` covers ordering behavior implemented alongside sidebar helpers.
- `format.ts` — relative time, token/byte counts, and path formatting.
- `models.ts` — model selection keys, supported reasoning resolution, and effort-preference migration.
- `focus.ts` — focus utilities.
- `notify.ts` — invokes native notification delivery.
- `sound.ts` — notification/celebration audio.
- `confetti.ts` — celebration visual effect.
- `theme.ts` — theme utilities.
- `platform.ts` — OS/platform detection.
- `utils.ts` — shared class-name/general helpers.

Most pure behavior has adjacent `*.test.ts`/`*.test.tsx` coverage. Preserve that pattern when adding logic that can be separated from UI wiring.

## Rust backend

Files in `src-tauri/src/`:

- `main.rs` — native executable entry point.
- `lib.rs` — Tauri builder, window lifecycle, command registration, and frontend-facing command wrappers.
- `session.rs` — core process/session manager: spawn/resume the native agent, local/worktree execution, stdin protocol, event streaming, prompt queuing, model changes, background-task control, forks, interrupt/kill, deletion, and status publication.
- `store.rs` — persistent session logs/index/snapshots, status flags, nesting metadata, and archive/pin state.
- `projects.rs` — persistent attached-project list, names, and recent selection ordering.
- `git.rs` — branch operations, tree snapshots, turn/revision diffs, file-version reads, commit log, work/sync status, commit, and push operations.
- `github.rs` — `gh`-based PR discovery, sidebar marks, checks/comments/reviews, ready/reopen/merge operations, and GitHub state normalization.
- `files.rs` — cached fuzzy file index used by `@file` mentions.
- `attachments.rs` — attachment validation/description, image encoding, session attachment storage, and returned-image archiving.
- `worktrees.rs` — Git worktree creation, remote fetch/fast-forward pull, and safe removal.
- `notifications.rs` — native desktop notifications and click handling.
- `quit.rs` — active-work quit interception and confirmation.
- `title.rs` — generated session-title behavior.
- `binpath.rs` — CLI executable discovery/path handling.
- `models/models.rs` — model IDs, OpenAI account model metadata, effort levels, and configured fallback model.
- `events/events.rs` — shared serializable event/domain model exported to TypeScript.
- `events/usage.rs` — token/context usage normalization.
- `harness/harness.rs` — harness abstraction and selection; Lathe native agent only.
- `harness/dray/dray.rs` — native process transport, auth delivery, persistence, and snapshots.
- `harness/dray/parser.rs` — native JSON-line event parsing.
- `harness/dray/mapper.rs` — maps native runtime events into the normalized event model.
- `harness/dray/commands.rs` — account model catalog and `.agents/skills` discovery.
- `account.rs` — loopback OAuth, PKCE, identity verification, serialized refresh, credential storage, cancellation, and revocation. `account/credential_store.rs` splits Windows credentials into bounded OS credential entries, publishing each complete generation through a manifest while retaining compatibility with older single-entry credentials.
- `usage/codex.rs` — read-only local Codex usage credentials from `CODEX_HOME/auth.json` (default `~/.codex`) or the native Codex credential store on macOS/Windows; requires a matching Lathe email and any known workspace ID. Codex owns refresh and persistence.
- `usage.rs` — account-wide Codex plan limits from `/backend-api/wham/usage`, following the official Codex client with backend-only ChatGPT OAuth credentials; exposes five-hour and weekly windows.

The frontend-facing Tauri command surface covers session send/read/control, attachments, models, commands/skills, file search, projects, branches, Git diffs/history/status, session flags/forks/deletion, notifications, PR operations, and quit confirmation. Add new native capabilities through a narrow command in `lib.rs` and keep implementation in the owning module.

## Worktree Sessions

Worktree mode requires a Git project and source branch. `worktrees.rs` creates
`<repo>/.lathe/worktrees/<uuid>` with detached HEAD (no new branch), fetches the source remote,
and pulls with `--ff-only` in the new checkout. Failed updates cancel creation;
the source checkout is never switched or merged. Nested worktrees are ignored
through the shared Git info/exclude. Agent context stays in the normal app store.
Forks can continue in place or create a new checkout; separate forks retain
agent history. Git snapshots, handoff actions, and file search use the actual
worktree cwd. Deletion never forces removal of dirty or shared checkouts.
Legacy `cloudName` remains in persisted data to preserve old transcripts;
resuming/forking legacy Cloud sessions is refused and their volumes are untouched.

## Built-in agent

`packages/agent/src/lib.rs` owns the persistent request/tool loop, SSE parsing,
queued follow-ups, interruption checkpoints, prompt caching, and compaction.
`tools.rs` provides directory listing, bounded read-only file discovery/content
search, research, and GitHub retrieval; the finder has dedicated native `find`
and `grep` tools instead of shell access. `process.rs` cleans up
shell descendants. `skills.rs` discovers
standard SKILL.md files in global `~/.mizius/skills` and applicable ancestor
`.agents/skills` directories. The embedded `SYSTEM.md` is the supplied personal Pi system prompt;
Pi itself and its authentication are no longer dependencies.

Responses output is collected from finalized SSE output items; the terminal
event may contain usage without repeating those items. Preserve message phases,
tool namespaces, and encrypted reasoning when saving and replaying history.
The loop executes tool calls and submits their results until a final answer,
continuing past commentary-only responses and consuming queued follow-ups.
Empty or unfinished output is an error rather than a successful completed turn.
Empty assistant text blocks are not transcript boundaries; consecutive tools stay
grouped across model requests until visible non-tool output appears.

Use the documented direct Sign in with ChatGPT token-sharing flow. Models and
reasoning levels come from the account catalog; do not hardcode supported models.
Plan usage follows the official Codex client's usage GET; do not probe unrelated
private quota endpoints. SIWC tokens may be denied by the Codex usage endpoint. On HTTP 401/403 only, usage can read a matching local Codex login without refreshing or modifying it; label that source and distinguish Codex account limits from Lathe’s per-app allowance. Missing, stale, mismatched, or denied plan limits must remain unavailable.
Model discovery sends a catalog compatibility `client_version` independently
of the app version, so newer account models are included. Reasoning preferences
are keyed by provider and model, and unsupported choices resolve to each model's
catalog default. Codex's `ultra` delegation mode is not a native reasoning effort.
Responses input is an array, function tools use the `dray` namespace, and requests
set `store: false`.
Retain encrypted reasoning between requests. Stable instructions, tool schemas,
history prefixes, and a session cache key support server caching. Record
per-request tokens including research and compaction. Cached input and reasoning
are subsets of input/output, not additional consumption.

Regenerate frontend types with `cargo test` or `cargo run --bin export-types`
from `apps/desktop/src-tauri`. Run standalone checks from the repository root
with `cargo test --manifest-path packages/agent/Cargo.toml`.

## Branding and compatibility

The product is Lathe with bundle ID `com.mizius.lathe` (development:
`com.mizius.lathe.dev`). The logo is the Lathe spindle mark. Monochrome SVGs
live in `apps/desktop/public/assets`, including `lathe-logo.svg` for the composer.
Light, dark, and purple development icon masters live in
`apps/desktop/src-tauri/icons/src`; regenerate the bundled PNG/ICO/ICNS assets
with `pnpm --filter lathe icons` after replacing a master. Development builds
use `icons/dev`. The dark master is retained as an appearance variant; it is
not currently selected automatically by the native app. The desktop package and
standalone executable are `lathe` and `lathe-agent`. Legacy `dray` harness/model
IDs, tool namespaces, preferences, data directories, environment variables, and
legacy data remain stable to preserve existing sessions and workspaces.
The GitHub repository and updater endpoint still use `miziusLabs/Dray`.

## Important interaction rules

- A local session is tied to a project/checkout; a Worktree Session uses its own local checkout.
- Completed-turn changes use Git tree snapshots. Do not replace them with a live `git diff` or the UI will drift after later edits.
- GitHub integration intentionally uses the user's installed/authenticated `gh` CLI rather than owning GitHub authentication.
- Session status distinguishes active work, waiting-for-user requests, completed-but-unread work, and idle/read work. Sidebar indicators and OS notices depend on that distinction.
- Parent/child session nesting represents forks/subsessions. Detach changes hierarchy; delete removes the session and its persisted data/resources.
- Worktree cleanup is user-data-sensitive: preserve dirty and shared checkouts.
- Generated TypeScript event types mirror Rust. Change the Rust model first and regenerate.
- Local links in Markdown use a proxy/unwrap path because normal browser link handling cannot safely expose arbitrary local paths directly.

## Keyboard shortcuts

`useHotkey` maps the primary modifier to Command on macOS and Control elsewhere unless the binding explicitly opts out.

- `Cmd/Ctrl+N` — new session.
- `Cmd/Ctrl+Shift+Up/Down` — move through sessions.
- `Cmd/Ctrl+,` — Settings.
- `Cmd/Ctrl+Shift+E` — cycle effort/reasoning level for the current model.
- `Cmd/Ctrl+M` — cycle the configured model subset.
- `Alt+O` — attach files.

## Development commands

Run workspace install/start commands from the repository root:

```sh
pnpm install
pnpm app
pnpm app:no-watch
pnpm test
pnpm build:app
```

`build:app` creates a local macOS `.dmg` with updater artifact generation
disabled and pnpm automatic version switching disabled. `npm run build:app`
invokes the same script from the repository root.

Run commands whose config paths are app-relative from the owning directory:

```sh
cd apps/desktop && pnpm tauri build
cd apps/desktop/src-tauri && cargo test
```

Useful targeted checks:

```sh
cd apps/desktop && pnpm test
cd apps/desktop && pnpm build
cd apps/desktop/src-tauri && cargo test
```

Prefer the smallest verification that covers a change. Documentation-only edits generally need only a diff/status review.

## Editing conventions

- Preserve existing user changes in a dirty worktree.
- Keep stateful Tauri I/O in hooks/backend modules and pure derivation in `src/lib/` when practical; this repository already tests that split heavily.
- Reuse the shared picker, UI primitive, diff, transcript, and formatting components instead of adding parallel one-off surfaces.
- Keep platform-specific behavior behind `platform.ts`, Tauri APIs, or Rust `cfg` branches.
- Update tests beside pure logic when behavior changes.
- Keep README feature claims aligned with the actual UI and backend; keep implementation detail here instead.
- When bumping the app version, keep `apps/desktop/package.json`, `apps/desktop/src-tauri/Cargo.toml`, `apps/desktop/src-tauri/tauri.conf.json`, and the README badge aligned.

## Version control workflow

- After completing each requested change or task, create a Git commit and push it to `main`.
- Every commit must have a concise title and a descriptive body explaining what changed and why.
