import { useState } from "react";
import { Check } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Kbd, KbdGroup } from "@/components/ui/kbd";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { IS_MAC } from "@/lib/platform";
import { modelKey, resolveEffort } from "@/lib/models";
import type { Effort, Model, ModelId, AgentModel } from "@/types/events";

export const EFFORT_LABELS: Record<Effort, string> = {
  off: "Off",
  none: "None",
  minimal: "Minimal",
  low: "Low",
  medium: "Medium",
  high: "High",
  xhigh: "Extra High",
  max: "Max",
};

export const EFFORTS: Effort[] = ["off", "none", "minimal", "low", "medium", "high", "xhigh", "max"];

// Preserve the original shortcut behavior until the user explicitly configures
// the cycle. Off and Low remain available both in the picker and as opt-in
// cycle levels.
export const DEFAULT_CYCLE_EFFORTS: Effort[] = ["medium", "high", "xhigh", "max"];

/// An unset cycle preserves the original defaults. An explicit empty selection
/// means every reasoning level is included, so the shortcut cannot be disabled.
export function resolveCycleEfforts(selectedEfforts: readonly Effort[] | null): readonly Effort[] {
  if (selectedEfforts === null) return DEFAULT_CYCLE_EFFORTS;
  return selectedEfforts.length === 0 ? EFFORTS : selectedEfforts;
}

export { modelKey } from "@/lib/models";

export const modelLabel = (model: Model) => model.label || model.agentModel?.id || model.id;

/// `null` or an empty selection means every discovered model is shown. Stable
/// keys let a nonempty selection survive catalog refreshes.
export function modelsForKeys(models: Model[], selectedKeys: readonly string[] | null): Model[] {
  return selectedKeys?.length
    ? models.filter((model) => selectedKeys.includes(modelKey(model)))
    : models;
}

/// Model and effort completions use the same ranking shape as commands: an
/// exact name prefix wins, while provider/model identifiers remain searchable.
export function filterModels(models: Model[], query: string): Model[] {
  const q = query.toLowerCase();
  return models
    .map((model) => {
      const values = [modelLabel(model), model.id, model.agentModel?.id, modelKey(model)]
        .filter((value): value is string => Boolean(value))
        .map((value) => value.toLowerCase());
      const score = values.some((value) => value.startsWith(q))
        ? 0
        : values.some((value) => value.includes(q))
          ? 1
          : null;
      return { model, score };
    })
    .filter((match) => match.score !== null)
    .sort((a, b) => a.score! - b.score!)
    .map((match) => match.model);
}

export function filterEfforts(query: string, efforts: readonly Effort[]): Effort[] {
  const q = query.toLowerCase();
  return efforts.filter(
    (effort) => effort.startsWith(q) || EFFORT_LABELS[effort].toLowerCase().startsWith(q),
  );
}

/// Next effort level for `model`, wrapping — what Cmd/Ctrl+Shift+E lands on. `null`
/// where the model offers nothing to cycle, so the chord no-ops rather than
/// inventing an effort the CLI would ignore.
///
/// Levels outside the configured cycle stay pickable from the menu and enter
/// the cycle at its first supported level when the shortcut is pressed.
export function nextEffort(
  model: Model | undefined,
  current: Effort | null,
  included: readonly Effort[] = DEFAULT_CYCLE_EFFORTS,
): Effort | null {
  const cycle = model?.efforts.filter((effort) => included.includes(effort)) ?? [];
  if (cycle.length === 0) return null;
  const from = current ?? model?.defaultEffort ?? null;
  const i = from ? cycle.indexOf(from) : -1;
  return cycle[(i + 1) % cycle.length];
}

export default function ModelSelector({
  models,
  modelId,
  id,
  agentModel,
  effort,
  onChange,
}: {
  models: Model[];
  modelId: ModelId;
  id?: string;
  agentModel: AgentModel | null;
  effort: Effort | null;
  onChange: (modelId: ModelId, effort: Effort | null, agentModel: AgentModel | null) => void;
}) {
  // Controlled so a click on a submenu trigger can close the whole menu; Radix
  // otherwise keeps the parent open for the submenu it just opened on hover.
  const [open, setOpen] = useState(false);

  const selected = models.find(
    (m) =>
      m.id === modelId &&
      (m.id !== "dray" ||
        (m.agentModel?.provider === agentModel?.provider && m.agentModel?.id === agentModel?.id)),
  ) ?? null;
  const selectedEffort = resolveEffort(selected, effort);
  /// What a row would resolve to if clicked: the live effort for the model
  /// already selected, each other model's own default. Mirrors the resolution
  /// in `useSessions`, so the menu can't advertise an effort the send wouldn't use.
  const rowEffort = (model: Model): Effort | null =>
    resolveEffort(model, selected && modelKey(model) === modelKey(selected) ? effort : null);

  return (
    <DropdownMenu open={open} onOpenChange={setOpen}>
      <Tooltip>
        <TooltipTrigger asChild>
          <DropdownMenuTrigger asChild>
            {/* `text-ui` over the button's own `text-sm`: the toolbar has to track
                the runtime font-size setting like the rest of the chrome. */}
            <Button
              id={id}
              type="button"
              variant="ghost"
              size="sm"
              className="gap-1 px-1.5 text-ui text-muted-foreground"
            >
              {/* Effort is a qualifier on the model, not part of its name, so it's
                  held back a step rather than reading as one long label. */}
              <span>
                {selected ? modelLabel(selected) : modelId === "dray" && agentModel ? agentModel.id : modelId}
              </span>
              {selectedEffort && (
                <span className="text-muted-foreground/60">{EFFORT_LABELS[selectedEffort]}</span>
              )}
            </Button>
          </DropdownMenuTrigger>
        </TooltipTrigger>
        {/* One line, so it stays a tooltip rather than a menu of shortcuts —
            hence `max-w-none`, which the default `max-w-xs` would wrap. */}
        <TooltipContent side="top" className="max-w-none whitespace-nowrap">
          Switch model
          <KbdGroup>
            <Kbd>{IS_MAC ? "⌘" : "Ctrl"}</Kbd>
            <Kbd>M</Kbd>
          </KbdGroup>
          <span className="text-muted-foreground">Effort</span>
          <KbdGroup>
            <Kbd>{IS_MAC ? "⌘" : "Ctrl"}</Kbd>
            <Kbd>Shift</Kbd>
            <Kbd>E</Kbd>
          </KbdGroup>
        </TooltipContent>
      </Tooltip>

      <DropdownMenuContent align="start" className="min-w-48">
        {models.map((model) =>
          model.efforts.length ? (
            // One row: hover opens the effort submenu (Radix's own behaviour),
            // click picks the model and leaves its effort alone. Splitting the
            // two into separate items would give the row two hover states.
            <DropdownMenuSub key={modelKey(model)}>
              <DropdownMenuSubTrigger
                className="cursor-pointer gap-1 text-ui"
                onClick={() => {
                  onChange(model.id, null, model.agentModel);
                  setOpen(false);
                }}
              >
                {modelLabel(model)}
                {rowEffort(model) && (
                  <span className="text-muted-foreground/60">
                    {EFFORT_LABELS[rowEffort(model)!]}
                  </span>
                )}
              </DropdownMenuSubTrigger>
              <DropdownMenuSubContent>
                <KbdGroup className="px-2 py-1.5" aria-label="Cycle reasoning: Cmd/Ctrl+Shift+E">
                  <Kbd>{IS_MAC ? "⌘" : "Ctrl"}</Kbd>
                  <Kbd>Shift</Kbd>
                  <Kbd>E</Kbd>
                </KbdGroup>
                {model.efforts.map((level) => (
                  <DropdownMenuItem
                    key={level}
                    className="text-ui"
                    onSelect={() => {
                      onChange(model.id, level, model.agentModel);
                      setOpen(false);
                    }}
                  >
                    {EFFORT_LABELS[level]}
                    {selected && modelKey(model) === modelKey(selected) && selectedEffort === level && (
                      <Check className="ml-auto size-4" aria-label="Selected" />
                    )}
                  </DropdownMenuItem>
                ))}
              </DropdownMenuSubContent>
            </DropdownMenuSub>
          ) : (
            // No submenu and no chevron for a model with no effort levels.
            <DropdownMenuItem
              key={modelKey(model)}
              className="text-ui"
              onSelect={() => onChange(model.id, null, model.agentModel)}
            >
              {modelLabel(model)}
            </DropdownMenuItem>
          ),
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
