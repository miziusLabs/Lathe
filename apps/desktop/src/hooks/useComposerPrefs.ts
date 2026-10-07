import { useCallback } from "react";

import { useLocalStorage } from "@/hooks/useLocalStorage";
import { migrateEffortPreferences } from "@/lib/models";
import type { Effort, Harness, ModelId, AgentModel } from "@/types/events";

/// Seeds for a first run with nothing stored. Once the user picks anything, their
/// pick is the default — these are never read again.
const SEED: ComposerPrefs = {
  harness: "dray",
  modelId: "dray",
  agentModel: null,
  effortByModel: {},
  useWorktree: false,
};

/// Key by provider/model so Luna does not inherit a choice made for Astra.
/// An absent entry uses the model's catalog default.
export type EffortByModel = Partial<Record<string, Effort>>;

/// What a new session starts with. Deliberately not the whole composer: `branch`
/// seeds from whatever the repo is checked out to, since restoring a name without
/// running the checkout would have the composer claim a branch the tree isn't on;
/// `projectPath` is already persisted backend-side by `set_last_selected_project`.
export type ComposerPrefs = {
  harness: Harness;
  modelId: ModelId;
  agentModel: AgentModel | null;
  effortByModel: EffortByModel;
  useWorktree: boolean;
};

/// The sticky half of the composer. Every control the user can change writes here,
/// so "I always want acceptEdits on Sonnet" survives both a session switch and a
/// relaunch, and `handleNewSession` seeds from it instead of from a constant.
///
/// Restoring a *session's* settings must never write here — clicking through old
/// sessions would otherwise rewrite the defaults behind the user's back.
export function useComposerPrefs() {
  const [prefs, setPrefs] = useLocalStorage<ComposerPrefs>("ade.composerPrefs", SEED);

  // Merged over the seed on read, so a record written by an older build that
  // lacks a key gets the seed for it rather than `undefined` reaching a picker.
  // Normalize preferences written by older builds so removed harnesses and
  // their model aliases cannot reach a new session.
  const merged: ComposerPrefs = {
    ...SEED,
    ...prefs,
    harness: "dray",
    modelId: "dray",
    effortByModel: migrateEffortPreferences(prefs.effortByModel ?? {}, "dray", prefs.agentModel ?? null),
  };

  const patch = useCallback(
    (next: Partial<ComposerPrefs>) => setPrefs((prev) => ({
      ...SEED,
      ...prev,
      effortByModel: migrateEffortPreferences(prev.effortByModel ?? {}, "dray", prev.agentModel ?? null),
      ...next,
    })),
    [setPrefs],
  );

  return [merged, patch] as const;
}
