import { useId, useState, type ReactNode } from "react";
import { open } from "@tauri-apps/plugin-dialog";

import ChatGptAccount from "@/components/ChatGptAccount";

import packageJson from "../../package.json";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import ModelSelector, {
  EFFORT_LABELS,
  EFFORTS,
  modelKey,
  modelLabel,
  modelsForKeys,
  resolveCycleEfforts,
} from "@/components/composer/ModelSelector";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Switch } from "@/components/ui/switch";
import { Kbd, KbdGroup } from "@/components/ui/kbd";
import { IS_MAC } from "@/lib/platform";
import type { Effort, Model, ModelId, AgentModel } from "@/types/events";
import type { UsageDisplayMode } from "@/types/usage";
import type { UpdateCheckResult } from "@/components/UpdateNotice";

/// The app's preferences, such as they are.
///
/// Mounted in `App` rather than beside the gear that opens it so the dialog's
/// lifecycle stays independent from the sidebar controls.
const SETTINGS_CATEGORIES = ["general", "account", "models", "updates"] as const;

type SettingsCategory = (typeof SETTINGS_CATEGORIES)[number];

const SETTINGS_CATEGORY_LABELS: Record<SettingsCategory, string> = {
  general: "General",
  account: "Providers",
  models: "Models",
  updates: "Updates",
};

const SETTINGS_CATEGORY_INDEX: Record<SettingsCategory, number> = {
  general: 0,
  account: 1,
  models: 2,
  updates: 3,
};

export default function SettingsDialog({
  open,
  onOpenChange,
  showArchived,
  onShowArchivedChange,
  usageDisplayMode,
  onUsageDisplayModeChange,
  noProjectPath,
  onNoProjectPathChange,
  models,
  visibleModelKeys,
  onVisibleModelKeysChange,
  cycleModelKeys,
  onCycleModelKeysChange,
  cycleEfforts,
  onCycleEffortsChange,
  autoDownloadUpdates,
  onAutoDownloadUpdatesChange,
  titleModels,
  titleModelId,
  titleAgentModel,
  titleEffort,
  onTitleModelChange,
  checkingForUpdates,
  onCheckForUpdates,
}: {
  open: boolean;
  onOpenChange: (next: boolean) => void;
  showArchived: boolean;
  onShowArchivedChange: (next: boolean) => void;
  usageDisplayMode: UsageDisplayMode;
  onUsageDisplayModeChange: (next: UsageDisplayMode) => void;
  noProjectPath: string;
  onNoProjectPathChange: (next: string) => void;
  models: Model[];
  visibleModelKeys: string[] | null;
  onVisibleModelKeysChange: (next: string[]) => void;
  cycleModelKeys: string[] | null;
  onCycleModelKeysChange: (next: string[]) => void;
  cycleEfforts: Effort[] | null;
  onCycleEffortsChange: (next: Effort[]) => void;
  autoDownloadUpdates: boolean;
  onAutoDownloadUpdatesChange: (next: boolean) => void;
  titleModels: Model[];
  titleModelId: ModelId;
  titleAgentModel: AgentModel | null;
  titleEffort: Effort;
  onTitleModelChange: (
    modelId: ModelId,
    effort: Effort | null,
    agentModel: AgentModel | null,
  ) => void;
  checkingForUpdates: boolean;
  onCheckForUpdates: () => Promise<UpdateCheckResult>;
}) {
  const [category, setCategory] = useState<SettingsCategory>("general");

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      {/* Each row carries its own sentence, so there is no one description the
          dialog is described *by* — left unset, Radix warns about the missing
          `aria-describedby` and pointing it at a row would read that row's copy
          out as the dialog's purpose. */}
      <DialogContent className="grid-cols-1 max-w-120" aria-describedby={undefined}>
        <DialogHeader>
          <DialogTitle>Settings</DialogTitle>
        </DialogHeader>

        <div
          className="relative grid grid-cols-4 rounded-lg bg-muted p-0.5"
          role="tablist"
          aria-label="Settings categories"
        >
          <span
            aria-hidden
            className="pointer-events-none absolute inset-y-0.5 left-0.5 rounded-md bg-popover shadow-sm transition-transform duration-200 ease-out"
            style={{
              width: `calc((100% - 0.25rem) / ${SETTINGS_CATEGORIES.length})`,
              transform: `translateX(${SETTINGS_CATEGORY_INDEX[category] * 100}%)`,
            }}
          />
          {SETTINGS_CATEGORIES.map((value) => (
            <button
              key={value}
              type="button"
              role="tab"
              aria-selected={category === value}
              aria-controls="settings-panel"
              tabIndex={category === value ? 0 : -1}
              onClick={() => setCategory(value)}
              className={[
                "relative z-10 rounded-md px-2 py-1.5 text-ui transition-colors focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:outline-none",
                category === value
                  ? "text-foreground"
                  : "text-muted-foreground hover:text-foreground",
              ].join(" ")}
            >
              {SETTINGS_CATEGORY_LABELS[value]}
            </button>
          ))}
        </div>

        {/* Keep every category in the same grid cell so the dialog reserves the
            height of the largest category while only the selected one is visible. */}
        <div
          id="settings-panel"
          role="tabpanel"
          aria-label={SETTINGS_CATEGORY_LABELS[category]}
          className="grid min-w-0 grid-cols-1 items-start"
        >
          <div
            className={[
              "col-start-1 row-start-1 flex flex-col gap-6",
              category !== "general" && "invisible pointer-events-none",
            ].filter(Boolean).join(" ")}
            aria-hidden={category !== "general"}
            inert={category !== "general"}
          >
            <SettledSessionsRow
              checked={showArchived}
              onChange={onShowArchivedChange}
            />
            <UsageDisplayRow
              mode={usageDisplayMode}
              onChange={onUsageDisplayModeChange}
            />
            <NoProjectDirectoryRow
              path={noProjectPath}
              onChange={onNoProjectPathChange}
            />
          </div>
          <div
            className={[
              "col-start-1 row-start-1 flex flex-col gap-6",
              category !== "models" && "invisible pointer-events-none",
            ].filter(Boolean).join(" ")}
            aria-hidden={category !== "models"}
            inert={category !== "models"}
          >
            <ModelSelectionRow
              models={models}
              selectedKeys={visibleModelKeys}
              onChange={onVisibleModelKeysChange}
              label="Shown models"
              description="Choose which models appear in the model selector and /model or /models commands."
            />
            <ModelSelectionRow
              models={modelsForKeys(models, visibleModelKeys)}
              selectedKeys={cycleModelKeys}
              onChange={onCycleModelKeysChange}
              label="Cycle models"
              description={
                <>
                  Choose which shown models{" "}
                  <KbdGroup>
                    <Kbd>{IS_MAC ? "⌘" : "Ctrl"}</Kbd>
                    <Kbd>M</Kbd>
                  </KbdGroup>{" "}
                  cycles through.
                </>
              }
            />
            <CycleEffortsRow
              selectedEfforts={cycleEfforts}
              onChange={onCycleEffortsChange}
            />
            <TitleGenerationRow
              models={titleModels}
              modelId={titleModelId}
              agentModel={titleAgentModel}
              effort={titleEffort}
              onChange={onTitleModelChange}
            />
          </div>
          <div
            className={[
              "col-start-1 row-start-1 flex min-w-0 flex-col gap-6",
              category !== "account" && "invisible pointer-events-none",
            ].filter(Boolean).join(" ")}
            aria-hidden={category !== "account"}
            inert={category !== "account"}
          >
            <ChatGptAccount />
          </div>
          <div
            className={[
              "col-start-1 row-start-1 flex flex-col gap-6",
              category !== "updates" && "invisible pointer-events-none",
            ].filter(Boolean).join(" ")}
            aria-hidden={category !== "updates"}
            inert={category !== "updates"}
          >
            <UpdateCheckRow
              checking={checkingForUpdates}
              onCheck={onCheckForUpdates}
            />
            <AutoDownloadUpdatesRow
              checked={autoDownloadUpdates}
              onChange={onAutoDownloadUpdatesChange}
            />
          </div>
        </div>

        <footer className="pt-3 text-center text-xs text-muted-foreground">
          © miziusLabs | v{packageJson.version}
        </footer>
      </DialogContent>
    </Dialog>
  );
}

function UsageDisplayRow({
  mode,
  onChange,
}: {
  mode: UsageDisplayMode;
  onChange: (next: UsageDisplayMode) => void;
}) {
  const id = useId();

  return (
    <SettingRow
      id={id}
      label="Plan usage indicator"
      description="Choose whether the plan usage bars show remaining or consumed usage."
    >
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button id={id} type="button" variant="outline" size="sm" className="text-ui">
            {mode === "left" ? "Remaining" : "Consumed"}
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="min-w-24">
          <DropdownMenuRadioGroup
            value={mode}
            onValueChange={(value) => {
              if (value === "left" || value === "used") onChange(value);
            }}
          >
            <DropdownMenuRadioItem value="left" className="text-ui">
              Remaining
            </DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="used" className="text-ui">
              Consumed
            </DropdownMenuRadioItem>
          </DropdownMenuRadioGroup>
        </DropdownMenuContent>
      </DropdownMenu>
    </SettingRow>
  );
}

function UpdateCheckRow({
  checking,
  onCheck,
}: {
  checking: boolean;
  onCheck: () => Promise<UpdateCheckResult>;
}) {
  const id = useId();
  const [message, setMessage] = useState<string | null>(null);

  const checkForUpdates = async () => {
    setMessage(null);
    try {
      const result = await onCheck();
      setMessage(
        result === "available"
          ? "Update available"
          : result === "none"
            ? "You're up to date."
            : "Update checks are unavailable in development builds.",
      );
    } catch {
      setMessage("Could not check for updates. Try again later.");
    }
  };

  return (
    <SettingRow
      id={id}
      label="App updates"
      description="Check for a newer version of Lathe. Automatic checks run every 15 minutes."
    >
      <Button
        id={id}
        type="button"
        variant="outline"
        size="sm"
        className="text-ui"
        disabled={checking}
        onClick={() => void checkForUpdates()}
      >
        {checking ? "Checking…" : message ?? "Check for updates"}
      </Button>
    </SettingRow>
  );
}

function AutoDownloadUpdatesRow({
  checked,
  onChange,
}: {
  checked: boolean;
  onChange: (next: boolean) => void;
}) {
  const id = useId();

  return (
    <SettingRow
      id={id}
      label="Automatically download updates"
      description="Download updates as soon as they become available. Installation still requires a restart."
    >
      <Switch id={id} checked={checked} onCheckedChange={onChange} />
    </SettingRow>
  );
}

function ModelSelectionRow({
  models,
  selectedKeys,
  onChange,
  label,
  description,
}: {
  models: Model[];
  selectedKeys: string[] | null;
  onChange: (next: string[]) => void;
  label: string;
  description: ReactNode;
}) {
  const id = useId();
  const resolvedKeys = selectedKeys?.length ? selectedKeys : models.map(modelKey);
  const selectedCount = models.filter((model) => resolvedKeys.includes(modelKey(model))).length;
  const summary =
    selectedCount === models.length
      ? "All models"
      : selectedCount === 1
        ? "1 model"
        : `${selectedCount} models`;

  const setChecked = (model: Model, checked: boolean) => {
    const key = modelKey(model);
    onChange(
      checked
        ? Array.from(new Set([...resolvedKeys, key]))
        : resolvedKeys.filter((selected) => selected !== key),
    );
  };

  return (
    <SettingRow id={id} label={label} description={description}>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button id={id} type="button" variant="outline" size="sm" className="text-ui">
            {summary}
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="min-w-48">
          {models.map((model) => {
            const key = modelKey(model);
            return (
              <DropdownMenuCheckboxItem
                key={key}
                checked={resolvedKeys.includes(key)}
                className="text-ui"
                onCheckedChange={(checked) => setChecked(model, checked === true)}
                onSelect={(event) => event.preventDefault()}
              >
                {modelLabel(model)}
              </DropdownMenuCheckboxItem>
            );
          })}
        </DropdownMenuContent>
      </DropdownMenu>
    </SettingRow>
  );
}

function CycleEffortsRow({
  selectedEfforts,
  onChange,
}: {
  selectedEfforts: Effort[] | null;
  onChange: (next: Effort[]) => void;
}) {
  const id = useId();
  const resolvedEfforts = resolveCycleEfforts(selectedEfforts);
  const selectedCount = EFFORTS.filter((effort) => resolvedEfforts.includes(effort)).length;
  const summary =
    selectedCount === EFFORTS.length
      ? "All levels"
      : selectedCount === 1
        ? "1 level"
        : `${selectedCount} levels`;

  const setChecked = (effort: Effort, checked: boolean) => {
    onChange(
      checked
        ? EFFORTS.filter((level) => level === effort || resolvedEfforts.includes(level))
        : resolvedEfforts.filter((level) => level !== effort),
    );
  };

  return (
    <SettingRow
      id={id}
      label="Cycle reasoning levels"
      description={
        <>
          Choose which reasoning levels{" "}
          <KbdGroup>
            <Kbd>Shift</Kbd>
            <Kbd>Tab</Kbd>
          </KbdGroup>{" "}
          cycles through.
        </>
      }
    >
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button id={id} type="button" variant="outline" size="sm" className="text-ui">
            {summary}
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="min-w-48">
          {EFFORTS.map((effort) => (
            <DropdownMenuCheckboxItem
              key={effort}
              checked={resolvedEfforts.includes(effort)}
              className="text-ui"
              onCheckedChange={(checked) => setChecked(effort, checked === true)}
              onSelect={(event) => event.preventDefault()}
            >
              {EFFORT_LABELS[effort]}
            </DropdownMenuCheckboxItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
    </SettingRow>
  );
}

function TitleGenerationRow({
  models,
  modelId,
  agentModel,
  effort,
  onChange,
}: {
  models: Model[];
  modelId: ModelId;
  agentModel: AgentModel | null;
  effort: Effort;
  onChange: (modelId: ModelId, effort: Effort | null, agentModel: AgentModel | null) => void;
}) {
  const id = useId();

  return (
    <SettingRow
      id={id}
      label="Title generation model"
      description="Choose the model and reasoning level used to name new sessions."
    >
      <ModelSelector
        id={id}
        models={models}
        modelId={modelId}
        agentModel={agentModel}
        effort={effort}
        onChange={onChange}
      />
    </SettingRow>
  );
}

function NoProjectDirectoryRow({
  path,
  onChange,
}: {
  path: string;
  onChange: (next: string) => void;
}) {
  const id = useId();

  const chooseDirectory = async () => {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked === "string") onChange(picked);
  };

  return (
    <SettingRow
      id={id}
      label="No Project directory"
      description="Choose where sessions without a project start. The default is ~/Coding/Sandbox."
      stacked
    >
      <div className="flex w-64 gap-2">
        <Input
          id={id}
          value={path}
          onChange={(event) => onChange(event.target.value)}
          aria-label="No Project directory"
          className="border-0"
        />
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="shrink-0 text-ui"
          onClick={() => void chooseDirectory()}
        >
          Browse
        </Button>
      </div>
    </SettingRow>
  );
}

function SettledSessionsRow({
  checked,
  onChange,
}: {
  checked: boolean;
  onChange: (next: boolean) => void;
}) {
  const id = useId();

  return (
    <SettingRow
      id={id}
      label="Show settled sessions"
      description="Show settled sessions instead of active sessions."
    >
      <Switch id={id} checked={checked} onCheckedChange={onChange} />
    </SettingRow>
  );
}

/// Label and reason on the left, with the control beside or below them.
function SettingRow({
  id,
  label,
  description,
  children,
  stacked = false,
}: {
  id: string;
  label: string;
  description: ReactNode;
  children: ReactNode;
  stacked?: boolean;
}) {
  return (
    <div className={stacked ? "flex flex-col gap-1" : "flex items-start justify-between gap-4"}>
      <div className="flex flex-col gap-1">
        <label htmlFor={id} className="text-ui font-medium">
          {label}
        </label>
        <p className="text-ui text-muted-foreground">{description}</p>
      </div>
      <div className={stacked ? "mt-2" : "mt-0.5"}>{children}</div>
    </div>
  );
}
