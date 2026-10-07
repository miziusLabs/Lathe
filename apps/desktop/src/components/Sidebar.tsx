import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import {
  BarChart3,
  Check,
  CircleDashed,
  Hammer,
  GitBranchPlus,
  Unlink,
  // Pin,
  Plus,
  Search,
  Settings,
  Trash2,
  Undo2,
} from "lucide-react";
import { ThinkingOrb } from "thinking-orbs";

import PrStateIcon, { prStateLabel } from "@/components/PrStateIcon";
import UpdateNotice, { type UpdateController } from "@/components/UpdateNotice";
import { Button } from "@/components/ui/button";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { Kbd, KbdGroup } from "@/components/ui/kbd";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useFullscreen } from "@/hooks/useFullscreen";
import { useLocalStorage } from "@/hooks/useLocalStorage";
import { useModifierPressed } from "@/hooks/useHotkey";
import { basename, isToday, relativeTime } from "@/lib/format";
import { sessionBranch } from "@/lib/pr";
import { IS_MAC } from "@/lib/platform";
import { cn } from "@/lib/utils";
import type {
  PrMark,
  Project,
  SessionIndexItem,
  SessionStatus,
} from "@/types/events";

type SidebarProps = {
  items: SessionIndexItem[];
  // The live search query. Owned by the caller so the list and keyboard
  // navigation can consume the same filtered array.
  search: string;
  onSearchChange: (query: string) => void;
  // The live status of every session the app has heard about this run. Wins over
  // the item's own field, which is only as fresh as the last list fetch.
  statusBySession: Record<string, SessionStatus>;
  // Sessions standing still behind a question. Kept apart from
  // `statusBySession` rather than folded into it: the backend's status machine
  // still reads these as `in_progress`, and it is right to — the turn is open,
  // it is only the agent that has stopped.
  askingSessions: Set<string>;
  /// The pull request this session's branch is marked with — open, draft or
  /// merged — or nothing. A lookup rather than a field on the item, because
  /// pull requests are read per repo and the index knows nothing about them.
  prFor: (repoPath: string, branch: string | null) => PrMark | undefined;
  selectedSessionId: string | null;
  onOpenSettings: () => void;
  onOpenAnalytics: () => void;
  onOpenDeveloper: () => void;
  onSelect: (sessionId: string) => Promise<void>;
  onNewSession: () => void;
  onSetFlags: (
    sessionId: string,
    flags: { archived?: boolean; pinned?: boolean },
  ) => Promise<void>;
  onFork: (sessionId: string, worktree: boolean) => Promise<void>;
  onDelete: (sessionId: string) => Promise<void>;
  onDetach: (sessionId: string) => Promise<void>;
  showArchived: boolean;
  projects: Project[];
  update: UpdateController;
};

/// One drawn row: the session, how deep it sits, and the flags its connector
/// rails are drawn from.
export type SessionListRow = {
  item: SessionIndexItem;
  /// Levels below the top. 0 is a root and draws no connector at all.
  depth: number;
  /// One entry per level *above* this row, saying whether that level's rail
  /// carries on below it. The last entry is this row's own parent — false there
  /// closes the line at the elbow, so a rail never runs on into an unrelated
  /// session.
  guides: boolean[];
  /// This row opens a rail of its own for the rows under it.
  opens: boolean;
};

/// The order the list is drawn in, with a session spawned by an agent sitting
/// directly under the one that spawned it — and the depth and guide flags each
/// row's rails are drawn from.
///
/// Order and rails come out of this one walk on purpose. Computing the rails
/// separately would let the shape drawn down the left edge disagree with the
/// order the rows are actually in, and a rail pointing at the wrong row says
/// something false about who spawned what.
///
/// A child whose parent is not in `items` is drawn at the top level rather than
/// hidden. That is the ordinary case, not an edge one: the parent may be
/// archived, filtered to another project, or deleted outright, and a row that
/// vanished with it would be unreachable — so it draws as a root, with no rail
/// reaching for a parent that isn't there.
///
/// Depth is drawn rather than flattened: the cap allows a spawned session to
/// spawn, so a grandchild exists, and it hangs off its own parent's rail while
/// that parent still hangs off the root's.
export function sessionRows(items: SessionIndexItem[]): SessionListRow[] {
  const byRecency = (a: SessionIndexItem, b: SessionIndexItem) =>
    Date.parse(b.modified) - Date.parse(a.modified);

  const present = new Map(items.map((i) => [i.sessionId, i]));
  const parentOf = (i: SessionIndexItem) => {
    if (!i.parentSessionId) return null;
    const parent = present.get(i.parentSessionId);
    return parent ? i.parentSessionId : null;
  };

  const children = new Map<string, SessionIndexItem[]>();
  for (const item of items) {
    const parent = parentOf(item);
    if (!parent) continue;
    const group = children.get(parent);
    if (group) group.push(item);
    else children.set(parent, [item]);
  }

  // Depth-first rather than one pass over roots and their direct children: the
  // depth cap allows a spawned session to spawn, so a grandchild exists, and a
  // single pass emitted neither it nor anything below it — the row simply
  // vanished from the sidebar. Pinned by a test.
  const rows: SessionListRow[] = [];
  const seen = new Set<string>();

  const walk = (item: SessionIndexItem, depth: number, guides: boolean[]) => {
    if (seen.has(item.sessionId)) return;
    seen.add(item.sessionId);
    const row: SessionListRow = { item, depth, guides, opens: false };
    rows.push(row);

    // Filtered before the walk, not during it: the last *drawn* child is what
    // closes the rail, and a cycle can leave a listed child already emitted
    // elsewhere — counting it would run the rail on past the row it ends at.
    const kids = (children.get(item.sessionId) ?? [])
      .filter((child) => !seen.has(child.sessionId))
      .sort(byRecency);
    const before = rows.length;
    kids.forEach((child, i) => {
      walk(child, depth + 1, [...guides, i < kids.length - 1]);
    });
    row.opens = rows.length > before;
  };

  for (const root of [...items].filter((i) => !parentOf(i)).sort(byRecency)) {
    walk(root, 0, []);
  }

  // A cycle in the index reaches no root, so its rows are still unemitted here.
  // Appended rather than dropped: the sidebar losing a session is worse than
  // drawing a strange order, and `seen` is what stops the walk itself hanging.
  for (const stranded of [...items].sort(byRecency)) {
    walk(stranded, 0, []);
  }

  return rows;
}

/// One project's run of rows, in the order the sidebar draws them.
export type SessionGroup = {
  projectPath: string;
  rows: SessionListRow[];
};

/// [`sessionRows`] gathered under the project each row's *root* belongs to.
///
/// Grouped on the root rather than on the row: a spawned session hangs off its
/// parent wherever its own `projectPath` points, and splitting one nest across
/// two headings would draw a child under a parent that isn't there.
///
/// A project with no session opens no group, which is the whole of "don't show
/// an empty project" — headings come from the sessions present, never from the
/// attached list.
///
/// **Group order is the project list's own**, not the recency of the sessions
/// inside it. Ordering on the newest session read well for one screenshot and
/// badly in use: every reply to any session lifted its whole project over the
/// others, so headings the eye had learned the position of moved while a turn
/// was running. The project list only reorders when a project is *selected*,
/// which is the reader's own act. A project no longer attached still has
/// sessions to draw, so it keeps first-appearance order after the attached
/// ones.
///
/// Rows inside a group stay newest-first, and a list already narrowed to one
/// project comes back in exactly the order it had before grouping existed.
export function sessionGroups(
  items: SessionIndexItem[],
  projects: Project[] = [],
): SessionGroup[] {
  const groups: SessionGroup[] = [];
  const byPath = new Map<string, SessionGroup>();
  let current: SessionGroup | undefined;

  for (const row of sessionRows(items)) {
    // Every subtree the walk emits opens with its own root, so a depth-0 row is
    // where one run ends and the next begins.
    if (row.depth === 0 || !current) {
      const path = row.item.projectPath;
      current = byPath.get(path);
      if (!current) {
        current = { projectPath: path, rows: [] };
        byPath.set(path, current);
        groups.push(current);
      }
    }
    current.rows.push(row);
  }

  // Unattached sorts to the end rather than to the front, and `sort` is stable,
  // so those keep the order they were built in.
  const rank = new Map(projects.map((p, i) => [p.path, i]));
  const place = (group: SessionGroup) =>
    rank.get(group.projectPath) ?? Number.MAX_SAFE_INTEGER;

  return groups.sort((a, b) => place(a) - place(b));
}

/// The order alone, for callers that only step through it.
///
/// Exported because the ⌘⇧↑/↓ shortcut walks the same sequence, and a second
/// comparator would let the two disagree about which row is "next". Grouped for
/// the same reason: the shortcut has to step past a heading the way
/// the eye does, so it takes the same project list the headings are ordered by.
export function sortSessions(
  items: SessionIndexItem[],
  projects: Project[] = [],
): SessionIndexItem[] {
  return sessionGroups(items, projects).flatMap((group) =>
    group.rows.map((row) => row.item),
  );
}

/// The rows left on screen by a title search. Matching happens before grouping
/// so empty project headings disappear and children whose parents do not match
/// remain visible at the top level.
export function filterSessions(
  items: SessionIndexItem[],
  query: string,
): SessionIndexItem[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return items;
  return items.filter((item) => item.title.toLowerCase().includes(needle));
}

/// Whether a row has a parent to detach from, judged the same way
/// [`sessionRows`] places it — a parent that isn't on screen means the row is
/// drawn at the top level, so the menu item can never offer to cut a link the
/// list doesn't draw.
export function isNested(item: SessionIndexItem, items: SessionIndexItem[]): boolean {
  return Boolean(
    item.parentSessionId && items.some((i) => i.sessionId === item.parentSessionId),
  );
}

/// Opens analytics from the sidebar titlebar strip.
export function AnalyticsButton({ onOpen }: { onOpen: () => void }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="icon-sm"
          onClick={onOpen}
          aria-label="Analytics"
          className="text-muted-foreground"
        >
          <BarChart3 className="size-4" />
        </Button>
      </TooltipTrigger>
      <TooltipContent side="right">
        Analytics
        <KbdGroup>
          <Kbd>{IS_MAC ? "⌘" : "Ctrl"}</Kbd>
          <Kbd>T</Kbd>
        </KbdGroup>
      </TooltipContent>
    </Tooltip>
  );
}

export function SettingsButton({ onOpen }: { onOpen: () => void }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="icon-sm"
          onClick={onOpen}
          aria-label="Settings"
          className="text-muted-foreground"
        >
          <Settings className="size-4" />
        </Button>
      </TooltipTrigger>
      <TooltipContent side="right">
        Settings
        <KbdGroup>
          <Kbd>{IS_MAC ? "⌘" : "Ctrl"}</Kbd>
          <Kbd>,</Kbd>
        </KbdGroup>
      </TooltipContent>
    </Tooltip>
  );
}

/// Opens development-only tools. The caller gates this control with DEV so it
/// cannot appear in a production build.
export function DeveloperButton({ onOpen }: { onOpen: () => void }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="icon-sm"
          onClick={onOpen}
          aria-label="Developer"
          className="text-muted-foreground"
        >
          <Hammer className="size-4" />
        </Button>
      </TooltipTrigger>
      <TooltipContent side="right">Developer</TooltipContent>
    </Tooltip>
  );
}

/// Marks a dev build so it can't be mistaken for the installed app. Gated on
/// `import.meta.env.DEV`, which Vite folds to a constant — the badge and this
/// component are dropped from a production bundle entirely.
export function DevBadge({ className }: { className?: string }) {
  return (
    <span
      className={cn(
        "rounded bg-orange-500/15 px-1.5 py-0.5 font-mono text-[10px] leading-none font-medium tracking-wide text-orange-500 uppercase",
        className,
      )}
    >
      Dev
    </span>
  );
}

const DEFAULT_SIDEBAR_WIDTH = 240;
const MIN_SIDEBAR_WIDTH = 192;
const MAX_SIDEBAR_WIDTH = 480;
const SIDEBAR_RESIZE_STEP = 8;

function clampSidebarWidth(value: unknown): number {
  const width = typeof value === "number" && Number.isFinite(value)
    ? value
    : DEFAULT_SIDEBAR_WIDTH;
  return Math.min(MAX_SIDEBAR_WIDTH, Math.max(MIN_SIDEBAR_WIDTH, width));
}

export default function Sidebar({
  items,
  search,
  onSearchChange,
  statusBySession,
  askingSessions,
  prFor,
  selectedSessionId,
  onSelect,
  onNewSession,
  onSetFlags,
  onFork,
  onDelete,
  showArchived,
  projects,
  onDetach,
  onOpenSettings,
  onOpenAnalytics,
  onOpenDeveloper,
  update,
}: SidebarProps) {
  const fullscreen = useFullscreen();
  const modifierPressed = useModifierPressed();
  const [searching, setSearching] = useState(false);
  const [persistedWidth, setPersistedWidth] = useLocalStorage(
    "ade.sidebarWidth",
    DEFAULT_SIDEBAR_WIDTH,
  );
  const [sidebarWidth, setSidebarWidth] = useState(() =>
    clampSidebarWidth(persistedWidth),
  );
  const sidebarWidthRef = useRef(sidebarWidth);
  const asideRef = useRef<HTMLElement>(null);
  const resizeRef = useRef<{
    pointerId: number;
    startX: number;
    startWidth: number;
  } | null>(null);
  const [resizing, setResizing] = useState(false);

  const updateSidebarWidth = (next: number, persist = false) => {
    const width = clampSidebarWidth(next);
    sidebarWidthRef.current = width;
    setSidebarWidth(width);
    if (persist) setPersistedWidth(width);
  };

  // Keep text and the cursor stable while the pointer crosses the conversation.
  // Pointer capture keeps delivering the drag to the narrow handle itself; these
  // document styles make the rest of the window still feel like one splitter.
  useEffect(() => {
    if (!resizing) return;
    const root = document.documentElement;
    const previousCursor = root.style.cursor;
    const previousUserSelect = root.style.userSelect;
    root.style.cursor = "col-resize";
    root.style.userSelect = "none";
    return () => {
      root.style.cursor = previousCursor;
      root.style.userSelect = previousUserSelect;
    };
  }, [resizing]);

  const closeSearch = () => {
    setSearching(false);
    onSearchChange("");
  };

  // Recency-ordered, with agent-spawned sessions nested under the one that
  // spawned them, and each row carrying the flags its connector rails are drawn
  // from — then gathered under the project it belongs to, in the project list's
  // own order, so the list reads as one group per repo and the headings hold
  // still while its sessions work.
  const groups = useMemo(() => sessionGroups(items, projects), [items, projects]);
  const rowCount = useMemo(
    () => groups.reduce((n, group) => n + group.rows.length, 0),
    [groups],
  );
  const shortcutIndexBySessionId = useMemo(() => {
    const index = new Map<string, number>();
    groups
      .flatMap((group) => group.rows)
      .slice(0, 9)
      .forEach(({ item }, rowIndex) => index.set(item.sessionId, rowIndex + 1));
    return index;
  }, [groups]);

  // A session under a repo nobody attached still has a project, so the folder
  // name stands in rather than the heading being dropped — the row has to sit
  // under something.
  const projectName = useMemo(() => {
    const named = new Map(projects.map((p) => [p.path, p.name]));
    return (path: string) =>
      path === "" ? "No Project" : (named.get(path) ?? basename(path));
  }, [projects]);

  // A filtered list that comes up empty is different from an empty app, and
  // saying "No tasks yet" over a query reads as data loss.
  const emptyText = search.trim()
    ? `No tasks matching "${search.trim()}".`
    : showArchived
      ? "Nothing settled yet."
      : "No tasks yet.";

  return (
    <aside
      ref={asideRef}
      className="relative flex shrink-0 flex-col bg-composer shadow-sm"
      style={{ width: sidebarWidth }}
    >
      <div
        role="separator"
        aria-label="Resize sidebar"
        aria-orientation="vertical"
        aria-valuemin={MIN_SIDEBAR_WIDTH}
        aria-valuemax={MAX_SIDEBAR_WIDTH}
        aria-valuenow={Math.round(sidebarWidth)}
        tabIndex={0}
        className={cn(
          "group absolute inset-y-0 right-0 z-20 w-1 translate-x-1/2 cursor-col-resize touch-none outline-none",
          resizing && "cursor-col-resize",
        )}
        onPointerDown={(event) => {
          if (event.button !== 0) return;
          event.preventDefault();
          resizeRef.current = {
            pointerId: event.pointerId,
            startX: event.clientX,
            startWidth:
              asideRef.current?.getBoundingClientRect().width ?? sidebarWidthRef.current,
          };
          event.currentTarget.setPointerCapture(event.pointerId);
          setResizing(true);
        }}
        onPointerMove={(event) => {
          const resize = resizeRef.current;
          if (!resize || resize.pointerId !== event.pointerId) return;
          updateSidebarWidth(resize.startWidth + event.clientX - resize.startX);
        }}
        onPointerUp={(event) => {
          if (resizeRef.current?.pointerId !== event.pointerId) return;
          resizeRef.current = null;
          setPersistedWidth(sidebarWidthRef.current);
          setResizing(false);
          if (event.currentTarget.hasPointerCapture(event.pointerId)) {
            event.currentTarget.releasePointerCapture(event.pointerId);
          }
        }}
        onPointerCancel={(event) => {
          if (resizeRef.current?.pointerId !== event.pointerId) return;
          resizeRef.current = null;
          setPersistedWidth(sidebarWidthRef.current);
          setResizing(false);
        }}
        onLostPointerCapture={(event) => {
          if (resizeRef.current?.pointerId !== event.pointerId) return;
          resizeRef.current = null;
          setPersistedWidth(sidebarWidthRef.current);
          setResizing(false);
        }}
        onKeyDown={(event) => {
          let next = sidebarWidthRef.current;
          if (event.key === "ArrowLeft") next -= SIDEBAR_RESIZE_STEP;
          else if (event.key === "ArrowRight") next += SIDEBAR_RESIZE_STEP;
          else if (event.key === "Home") next = MIN_SIDEBAR_WIDTH;
          else if (event.key === "End") next = MAX_SIDEBAR_WIDTH;
          else return;
          event.preventDefault();
          updateSidebarWidth(next, true);
        }}
      >
        <span
          aria-hidden
          className={cn(
            "absolute inset-y-0 left-1/2 w-px -translate-x-1/2 bg-transparent transition-colors group-hover:bg-sidebar-ring group-focus-visible:bg-sidebar-ring",
            resizing && "bg-sidebar-ring",
          )}
        />
      </div>

      {/* The toggle shares this strip with the traffic lights, so it sits at the
          right to clear them — except in fullscreen, where they're gone and the
          left edge is free. */}
      <div
        className={cn(
          "flex h-(--titlebar-h) shrink-0 items-center px-2",
          // Left-aligned, the toggle's larger icon would sit 2px inside the
          // buttons below it; nudge it out so every icon shares one edge.
          fullscreen ? "justify-start pl-2" : "justify-end",
        )}
        data-tauri-drag-region="deep"
      >
        {import.meta.env.DEV && <DevBadge className="mr-auto" />}
        {fullscreen ? (
          <>
            {import.meta.env.DEV && <DeveloperButton onOpen={onOpenDeveloper} />}
            <AnalyticsButton onOpen={onOpenAnalytics} />
            <SettingsButton onOpen={onOpenSettings} />
          </>
        ) : (
          <>
            <AnalyticsButton onOpen={onOpenAnalytics} />
            <SettingsButton onOpen={onOpenSettings} />
            {import.meta.env.DEV && <DeveloperButton onOpen={onOpenDeveloper} />}
          </>
        )}
      </div>

      {/* `px-1.5` on the buttons rather than `size="sm"`'s `px-2.5`, so their
          icons land on the same 12px inset as the titlebar controls. */}
      <div className="flex flex-col gap-px px-2">
        <Button
          variant="ghost"
          size="sm"
          onClick={onNewSession}
          className="w-full justify-start px-1.5 text-ui"
        >
          <Plus />
          New Task
          <KbdGroup className="ml-auto">
            <Kbd>{IS_MAC ? "⌘" : "Ctrl"}</Kbd>
            <Kbd>N</Kbd>
          </KbdGroup>
        </Button>

        {searching ? (
          <div className="flex h-7 w-full items-center gap-1 border border-transparent px-1.5 text-ui">
            <Search className="size-3.5 shrink-0" />
            <input
              autoFocus
              type="text"
              value={search}
              placeholder="Search"
              aria-label="Search tasks"
              onChange={(e) => onSearchChange(e.target.value)}
              onKeyDown={(e) => {
                if (e.key !== "Escape") return;
                e.preventDefault();
                closeSearch();
              }}
              onBlur={() => {
                if (!search) setSearching(false);
              }}
              className="min-w-0 flex-1 bg-transparent outline-none placeholder:text-muted-foreground"
            />
          </div>
        ) : (
          <Button
            variant="ghost"
            size="sm"
            onClick={() => setSearching(true)}
            className="w-full justify-start px-1.5 text-ui"
          >
            <Search />
            Search
          </Button>
        )}
      </div>

      {/* Keep the session cards inset from the edges of the scroll list. */}
      <div className="mt-4 flex min-h-0 flex-1 flex-col gap-px overflow-y-auto pb-3 pl-2 pr-2">
        {rowCount === 0 ? (
          <p className="px-2 py-6 text-ui text-muted-foreground">{emptyText}</p>
        ) : (
          groups.map((group, index) => (
            <Fragment key={group.projectPath}>
              {/* Name only — no icon, no count: the heading says which repo the
                  run below it is, and anything else on it competes with the
                  titles it introduces. Full path on hover, since two projects
                  can share a folder name. */}
              <div
                title={group.projectPath}
                className={cn(
                  "flex min-h-6 items-center truncate pr-2 pl-2 text-ui text-muted-foreground/70",
                  index > 0 && "mt-3",
                )}
              >
                {projectName(group.projectPath)}
              </div>

              {group.rows.map(({ item, depth, guides, opens }) => (
                <SessionRow
                  key={item.sessionId}
                  item={item}
                  depth={depth}
                  guides={guides}
                  opens={opens}
                  modifierPressed={modifierPressed}
                  shortcutIndex={shortcutIndexBySessionId.get(item.sessionId)}
                  status={statusBySession[item.sessionId] ?? item.status}
                  asking={askingSessions.has(item.sessionId)}
                  pr={prFor(item.projectPath, sessionBranch(item))}
                  active={item.sessionId === selectedSessionId}
                  // The settled list is a history, and the question asked of it is
                  // "what did I finish today" — so everything older is held back
                  // rather than filtered out. Only there: the active list is a
                  // worklist, where an older row is still open work.
                  faded={showArchived && !isToday(item.modified)}
                  // Nothing refreshes marks over here: the archived view asks for
                  // no repos, so its rows draw from a cache nothing will update.
                  // A stale glyph is the accepted trade; a stale *spinner* is not,
                  // since it animates a claim that something is happening now.
                  marksLive={!showArchived}
                  nested={isNested(item, items)}
                  onSelect={onSelect}
                  onSetFlags={onSetFlags}
                  onFork={onFork}
                  onDelete={onDelete}
                  onDetach={onDetach}
                />
              ))}
            </Fragment>
          ))
        )}

      </div>

      <UpdateNotice controller={update} />
    </aside>
  );
}

/// One hover control on a session row. Stops the click from reaching the row's
/// own handler, so acting on a session never also selects it.
function RowAction({
  label,
  active,
  onClick,
  className,
  children,
}: {
  label: string;
  active: boolean;
  onClick: (e: React.MouseEvent<HTMLButtonElement>) => void;
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label={label}
          aria-pressed={active}
          onClick={(e) => {
            e.stopPropagation();
            onClick(e);
          }}
          // Set here rather than inherited from the row: the UA stylesheet's own
          // `button { cursor: default }` wins over an inherited value.
          className={cn(
            "cursor-pointer",
            active ? "text-foreground" : "text-muted-foreground",
            className,
          )}
        >
          {children}
        </Button>
      </TooltipTrigger>
      <TooltipContent side="bottom">{label}</TooltipContent>
    </Tooltip>
  );
}

/// The fork submenu's rows, in the order they are drawn. The number key that
/// picks one is its position here, so reordering moves the digits with it and
/// there is no second table to fall out of step with the labels.
const FORKS = [
  { label: "Fork here", worktree: false },
  { label: "Fork in new Worktree Session", worktree: true },
] as const;

/// The row's right-click menu. Delete confirms in place — a second surface for
/// two words costs more than it protects, and the menu is already open under the
/// cursor. Both steps live in one `Content` so the menu holds its position
/// across the swap; reanchoring mid-decision would move the target being aimed
/// at.
///
/// `confirming` resets on open rather than on close. The content unmounts either
/// way, but the state lives out here, so without it a cancelled delete would
/// reopen already armed.
///
/// A context menu can't be opened programmatically, so `ContextMenu` takes no
/// `open` — the trigger's own `data-state` is what the row styles off, and there
/// is no second copy of the flag to fall out of step with it.
function RowMenu({
  onFork,
  forkDisabled,
  onDelete,
  onDetach,
  children,
}: {
  onFork: (worktree: boolean) => void;
  /// The session is working. The CLI forks by reading its transcript, which a
  /// live child is still appending to, so a fork taken now can inherit half a
  /// turn. The backend refuses it too — this only saves the trip.
  forkDisabled: boolean;
  onDelete: () => void;
  /// Absent on a row that isn't nested — there is nothing to detach from, and
  /// a disabled item on every row in the list would be noise rather than a
  /// promise of something coming.
  onDetach?: () => void;
  children: React.ReactNode;
}) {
  const [confirming, setConfirming] = useState(false);
  // Radix moves DOM focus onto the sub's own content only once the pointer (or
  // an arrow key) actually enters it — hovering the trigger alone opens the
  // submenu but leaves focus behind on the trigger. Reading `open` instead of
  // focus location is what lets a number fire the moment the submenu is drawn,
  // without the cursor ever crossing into it.
  const [forkOpen, setForkOpen] = useState(false);
  // A digit clicks the row rather than calling `onFork` directly, so closing the
  // menu and restoring focus stay Radix's job — picking by key and picking by
  // mouse then cannot end in different states.
  const forkRefs = useRef<(HTMLDivElement | null)[]>([]);

  return (
    <ContextMenu onOpenChange={(open) => open && setConfirming(false)}>
      <ContextMenuTrigger asChild>{children}</ContextMenuTrigger>

      {/* Portaled, so a click in here never reaches the row's select handler. */}
      {/* Wide enough for the confirm step's two buttons, which is the widest
          thing this menu ever holds — the width is fixed rather than fitted so
          the frame doesn't resize under the cursor when Delete swaps them in. */}
      {/*
          The digit listener lives up here rather than on the sub's own content:
          `SubContent` renders through a portal, so a handler placed there only
          ever fires once focus — not just the open submenu — has moved inside
          it. React still delivers the event here through the component tree
          rather than the DOM one, so this fires the instant the submenu opens,
          whether that happened by hover or by keyboard, and no matter which of
          the two elements the key lands on.
      */}
      <ContextMenuContent
        className="w-48"
        onKeyDown={(e) => {
          if (!forkOpen) return;
          const picked = forkRefs.current[Number(e.key) - 1];
          if (!picked) return;
          // Holds the digit back from Radix's own typeahead, which would
          // otherwise read it as a search letter and jump focus instead.
          e.preventDefault();
          picked.click();
        }}
      >
        {confirming ? (
          <>
            {/* No title in the copy: the menu opens on the row, which stays on
                screen beside it and already names the session. */}
            <p className="px-1.5 py-1 text-ui text-muted-foreground">
              Are you sure?
            </p>

            {/* Items rather than buttons, so selecting either closes the menu on
                its own — nothing here can reopen it, and a stranded confirm step
                is the one state with no way out but Escape. */}
            <div className="mt-1 flex gap-1">
              <ContextMenuItem className="flex-1 justify-center text-ui">
                Cancel
              </ContextMenuItem>
              <ContextMenuItem
                variant="destructive"
                onSelect={onDelete}
                className="flex-1 justify-center bg-destructive/10 text-ui"
              >
                Yes, Delete
              </ContextMenuItem>
            </div>
          </>
        ) : (
          <>
            {/* A submenu because the two forks differ in where the copy
                *runs*, not in what it copies — both carry the whole
                conversation. Flattening them into two top-level items would put
                the rarer choice beside Delete on every row. */}
            <ContextMenuSub onOpenChange={setForkOpen}>
              <ContextMenuSubTrigger
                disabled={forkDisabled}
                className="text-ui"
              >
                <GitBranchPlus />
                Fork
              </ContextMenuSubTrigger>

              {/* Sized by its own rows. Nothing swaps in here, so there is
                  no second layout to hold a width for. */}
              <ContextMenuSubContent>
                {FORKS.map((fork, i) => (
                  <ContextMenuItem
                    key={fork.label}
                    ref={(el) => {
                      forkRefs.current[i] = el;
                    }}
                    className="text-ui"
                    onSelect={() => onFork(fork.worktree)}
                  >
                    {fork.label}
                    <Kbd className="ml-auto">{i + 1}</Kbd>
                  </ContextMenuItem>
                ))}
              </ContextMenuSubContent>
            </ContextMenuSub>

            {onDetach && (
              <ContextMenuItem className="text-ui" onSelect={onDetach}>
                <Unlink />
                Detach from parent
              </ContextMenuItem>
            )}

            {/* `preventDefault` holds the menu open — an item select closes it by
                default, which would take the confirm step down with it. */}
            <ContextMenuItem
              variant="destructive"
              className="text-ui"
              onSelect={(e) => {
                e.preventDefault();
                setConfirming(true);
              }}
            >
              <Trash2 />
              Delete
            </ContextMenuItem>
          </>
        )}
      </ContextMenuContent>
    </ContextMenu>
  );
}

/// Connector geometry, in px from the row's left edge. `RAIL_X` sits just
/// inside the unread rail's own 8px slot, `STEP` is one level of nesting, and
/// `ELBOW` is how far the horizontal reaches before the title starts.
const RAIL_X = 12;
const STEP = 12;
const ELBOW = 10;

function SessionRow({
  item,
  depth,
  guides,
  opens,
  modifierPressed,
  shortcutIndex,
  status,
  asking,
  pr,
  active,
  faded = false,
  marksLive = true,
  onSelect,
  nested = false,
  onSetFlags,
  onFork,
  onDelete,
  onDetach,
}: {
  item: SessionIndexItem;
  /// Levels below the top; 0 draws no connector at all. See [`sessionRows`] —
  /// these three come out of the same walk that ordered the list.
  depth: number;
  guides: boolean[];
  opens: boolean;
  modifierPressed: boolean;
  shortcutIndex?: number;
  status: SessionStatus;
  asking: boolean;
  /// The pull request this row is marked with, already narrowed to one where
  /// the branch carries several. Undefined covers both "no PR" and "we couldn't
  /// ask" — the mark is decoration, so the two read the same.
  pr?: PrMark;
  active: boolean;
  faded?: boolean;
  /// Something is still refreshing this row's mark. False in the archived view,
  /// which asks for no repos — see the call site.
  marksLive?: boolean;
  onSelect: (sessionId: string) => Promise<void>;
  /// This row has a parent in the same list, so 'Detach from parent' is a real
  /// offer. Kept apart from `depth` only because a cyclic index draws a row at
  /// the top level that still has a link worth cutting.
  nested?: boolean;
  onSetFlags: (
    sessionId: string,
    flags: { archived?: boolean; pinned?: boolean },
  ) => Promise<void>;
  onFork: (sessionId: string, worktree: boolean) => Promise<void>;
  onDelete: (sessionId: string) => Promise<void>;
  onDetach: (sessionId: string) => Promise<void>;
}) {
  // The keyboard shortcut can walk the selection past the fold, and `nearest`
  // means a row selected by click — already in view — doesn't scroll at all.
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (active) ref.current?.scrollIntoView({ block: "nearest" });
  }, [active]);

  // The rail this row elbows onto is its parent's, one step to the left of the
  // one it opens for its own children.
  const ownRail = RAIL_X + (depth - 1) * STEP;
  const parentCarriesOn = guides[depth - 1] ?? false;
  // Titles generated from prompts may already carry the app's 60-character
  // ellipsis. Remove that marker so CSS owns the visible truncation and never
  // leaves a second, separated ellipsis in the row.
  const title = item.title.trimEnd().replace(/\s*(?:…|\.{3})$/, "");

  return (
    <RowMenu
      onFork={(worktree) => void onFork(item.sessionId, worktree)}
      forkDisabled={status === "in_progress"}
      onDelete={() => void onDelete(item.sessionId)}
      onDetach={nested ? () => void onDetach(item.sessionId) : undefined}
    >
      {/* A button can't nest a button, so the row is a div with a click handler
          and the pin/settle controls are the only real buttons inside it. */}
      <div
        ref={ref}
        role="button"
        tabIndex={0}
        onClick={() => void onSelect(item.sessionId)}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            void onSelect(item.sessionId);
          }
        }}
        className={cn(
          // No vertical padding: the 24px hover buttons are the tallest thing in
          // the row, so they'd add to any padding here and make the row grow the
          // moment these controls landed. `min-h` keeps the height when they're
          // the only thing not rendered — an empty row still matches a populated
          // one.
          // No left padding and no `gap`: the unread rail's own 8px slot is the
          // indent that padding used to provide, so the title sits exactly where
          // it always did and the rail can still touch the row's edge. The one
          // gap that remains — title to hover controls — is `pl-2` on that slot.
          "group relative flex min-h-7 w-full cursor-pointer items-center rounded-md pl-0 pr-2",
          // Opacity is in the transition list for the faded rows below. It has
          // to ride the same declaration: `transition-colors` and
          // `transition-opacity` both set `transition-property`, so `cn` would
          // merge one away and the row would lose its hover fill animation.
          "transition-[color,background-color,opacity]",
          // Held back rather than hidden, and brought back to full strength the
          // moment the row is reached for — the fade sorts the list at a glance
          // and must not make an old row harder to read once it's the one being
          // used.
          faded &&
            !active &&
            "opacity-50 hover:opacity-100 focus-visible:opacity-100 data-[state=open]:opacity-100",
          "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-sidebar-ring",
          // `data-state` is the trigger's, set on this element by
          // `ContextMenuTrigger asChild` — an open menu holds the row lit, since
          // the cursor is over the menu rather than the row it belongs to. In the
          // inactive branch only: the selected row's fill is already stronger.
          active
            ? "bg-sidebar-accent text-sidebar-accent-foreground"
            : "text-sidebar-foreground/80 hover:bg-sidebar-accent/50 data-[state=open]:bg-sidebar-accent/50",
        )}
      >
        {/* Two things put a mark here, and both are the reader's to clear: the
            session finished and hasn't been read, or it has stopped and is
            waiting on an answer. Working is shown on the right instead, in place
            of the timestamp — this rail is the "over to you" mark alone, so it
            keeps the left edge it can be scanned down.

            Waiting wins where both could apply, and it is the command yellow
            against the finished green: one is news that will keep, the other is
            an agent standing still until it is dealt with. `--accent-command`
            rather than a fresh amber, so the app keeps to one yellow — it
            already means "this is for you" wherever a slash command is drawn.

            The slot is always here and the rail inside it is what comes and
            goes: a mark that reflows the title would shift the text of a row
            just because its agent finished. A fixed height rather than the row's
            own, so the rail reads the same length whatever the row grows to. */}
        <span className="flex w-2 shrink-0 items-center self-stretch">
          {(asking || status === "completed") && (
            <span
              role="img"
              aria-label={asking ? "Waiting for you" : "Unread"}
              className={cn(
                "h-3 w-0.5 rounded-[1px]",
                asking ? "bg-accent-command" : "bg-emerald-500",
              )}
            />
          )}
        </span>

        {/* The lineage, drawn as rails: this row elbows onto its parent's, and
            an ancestor's carries straight through to the last row of its
            subtree. Aria-hidden and never a target — it is a picture of the
            list's own shape, and a screen reader reads the rows in the order
            they are drawn anyway.

            Every piece sits on its own pixel and no two overlap.
            `--sidebar-border` is white at 8%, so two segments sharing a column
            stack to ~15% and read as a bright patch halfway down the rail. */}

        {/* One pass-through per ancestor above this row's own parent whose line
            is still open, each on its own column. */}
        {guides.slice(0, -1).map(
          (open, level) =>
            open && (
              <span
                key={level}
                aria-hidden
                className="pointer-events-none absolute top-0 -bottom-px w-px bg-sidebar-border"
                style={{ left: RAIL_X + level * STEP }}
              />
            ),
        )}

        {depth > 0 && (
          <>
            {/* Stops at the elbow unless the parent's rail carries on below —
                never a full-height line with the corner drawn over half of it.
                Rows sit in a `gap-px` column, so a piece that carries on has to
                reach 1px past its own bottom edge or the rail reads dashed. */}
            <span
              aria-hidden
              className="pointer-events-none absolute top-0 w-px bg-sidebar-border"
              style={{
                left: ownRail,
                height: parentCarriesOn ? "calc(100% + 1px)" : "50%",
              }}
            />
            {/* Square corner, started one pixel clear of the vertical's own
                column. */}
            <span
              aria-hidden
              className="pointer-events-none absolute h-px bg-sidebar-border"
              style={{ left: ownRail + 1, top: "50%", width: ELBOW - 5 }}
            />
          </>
        )}

        {/* The rail this row opens for the rows under it, from its own centre
            down. Without it a parent's line would start a row late. */}
        {opens && (
          <span
            aria-hidden
            className="pointer-events-none absolute -bottom-px w-px bg-sidebar-border"
            style={{ left: RAIL_X + depth * STEP, top: "50%" }}
          />
        )}

        {/* A fixed slot rather than padding, so every row at a level starts its
            title on the same column whatever else the row is carrying. */}
        {depth > 0 && (
          <span aria-hidden className="shrink-0" style={{ width: ownRail + ELBOW - 8 }} />
        )}

        {/* Ahead of the title, and it takes no room when there is none — unlike
            the rail beside it, which holds its slot. The two differ because the
            rail comes and goes on the *same* row as its agent works, where a
            branch either has a pull request or does not: a row that never gets
            one would otherwise pay for the mark forever, and most never do.

            The rail keeps the outer edge because it is the "over to you" mark
            and clears when the reader deals with it, where this is a standing
            fact about the branch.

            Glyph and colour are [PrStateIcon]'s, shared with the panel's own
            header — the mark and the thing it points at have to match, and two
            copies of that table drift on exactly the state nobody was looking
            at. Emerald open, muted draft, purple merged, and red wherever CI
            has failed on one still open.

            Merged earns a mark for a different reason than the other two do.
            They say "this branch has somewhere to land"; it says the work
            landed, so the row is one to archive — which is the question asked
            of this list at the end of a day, and until now had to be answered
            by opening every session in turn.

            `title` rather than a tooltip: this is a decoration on a row that is
            itself a control, and a tooltip on it would open every time the
            cursor crossed the list. Number first and the state after it, since
            the state is a clause now that a failure can extend it. */}
        {pr && (
          <span
            className="mr-1 flex shrink-0 items-center"
            title={`Pull request #${pr.number} · ${prStateLabel(pr).toLowerCase()}`}
          >
            <PrStateIcon pr={pr} strokeWidth={1.5} />
          </span>
        )}

        <span className="min-w-0 flex-1 line-clamp-1 text-ui">{title}</span>

        {/* Stable trailing slot for the title boundary and row metadata. The
            long title truncates before either one and nothing reflows on hover.
            The indicator and hover action share the same position and crossfade on
            `opacity` over the same duration, so they never both read at once;
            its platform-sized minimum keeps the title's trailing space stable
            whether the shortcut hint is visible or not. */}
        <div
          className={cn(
            "relative flex shrink-0 items-center justify-end self-stretch",
            IS_MAC ? "min-w-8" : "min-w-10",
          )}
        >
          {/* The indicator is centered in the exact box occupied by Settle.
              Keeping this box fixed gives the title one predictable ellipsis
              boundary, regardless of timestamp, orb, or shortcut. */}
          <span
            className={cn(
              "pointer-events-none absolute inset-y-0 flex w-6 items-center justify-center text-ui text-muted-foreground group-hover:opacity-0 group-data-[state=open]:opacity-0",
              modifierPressed && shortcutIndex ? "-right-[3px]" : "-right-1.5",
            )}
          >
            {/* The slot shows one value at a time: ⌘ navigation first, then a
                running check, then the working orb, and finally the timestamp.
                Centering each value keeps replacements aligned with Settle;
                the shortcut moves into the row's trailing padding with it. */}
            {modifierPressed && shortcutIndex ? (
              <Kbd className="mr-[3px] shrink-0 whitespace-nowrap">{`${IS_MAC ? "⌘" : "Ctrl"} ${shortcutIndex}`}</Kbd>
            ) : marksLive && pr?.checksState === "RUNNING" ? (
              <CircleDashed
                className="size-3.5 animate-spin text-accent-command [animation-duration:3s]"
                strokeWidth={1.5}
                aria-label="Checks running"
              />
            ) : status === "in_progress" ? (
              <ThinkingOrb
                state="listening"
                size={20}
                theme="dark"
                aria-label="Working"
              />
            ) : (
              <span className="mr-[3px]">{relativeTime(item.modified)}</span>
            )}
          </span>

          {/* `opacity-0` rather than `hidden`: shadcn's button base sets
              `inline-flex`, and Tailwind emits that after `hidden` at equal
              specificity, so a `display` utility here silently loses.
              `pointer-events-none` keeps the invisible buttons unclickable. */}
          <div className="pointer-events-none relative flex items-center gap-0.5 opacity-0 group-hover:pointer-events-auto group-hover:opacity-100 group-data-[state=open]:pointer-events-auto group-data-[state=open]:opacity-100">
            {/* Pin hidden for now; the flag and its write path stay live. */}
            {/* <RowAction
              label={item.pinned ? "Unpin" : "Pin"}
              active={item.pinned}
              onClick={() => onSetFlags(item.sessionId, { pinned: !item.pinned })}
            >
              <Pin />
            </RowAction> */}

            <RowAction
              label={item.archived ? "Unsettle" : "Settle"}
              className="-mr-1.5 text-sidebar-foreground/80"
              active={item.archived}
              onClick={() =>
                onSetFlags(item.sessionId, { archived: !item.archived })
              }
            >
              {/* Settle reads as "check this off"; unsettle isn't a second
                  checkmark, it's undoing the first one. */}
              {item.archived ? <Undo2 /> : <Check />}
            </RowAction>
          </div>
        </div>
      </div>
    </RowMenu>
  );
}
