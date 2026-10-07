import { Plus } from "lucide-react";

import BranchSelector from "@/components/composer/BranchSelector";
import BranchSwitchDialog from "@/components/composer/BranchSwitchDialog";
import ContextMeter from "@/components/composer/ContextMeter";
import ModelSelector from "@/components/composer/ModelSelector";
import ProjectSelector from "@/components/composer/ProjectSelector";
import WorktreeToggle from "@/components/composer/WorktreeToggle";
import { Button } from "@/components/ui/button";
import { Kbd, KbdGroup } from "@/components/ui/kbd";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type {
  BranchList,
  Effort,
  Model,
  ModelId,
  AgentModel,
  Project,
} from "@/types/events";

export type ComposerToolbarProps = {
  models: Model[];
  modelId: ModelId;
  agentModel: AgentModel | null;
  effort: Effort | null;
  onModelChange: (modelId: ModelId, effort: Effort | null, agentModel: AgentModel | null) => void;

  projects: Project[];
  projectPath: string | null;
  onSelectProject: (path: string | null) => void;
  onAttachProject: () => void;
  onRenameProject: (path: string, name: string) => Promise<boolean>;
  onDeleteProject: (path: string) => Promise<boolean>;
  onReorderProjects: (paths: string[]) => Promise<boolean>;

  branches: BranchList | null;
  branch: string | null;
  onSelectBranch: (branch: string) => void;

  /// Set while a switch waits on the uncommitted-changes prompt.
  pendingBranch: string | null;
  onConfirmBranchSwitch: (stash: boolean) => void;
  onCancelBranchSwitch: () => void;

  useWorktree: boolean;
  worktreeAvailable: boolean;
  worktreeUnavailableReason: string;
  onToggleWorktree: () => void;

  /// Opens the file picker. The attachments themselves are held in a
  /// module-level store keyed by session, not passed through here — this row is
  /// handed to `ChatInput` as an opaque node, so the two cannot share props.
  onAttach: () => void;

  /// How full the model's context is, or `null` before any turn has reported
  /// it. Sits at the far end of the row rather than among the pickers: it
  /// reports rather than sets, and nothing here changes it.
  contextUsage: { used: number; max: number } | null;

  /// Where the session runs is fixed at creation, so the last three controls
  /// only exist before one starts.
  isNewSession: boolean;
};

/// The composer's control row. Project, branch, and worktree decide where a session
/// starts and disappear once it has — a control that can never be used is noise,
/// and the session header already shows the project and branch. Its own spacing
/// from the card is the caller's, since only the caller knows which side of it
/// the row sits on.
export default function ComposerToolbar({
  models,
  modelId,
  agentModel,
  effort,
  onModelChange,
  projects,
  projectPath,
  onSelectProject,
  onAttachProject,
  onRenameProject,
  onDeleteProject,
  onReorderProjects,
  branches,
  branch,
  onSelectBranch,
  pendingBranch,
  onConfirmBranchSwitch,
  onCancelBranchSwitch,
  useWorktree,
  worktreeAvailable,
  worktreeUnavailableReason,
  onToggleWorktree,
  onAttach,
  contextUsage,
  isNewSession,
}: ComposerToolbarProps) {
  return (
    <div className="flex min-w-0 items-center gap-0.5 px-1">
      {/* No radius override: `icon-sm` already carries the app's rounded-square,
          and a circle here would be the one round control in a row of them. */}
      <Tooltip>
        <TooltipTrigger asChild>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            onClick={onAttach}
            aria-label="Attach files"
            className="text-muted-foreground"
          >
            <Plus />
          </Button>
        </TooltipTrigger>
        <TooltipContent side="top">
          Attach files
          <KbdGroup>
            <Kbd>⌘</Kbd>
            <Kbd>⌥</Kbd>
            <Kbd>O</Kbd>
          </KbdGroup>
        </TooltipContent>
      </Tooltip>

      <ModelSelector
        models={models}
        modelId={modelId}
        agentModel={agentModel}
        effort={effort}
        onChange={onModelChange}
      />

      {isNewSession && (
        <>
          <ProjectSelector
            projects={projects}
            value={projectPath}
            onSelect={onSelectProject}
            onAttach={onAttachProject}
            onRename={onRenameProject}
            onDelete={onDeleteProject}
            onReorder={onReorderProjects}
          />

          <WorktreeToggle
            on={useWorktree}
            onToggle={onToggleWorktree}
            disabled={!worktreeAvailable}
            disabledReason={!worktreeAvailable ? worktreeUnavailableReason : undefined}
          />

          {projectPath && (
            <div className="relative flex min-w-0 items-center">
              <BranchSelector
                key="branch-selector"
                branches={branches}
                value={branch}
                onSelect={onSelectBranch}
                disabled={pendingBranch !== null}
              />
              <BranchSwitchDialog
                key="branch-switch-dialog"
                target={pendingBranch}
                dirty={branches?.dirty ?? 0}
                onConfirm={onConfirmBranchSwitch}
                onCancel={onCancelBranchSwitch}
              />
            </div>
          )}
        </>
      )}
      {/* `ml-auto` rather than a spacer, so a long branch name still gets the
          whole middle of the row and this stays pinned to the right edge. */}
      {contextUsage && (
        <div className="ml-auto">
          <ContextMeter used={contextUsage.used} max={contextUsage.max} />
        </div>
      )}
    </div>
  );
}
