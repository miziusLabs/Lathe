import { useEffect, useMemo, useRef, useState } from "react";

import { invoke } from "@tauri-apps/api/core";

import "./App.css";
import Chat from "@/components/Chat";
import AnalyticsDialog from "@/components/AnalyticsDialog";
import ChatInput from "@/components/ChatInput";
import DeveloperDialog from "@/components/DeveloperDialog";
import DiffWorkerPool from "@/components/DiffWorkerPool";
import NoticeStack from "@/components/NoticeStack";
import QuitDialog from "@/components/QuitDialog";
import SettingsDialog from "@/components/SettingsDialog";
import { usePrMarks } from "@/hooks/usePrMarks";
import { usePrReady } from "@/hooks/usePrReady";
import { useWorkStatus } from "@/hooks/useWorkStatus";
import HandoffRow from "@/components/composer/HandoffRow";
import { handoffActions } from "@/lib/handoff";
import Sidebar, {
  filterSessions,
  sortSessions,
} from "@/components/Sidebar";
import { useUpdate } from "@/components/UpdateNotice";
import ComposerToolbar from "@/components/composer/ComposerToolbar";
import AppShell from "@/components/layout/AppShell";
import SessionHeader from "@/components/layout/SessionHeader";
import {
  modelsForKeys,
  nextEffort,
  resolveCycleEfforts,
} from "@/components/composer/ModelSelector";
import { TooltipProvider } from "@/components/ui/tooltip";
import { pickAttachments } from "@/hooks/useAttachments";
import { useCodeTheme } from "@/hooks/useCodeTheme";
import { useFullscreen } from "@/hooks/useFullscreen";
import { useVibrancy } from "@/hooks/useVibrancy";
import { warmHighlighter } from "@/hooks/useHighlighter";
import { useHotkey } from "@/hooks/useHotkey";
import { useLocalStorage } from "@/hooks/useLocalStorage";
import { titleDefaultEffort, titleModels, useTitlePrefs } from "@/hooks/useTitlePrefs";
import { useSessions } from "@/hooks/useSessions";
import type { Effort, Model, AgentModel } from "@/types/events";
import type { UsageDisplayMode } from "@/types/usage";
import { useSlashCommands } from "@/hooks/useSlashCommands";
import { sessionBranch } from "@/lib/pr";
import { playCelebration } from "@/lib/sound";
import { DEFAULT_NO_PROJECT_PATH } from "@/lib/projects";

function App() {
  const [titlePrefs, setTitlePrefs] = useTitlePrefs();
  const [noProjectPath, setNoProjectPath] = useLocalStorage<string>(
    "ade.noProjectPath",
    DEFAULT_NO_PROJECT_PATH,
  );
  const [autoDownloadUpdates, setAutoDownloadUpdates] = useLocalStorage<boolean>(
    "ade.autoDownloadUpdates",
    true,
  );
  const update = useUpdate(autoDownloadUpdates);
  const {
    selectedSessionId,
    selectedSession,
    streamingContentBlock,
    sessionIndexItems,
    statusBySession,
    askingSessions,
    showArchived,
    setShowArchived,
    harness,
    models,
    modelId,
    agentModel,
    effort,
    projects,
    projectPath,
    branches,
    branch,
    useWorktree,
    worktreeAvailable,
    worktreeUnavailableReason,
    busy,
    compacting,
    working,
    contextUsage,
    error,
    setError,
    handleModelChange,
    handleAttachProject,
    handleSelectProject,
    handleRenameProject,
    handleDeleteProject,
    handleReorderProjects,
    handleSelectBranch,
    pendingBranch,
    setPendingBranch,
    runCheckout,
    setUseWorktree,
    handleSendMsg,
    handleInterrupt,
    queuedMessages,
    handleCancelQueued,
    handleAnswerQuestions,
    handleSelectSessionIndexItem,
    handleNewSession,
    setSessionFlags,
    forkSession,
    detachSession,
    deleteSession,
  } = useSessions(titlePrefs, noProjectPath);

  const titleModelOptions = useMemo(() => titleModels(models), [models]);
  const handleTitleModelChange = (
    nextModelId: Model["id"],
    nextEffort: Effort | null,
    nextAgentModel: AgentModel | null,
  ) => {
    const nextModel = titleModelOptions.find(
      (model) =>
        model.id === nextModelId &&
        (model.id !== "dray" ||
          (model.agentModel?.provider === nextAgentModel?.provider &&
            model.agentModel?.id === nextAgentModel?.id)),
    );
    setTitlePrefs(
      nextModelId,
      nextEffort ?? titleDefaultEffort(nextModel),
      nextAgentModel,
    );
  };

  // `null` means the user has not configured a subset, so newly discovered
  // models appear in the picker automatically. Once configured, the stored
  // stable keys preserve that explicit choice across catalog refreshes and
  // relaunches.
  const [visibleModelKeys, setVisibleModelKeys] = useLocalStorage<string[] | null>(
    "ade.visibleModelKeys",
    null,
  );
  const visibleModels = modelsForKeys(models, visibleModelKeys);
  // `null` means the user has not configured a subset, so newly discovered
  // models join the cycle automatically. Once configured, the stored stable
  // keys preserve that explicit choice across catalog refreshes and relaunches.
  const [cycleModelKeys, setCycleModelKeys] = useLocalStorage<string[] | null>(
    "ade.cycleModelKeys",
    null,
  );
  const cycleModels = modelsForKeys(visibleModels, cycleModelKeys);
  // `null` preserves the original Medium-through-Max cycle. An explicit list,
  // including an empty one, is the user's configured reasoning cycle.
  const [cycleEfforts, setCycleEfforts] = useLocalStorage<Effort[] | null>(
    "ade.cycleEfforts",
    null,
  );

  // Not persisted: settings are opened to change something and closed again, so
  // reopening the app into them would be the app remembering the wrong half of
  // a session.
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [analyticsOpen, setAnalyticsOpen] = useState(false);
  const [usageDisplayMode, setUsageDisplayMode] = useLocalStorage<UsageDisplayMode>(
    "ade.usageDisplayMode",
    "left",
  );
  const [developerOpen, setDeveloperOpen] = useState(false);

  // Themes and Shiki's engine are shared by every code surface, so they load
  // once here instead of on the first diff the user happens to open.
  const { pair: codeThemePair } = useCodeTheme();
  useEffect(() => warmHighlighter(codeThemePair), [codeThemePair]);

  // What the composer's handoff row draws itself from, and — one line down —
  // which branch the pull requests are looked up by. Read on the same falling
  // edge as those, since a turn is what moves all of it.
  const { status: workStatus, refresh: refreshWorkStatus } = useWorkStatus(
    selectedSession?.projectPath ? selectedSession.cwd : "",
    busy,
  );

  // The sidebar starts from every session so repository marks and ready-to-merge
  // notices keep watching work even when a search temporarily hides a row.
  const visibleSessions = sessionIndexItems;
  const [search, setSearch] = useState("");
  const searchedSessions = useMemo(
    () => filterSessions(visibleSessions, search),
    [visibleSessions, search],
  );

  // The sidebar's marks: one `gh` per repo on screen rather than one per row —
  // see `usePrMarks`. Distinct paths, and the *active* list's only: a settled
  // session is work the reader has already dealt with, so a repo that appears
  // nowhere but the archived list is one nobody is waiting to land. Marks
  // already fetched still draw over there, since the cache outlives this — what
  // is dropped is the spending, not the answer.
  const repoPaths = useMemo(
    () =>
      showArchived
        ? []
        : [
            ...new Set(
              visibleSessions
                .filter((item) => item.projectPath)
                .map((item) => item.projectPath),
            ),
          ],
    [showArchived, visibleSessions],
  );
  const prMarks = usePrMarks(repoPaths);

  // Ready-to-merge notices and branch marks remain available in the sidebar.
  usePrReady({ sessions: visibleSessions, prFor: prMarks.prFor });

  const prBranch = !selectedSession?.projectPath
    ? null
    : sessionBranch(selectedSession, workStatus?.branch);
  const markHere = selectedSession?.projectPath
    ? prMarks.prFor(selectedSession.projectPath, prBranch)
    : null;

  // An open session's own directory, since project- and local-scoped commands
  // differ per repo and a session can be running somewhere the picker isn't
  // pointed — a worktree, or a project switched away from since. The `@` picker
  // resolves against the same directory for the same reason, and off the same
  // expression so the two can't answer for different trees.
  const composerCwd = selectedSession?.cwd ?? projectPath;
  const slashSkills = useSlashCommands(composerCwd, harness);

  const settleCurrentSession = async () => {
    if (!selectedSessionId) return;
    const settled = await setSessionFlags(selectedSessionId, { archived: true });
    if (!settled) return;
    playCelebration();
    handleNewSession();
  };

  const lastTurn = useRef({ sessionId: selectedSessionId, busy });
  useEffect(() => {
    const prev = lastTurn.current;
    lastTurn.current = { sessionId: selectedSessionId, busy };
    if (prev.sessionId !== selectedSessionId || !prev.busy || busy) return;
    prMarks.refresh();
  }, [selectedSessionId, busy, prMarks.refresh]);

  // Do not offer a duplicate Create PR action for a branch with an open PR.
  const sessionHasPr = markHere?.state === "OPEN";

  // The one handoff action that runs rather than asks. It reports into no
  // transcript, so both ends are its own: the flag the button spins on, and the
  // error banner above the composer — the same one every other backend failure
  // reaches the reader through.
  const [pushing, setPushing] = useState(false);
  const push = async () => {
    if (pushing || !selectedSession) return;
    setPushing(true);
    try {
      await invoke("push_branch", { cwd: selectedSession.cwd });
      // Straight away rather than on the next turn's edge: the count on the
      // button is now wrong, and it is the thing the reader is looking at.
      refreshWorkStatus();
    } catch (e) {
      setError(String(e));
    } finally {
      setPushing(false);
    }
  };

  // Same order the sidebar draws, so the walk matches the list — project list
  // included, since that is what orders the groups it steps through.
  const ordered = useMemo(
    () => sortSessions(searchedSessions, projects),
    [searchedSessions, projects],
  );

  // Wraps downward only. Falling off the bottom returns to the newest session,
  // which is where a walk through the whole list wants to end up; the top holds
  // instead, since arriving at the oldest session by pressing *up* past the
  // newest one reads as a mistake rather than as a wrap.
  const stepSession = (delta: number) => {
    if (ordered.length === 0) return;
    const from = ordered.findIndex((i) => i.sessionId === selectedSessionId);
    // No selection is the empty composer — either direction enters at the top.
    const next =
      from === -1
        ? 0
        : delta > 0
          ? (from + 1) % ordered.length
          : Math.max(from - 1, 0);
    const item = ordered[next];
    if (item.sessionId !== selectedSessionId) {
      void handleSelectSessionIndexItem(item.sessionId);
    }
  };

  const jumpSession = (index: number) => {
    const item = ordered[index];
    if (item && item.sessionId !== selectedSessionId) {
      void handleSelectSessionIndexItem(item.sessionId);
    }
  };

  useHotkey("n", handleNewSession);
  // ⌘⇧ rather than plain ⌘: the composer is focused most of the time, where
  // ⌘↑/↓ is the webview's own jump-to-start/end of the input.
  useHotkey("ArrowUp", () => stepSession(-1), { shift: true });
  useHotkey("ArrowDown", () => stepSession(1), { shift: true });
  // By position in the sidebar, so the shortcut follows the same order the
  // reader sees, including the current search and project grouping. The first
  // nine rows are reachable without leaving the composer.
  useHotkey("1", () => jumpSession(0));
  useHotkey("2", () => jumpSession(1));
  useHotkey("3", () => jumpSession(2));
  useHotkey("4", () => jumpSession(3));
  useHotkey("5", () => jumpSession(4));
  useHotkey("6", () => jumpSession(5));
  useHotkey("7", () => jumpSession(6));
  useHotkey("8", () => jumpSession(7));
  useHotkey("9", () => jumpSession(8));
  // ⌘, — every macOS app's preferences chord. Safe to take for
  // `useHotkey`'s usual pair of reasons: it claims the chord, and the app's
  // custom menu carries no Settings item to swallow the key first.
  useHotkey(",", () => setSettingsOpen(true));
  useHotkey("t", () => setAnalyticsOpen(true));
  // No accelerator: Shift+Tab cycles the effort setting for the current model.
  useHotkey(
    "Tab",
    () => {
      const next = nextEffort(
        models.find(
          (m) =>
            m.id === modelId &&
            (m.id !== "dray" ||
              (m.agentModel?.provider === agentModel?.provider && m.agentModel?.id === agentModel?.id)),
        ),
        effort,
        resolveCycleEfforts(cycleEfforts),
      );
      if (next) handleModelChange(modelId, next, agentModel);
    },
    { meta: false, shift: true },
  );
  // Ctrl+M cycles the configured model subset, leaving each model's own
  // remembered effort alone, same as picking it from the menu.
  useHotkey("m", () => {
    if (cycleModels.length === 0) return;
    const index = cycleModels.findIndex(
      (m) =>
        m.id === modelId &&
        (m.id !== "dray" ||
          (m.agentModel?.provider === agentModel?.provider && m.agentModel?.id === agentModel?.id)),
    );
    // A one-model cycle can still bring an excluded current model back into the
    // configured set; once it is selected, there is nowhere else to move.
    if (cycleModels.length === 1 && index === 0) return;
    const next = cycleModels[(index + 1) % cycleModels.length];
    handleModelChange(next.id, null, next.agentModel);
  });
  const fullscreen = useFullscreen();
  useVibrancy(fullscreen);

  return (
    <TooltipProvider>
    <DiffWorkerPool pair={codeThemePair}>
    <AppShell
      centered={!selectedSession}
      sidebar={
        <Sidebar
          items={searchedSessions}
          search={search}
          onSearchChange={setSearch}
          projects={projects}
          statusBySession={statusBySession}
          askingSessions={askingSessions}
          prFor={prMarks.prFor}
          selectedSessionId={selectedSessionId}
          onOpenSettings={() => setSettingsOpen(true)}
          onOpenAnalytics={() => setAnalyticsOpen(true)}
          onOpenDeveloper={() => setDeveloperOpen(true)}
          onSelect={handleSelectSessionIndexItem}
          onNewSession={handleNewSession}
          onDetach={detachSession}
          onSetFlags={async (sessionId, flags) => {
            const updated = await setSessionFlags(sessionId, flags);
            if (!updated) return;
            if (flags.archived === true) {
              playCelebration();
            }
            // Settling the open session leaves nothing to look at but the
            // unsettle bar, so it goes back to the empty composer instead.
            if (flags.archived === true && sessionId === selectedSessionId) {
              handleNewSession();
            } else if (flags.archived === false) {
              // Unsettling only happens from the settled list, and the row
              // just left it — follow it back to where it landed, onto the
              // row itself.
              if (showArchived) setShowArchived(false);
              void handleSelectSessionIndexItem(sessionId);
            }
          }}
          onFork={forkSession}
          onDelete={deleteSession}
          showArchived={showArchived}
          update={update}
        />
      }
      header={
        <header
          className="flex h-(--titlebar-h) shrink-0 items-center gap-2 px-3"
          // `deep`, not bare: bare drags only on direct hits, so every label
          // inside this row was a dead strip in a titlebar that looks uniform.
          // Buttons still block on their own — Tauri stops walking up at any
          // clickable element that carries no attribute of its own.
          data-tauri-drag-region="deep"
        >
          <SessionHeader
            session={selectedSession}
            branch={prBranch}
            className="flex-1"
          />
        </header>
      }
      footer={
        <ChatInput
          onSend={handleSendMsg}
          commands={slashSkills}
          models={visibleModels}
          modelId={modelId}
          agentModel={agentModel}
          effort={effort}
          onModelChange={handleModelChange}
          onNewSession={handleNewSession}
          onSettle={settleCurrentSession}
          cwd={composerCwd}
          projectPath={projectPath}
          onStop={handleInterrupt}
          onCancelQueued={handleCancelQueued}
          queuedCount={queuedMessages.length}
          busy={busy}
          sessionId={selectedSessionId}
          isNewTask={!selectedSession}
          error={error}
          onDismissError={() => setError(null)}
          archived={selectedSession?.archived ?? false}
          onUnarchive={() =>
            selectedSessionId && setSessionFlags(selectedSessionId, { archived: false })
          }
          handoff={
            <HandoffRow
              actions={handoffActions(workStatus, sessionHasPr)}
              // Straight out as a prompt, exactly as if it had been typed. A
              // turn already running queues it, like any other send.
              onSend={(prompt) => void handleSendMsg(prompt)}
              onPush={() => void push()}
              pushing={pushing}
              disabled={!selectedSessionId}
            />
          }
          toolbar={
            <ComposerToolbar
              models={visibleModels}
              modelId={modelId}
              agentModel={agentModel}
              effort={effort}
              onModelChange={handleModelChange}
              projects={projects}
              projectPath={projectPath}
              onSelectProject={handleSelectProject}
              onAttachProject={handleAttachProject}
              onRenameProject={handleRenameProject}
              onDeleteProject={handleDeleteProject}
              onReorderProjects={handleReorderProjects}
              branches={branches}
              branch={branch}
              onSelectBranch={handleSelectBranch}
              pendingBranch={pendingBranch}
              onConfirmBranchSwitch={(stash) =>
                pendingBranch && runCheckout(pendingBranch, stash)
              }
              onCancelBranchSwitch={() => setPendingBranch(null)}
              useWorktree={useWorktree}
              worktreeAvailable={worktreeAvailable}
              worktreeUnavailableReason={worktreeUnavailableReason}
              onToggleWorktree={() => setUseWorktree((v) => !v)}
              onAttach={() => void pickAttachments(selectedSessionId)}
              contextUsage={contextUsage}
              isNewSession={!selectedSessionId}
            />
          }
        />
      }
    >
      <Chat
        session={selectedSession}
        streamingBlock={
          selectedSessionId ? streamingContentBlock[selectedSessionId] ?? null : null
        }
        onOpenSession={(id) => void handleSelectSessionIndexItem(id)}
        onAnswerQuestions={handleAnswerQuestions}
        busy={busy}
        compacting={compacting}
        queuedMessages={queuedMessages}
        working={working}
      />
    </AppShell>
    {/* Outside `AppShell` on purpose: it is fixed to the window rather than
        placed in the layout, and the shell has no slot that isn't a pane. */}
    <NoticeStack
      onSelect={(id) => void handleSelectSessionIndexItem(id)}
    />
    <QuitDialog />
    {/* Mounted here rather than in the sidebar, which unmounts whole when it
        collapses and would take ⌘, with it. */}
    {import.meta.env.DEV && (
      <DeveloperDialog
        open={developerOpen}
        onOpenChange={setDeveloperOpen}
        onFakeUpdateAvailable={update.fakeUpdateAvailable}
      />
    )}
    <AnalyticsDialog
      open={analyticsOpen}
      onOpenChange={setAnalyticsOpen}
      displayMode={usageDisplayMode}
    />
    <SettingsDialog
      open={settingsOpen}
      onOpenChange={setSettingsOpen}
      showArchived={showArchived}
      onShowArchivedChange={setShowArchived}
      usageDisplayMode={usageDisplayMode}
      onUsageDisplayModeChange={setUsageDisplayMode}
      noProjectPath={noProjectPath}
      onNoProjectPathChange={setNoProjectPath}
      models={models}
      visibleModelKeys={visibleModelKeys}
      onVisibleModelKeysChange={setVisibleModelKeys}
      cycleModelKeys={cycleModelKeys}
      onCycleModelKeysChange={setCycleModelKeys}
      cycleEfforts={cycleEfforts}
      onCycleEffortsChange={setCycleEfforts}
      autoDownloadUpdates={autoDownloadUpdates}
      onAutoDownloadUpdatesChange={setAutoDownloadUpdates}
      titleModels={titleModelOptions}
      titleModelId={titlePrefs.modelId}
      titleAgentModel={titlePrefs.agentModel}
      titleEffort={titlePrefs.effort}
      onTitleModelChange={handleTitleModelChange}
      checkingForUpdates={update.checking}
      onCheckForUpdates={update.checkForUpdates}
    />
    </DiffWorkerPool>
    </TooltipProvider>
  );
}

export default App;
