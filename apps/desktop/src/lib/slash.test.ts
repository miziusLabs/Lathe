import { describe, expect, it } from "vitest";

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
} from "./slash";
import type { SlashCommand } from "@/types/events";

function command(
  name: string,
  description = "",
  aliases: string[] = [],
  isSkill = false,
): SlashCommand {
  return { name, description, argumentHint: "", aliases, isSkill };
}

describe("slashQuery", () => {
  it("opens on a leading slash and tracks what follows it", () => {
    expect(slashQuery("/", 1)).toBe("");
    expect(slashQuery("/rev", 4)).toBe("rev");
  });

  it("opens skills with a leading dollar sign", () => {
    expect(slashQuery("$commit", 7)).toBe("commit");
  });

  it("opens skills in the middle of prose", () => {
    const text = "finish with $commit-and-push when ready";
    expect(slashQuery(text, 24)).toBe("commit-and-push");
    expect(slashPrefix(text, 24)).toBe("$");
    expect(slashQuery(text, text.length)).toBeNull();
  });

  /// A slash inside prose is a path or a date, not a command. Firing there
  /// would put a picker over most sentences that mention a file.
  it("stays shut for a slash that isn't leading", () => {
    expect(slashQuery("look at src/lib", 15)).toBeNull();
    expect(slashQuery("on 12/08", 8)).toBeNull();
  });

  /// The command is settled once a space is typed, so the picker gets out of
  /// the way of the arguments.
  it("closes once the caret moves into the arguments", () => {
    expect(slashQuery("/review the diff", 16)).toBeNull();
    expect(slashQuery("/review ", 8)).toBeNull();
  });

  /// Backspacing into the name to fix it has to reopen the picker, which is
  /// what makes this caret-based rather than "the text has no space in it".
  it("reopens when the caret goes back into the name", () => {
    expect(slashQuery("/review the diff", 7)).toBe("review");
    expect(slashQuery("/review the diff", 4)).toBe("review");
  });

  it("ignores a caret sitting on the slash itself", () => {
    expect(slashQuery("/rev", 0)).toBeNull();
  });
});

describe("drayCommands", () => {
  it("offers only model and effort in the New Task menu", () => {
    expect(drayCommands(true).map((command) => command.name)).toEqual(["model", "effort"]);
    expect(drayCommands(true)[0].aliases).toEqual(["models"]);
    expect(drayCommands(false).map((command) => command.name)).toEqual([
      "model",
      "effort",
      "new",
      "settle",
    ]);
  });
});

describe("filterCommandsByPrefix", () => {
  const commands = [command("review"), command("commit", "", [], true)];

  it("keeps skills out of the slash menu", () => {
    expect(filterCommandsByPrefix(commands, "/").map((c) => c.name)).toEqual(["review"]);
  });

  it("keeps commands out of the skills menu", () => {
    expect(filterCommandsByPrefix(commands, "$").map((c) => c.name)).toEqual(["commit"]);
  });
});

describe("filterCommands", () => {
  const commands = [
    command("compact", "Free up context"),
    command("clear", "Start a new session", ["reset", "new"]),
    command("railway:deploy", "Deploy to Railway"),
    command("usage", "Show plan limits"),
    command("cost", "Token usage for this session"),
    command("model", "Set the AI model for Lathe"),
  ];

  /// Stable sort plus an all-equal score means the CLI's own ordering survives,
  /// which is what groups user commands ahead of built-ins.
  it("keeps the given order when nothing is typed", () => {
    expect(filterCommands(commands, "").map((c) => c.name)).toEqual([
      "compact",
      "clear",
      "railway:deploy",
      "usage",
      "cost",
      "model",
    ]);
  });

  /// `/cost` describes itself as token usage, so a bare "usage" matches both —
  /// the one *named* it has to win, or typing a command's exact name can still
  /// leave something else selected.
  it("puts a name prefix ahead of a description mention", () => {
    expect(filterCommands(commands, "usage").map((c) => c.name)).toEqual(["usage", "cost"]);
  });

  it("finds a command by its alias", () => {
    expect(filterCommands(commands, "reset").map((c) => c.name)).toEqual(["clear"]);
  });

  /// A namespaced command is most often remembered by its bare half — nobody
  /// types the plugin name first.
  it("finds a namespaced command by its bare half", () => {
    expect(filterCommands(commands, "deploy").map((c) => c.name)).toEqual(["railway:deploy"]);
  });

  it("drops what matches nothing", () => {
    expect(filterCommands(commands, "zzz")).toEqual([]);
  });

  it("is case-insensitive", () => {
    expect(filterCommands(commands, "COMPACT").map((c) => c.name)).toEqual(["compact"]);
  });
});

describe("slashArgumentQuery", () => {
  it("opens model completion after the command name", () => {
    expect(slashArgumentQuery("/model ", 7, ["model", "effort"])).toEqual({
      commandName: "model",
      query: "",
    });
    expect(slashArgumentQuery("/models claude", 13, ["model", "models", "effort"])).toEqual({
      commandName: "models",
      query: "claude",
    });
    expect(slashArgumentQuery("/model claude", 12, ["model", "effort"])).toEqual({
      commandName: "model",
      query: "claude",
    });
  });

  it("does not open for another command or its later arguments", () => {
    expect(slashArgumentQuery("/clear ", 7, ["model", "effort"])).toBeNull();
    expect(slashArgumentQuery("/effort high now", 16, ["model", "effort"])).toBeNull();
  });
});

describe("applyCommand", () => {
  it("completes a half-typed name and leaves the caret past a space", () => {
    expect(applyCommand("/comp", "compact")).toEqual({ text: "/compact ", caret: 9 });
  });

  /// Picking a different command from inside a line that already has arguments
  /// must not eat them.
  it("keeps arguments already typed", () => {
    expect(applyCommand("/rev the diff", "review")).toEqual({
      text: "/review the diff",
      caret: 8,
    });
  });

  it("completes a bare slash", () => {
    expect(applyCommand("/", "usage")).toEqual({ text: "/usage ", caret: 7 });
    expect(applyCommand("$", "commit-and-push", true)).toEqual({
      text: "$commit-and-push ",
      caret: 17,
    });
  });

  it("completes a skill in prose without replacing the message", () => {
    expect(applyCommand("finish with $comm when ready", "commit-and-push", true, 17)).toEqual({
      text: "finish with $commit-and-push when ready",
      caret: 29,
    });
  });
});

describe("applyCommandArgument", () => {
  it("completes a model argument and keeps the command prefix", () => {
    expect(applyCommandArgument("/model cla", "model", "claude")).toEqual({
      text: "/model claude ",
      caret: 14,
    });
  });
});

describe("parseSlashCommand", () => {
  it("splits a command from its arguments", () => {
    expect(parseSlashCommand("/compact keep the diff notes")).toEqual({
      name: "compact",
      args: "keep the diff notes",
    });
  });

  it("reads a bare command or skill", () => {
    expect(parseSlashCommand("/usage")).toEqual({ name: "usage", args: "" });
    expect(parseSlashCommand("/usage  ")).toEqual({ name: "usage", args: "" });
    expect(parseSlashCommand("$commit-and-push")).toEqual({
      name: "commit-and-push",
      args: "",
    });
  });

  it("keeps namespaced names whole", () => {
    expect(parseSlashCommand("/railway:deploy now")).toEqual({
      name: "railway:deploy",
      args: "now",
    });
  });

  /// Multi-line arguments are ordinary — a command taking a pasted block still
  /// leads with its name.
  it("reads arguments spanning lines", () => {
    expect(parseSlashCommand("/spec build\na login page")).toEqual({
      name: "spec",
      args: "build\na login page",
    });
  });

  it("leaves prose alone", () => {
    expect(parseSlashCommand("check src/lib/slash.ts")).toBeNull();
    expect(parseSlashCommand("/")).toBeNull();
    expect(parseSlashCommand("// a comment")).toBeNull();
    expect(parseSlashCommand("/ spaced")).toBeNull();
  });
});
