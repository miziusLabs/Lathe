import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";

import type { Harness, SlashCommand } from "@/types/events";

/// Kept across mounts, keyed by directory. The backend caches these too, so this
/// only saves the round trip — but the composer remounts on every session
/// switch, and a picker that refetched each time would open empty on the first
/// keystroke after every switch.
const cache = new Map<string, SlashCommand[]>();

/// Lathe skills available in `cwd`, empty until they land. Lathe commands are not
/// returned to the composer; Lathe owns the slash-command surface.
///
/// A failed probe resolves to no commands rather than surfacing an error: the
/// picker is an accelerator for text the user can always type by hand, so it
/// staying shut is a smaller failure than an error banner over the composer.
export function useSlashCommands(cwd: string | null, harness: Harness): SlashCommand[] {
  const cacheKey = cwd ? `${harness}\0${cwd}` : null;
  const [commands, setCommands] = useState<SlashCommand[]>(
    () => (cacheKey ? cache.get(cacheKey) : undefined) ?? [],
  );

  useEffect(() => {
    if (!cwd) {
      setCommands([]);
      return;
    }

    const hit = cacheKey ? cache.get(cacheKey) : undefined;
    if (hit) {
      setCommands(hit);
      return;
    }

    // The probe spawns a CLI child and takes ~1.5s, so a fast project switch can
    // easily outrun it — without the guard the previous project's commands land
    // on top of this one's.
    let cancelled = false;
    // Cleared rather than left stale: offering another project's project-scoped
    // commands is worse than offering none.
    setCommands([]);

    invoke<SlashCommand[]>("list_slash_commands", { cwd, harness })
      .then((list) => {
        const skills = list.filter((command) => command.isSkill);
        if (cacheKey) cache.set(cacheKey, skills);
        if (!cancelled) setCommands(skills);
      })
      .catch((e) => {
        console.error("[slash commands]", e);
        if (!cancelled) setCommands([]);
      });

    return () => {
      cancelled = true;
    };
  }, [cwd, harness]);

  return commands;
}
