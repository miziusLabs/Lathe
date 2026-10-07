import { describe, expect, it } from "vitest";

import {
  backgroundAction,
  formatDuration,
  isRoutineError,
  runsInBackground,
  shortenPath,
  streamingLabel,
  toolGroupLabel,
  toolLabel,
  toolSummary,
} from "./tools";

// Every string here is a real `Bash` error taken out of `~/.dray/sessions`,
// including the "Exit code N" prefix a shell failure actually arrives with —
// the patterns have to match inside that, not against a bare message.
describe("Lathe extension tool labels", () => {
  it("shows the useful argument for installed extension tools", () => {
    expect(toolSummary("finder", "other", { query: "find the parser" })).toBe("find the parser");
    expect(toolSummary("libarian", "other", { task: "research the protocol" })).toBe(
      "research the protocol",
    );
    expect(
      toolSummary("background_command", "shell", { action: "start", command: "npm run dev" }),
    ).toBe("npm run dev");
    expect(
      toolSummary("background_command", "shell", {
        action: "check",
        id: "bg-1",
        command: "must not be shown",
      }),
    ).toBe("bg-1");
    expect(
      toolSummary("background_command", "shell", {
        action: "input",
        id: "bg-2",
        input: "yes\n",
        command: "must not be shown",
      }),
    ).toBe("bg-2");
    expect(
      toolSummary("background_command", "shell", {
        action: "stop",
        id: "bg-3",
        command: "must not be shown",
      }),
    ).toBe("bg-3");
  });

  it("only marks background starts with a command argument", () => {
    expect(
      runsInBackground("background_command", { action: "start", command: "npm run dev" }),
    ).toBe(true);
    expect(
      runsInBackground("background_command", { action: "check", id: "bg-1", command: "ignored" }),
    ).toBe(false);
    expect(runsInBackground("background_command", { action: "check", id: "bg-1" })).toBe(false);
    expect(runsInBackground("bash", { action: "start", command: "npm run dev" })).toBe(false);
  });

  it("labels background checks in both tenses", () => {
    expect(backgroundAction("background_command", { action: "check" })).toBe("check");
    expect(backgroundAction("bash", { action: "check" })).toBe(null);
    expect(toolLabel("background_command", true, "check")).toBe("Checking on");
    expect(toolLabel("background_command", false, "check")).toBe("Checked on");
    expect(streamingLabel("background_command", "check")).toBe("Checking on a command");
  });

  it("labels background input and stop actions in both tenses", () => {
    expect(toolLabel("background_command", true, "input")).toBe("Sending input to");
    expect(toolLabel("background_command", false, "input")).toBe("Sent input to");
    expect(streamingLabel("background_command", "input")).toBe("Sending input to a command");
    expect(toolLabel("background_command", true, "stop")).toBe("Stopping");
    expect(toolLabel("background_command", false, "stop")).toBe("Stopped");
    expect(streamingLabel("background_command", "stop")).toBe("Stopping a command");
  });

  it("uses readable labels for Lathe built-ins and extensions", () => {
    expect(toolLabel("read", true)).toBe("Reading");
    expect(toolLabel("edit", false)).toBe("Edited");
    expect(toolLabel("libarian", false)).toBe("Researched");
    expect(streamingLabel("finder")).toBe("Exploring codebase");
  });

  it("labels user questions instead of showing the raw tool name", () => {
    expect(toolLabel("ask_user", false)).toBe("Asked user");
    expect(toolLabel("ask_user", true)).toBe("Asking user");
    expect(streamingLabel("ask_user")).toBe("Asking a question");
  });

  it("shortens Windows paths without treating the drive as a scheme", () => {
    expect(shortenPath("C:\\Users\\jan\\repo\\src\\App.tsx")).toBe("src/App.tsx");
  });
});

describe("tool group labels", () => {
  it("summarizes mixed calls in first-seen order", () => {
    expect(
      toolGroupLabel([
        { name: "Read", toolType: "file_read" },
        { name: "Bash", toolType: "shell" },
      ]),
    ).toBe("Read files, ran a command");

    expect(
      toolGroupLabel([
        { name: "Bash", toolType: "shell" },
        { name: "Bash", toolType: "shell" },
      ]),
    ).toBe("Ran 2 commands");
  });

  it("formats worked time without decimal noise", () => {
    expect(formatDuration(26_000)).toBe("26s");
    expect(formatDuration(60_000)).toBe("1m 0s");
    expect(formatDuration(86_000)).toBe("1m 26s");
    expect(formatDuration(3_600_000)).toBe("1h 0m 0s");
    expect(formatDuration(3_661_000)).toBe("1h 1m 1s");
  });
});

describe("isRoutineError", () => {
  it("passes over a missing path, whatever spelled it", () => {
    expect(isRoutineError("Exit code 1\n(eval):cd:1: no such file or directory: apps/desktop")).toBe(
      true,
    );
    expect(
      isRoutineError("Exit code 1\nsed: src/components/ChatInput.tsx: No such file or directory"),
    ).toBe(true);
    expect(isRoutineError("Exit code 2\nugrep: warning: src/App.tsx: No such file or directory")).toBe(
      true,
    );
  });

  it("passes over a glob that matched nothing", () => {
    expect(isRoutineError("Exit code 1\n(eval):1: no matches found: *.tgz")).toBe(true);
  });

  it("passes over a binary that isn't there", () => {
    expect(isRoutineError("Exit code 127\n(eval):1: command not found: dray")).toBe(true);
  });

  it("passes over a command the harness blocked", () => {
    expect(isRoutineError("Blocked: sleep 60 followed by: gh pr view 7")).toBe(true);
  });

  it("still marks a real failure", () => {
    expect(isRoutineError("Exit code 1\ntar: Option --one-top-level=dlg is not supported")).toBe(
      false,
    );
    expect(
      isRoutineError("Exit code 1\nTraceback (most recent call last):\n  File \"<string>\", line 1"),
    ).toBe(false);
    expect(isRoutineError("Exit code 1\n(eval):1: === not found")).toBe(false);
  });

  it("treats a call with no text as worth marking", () => {
    expect(isRoutineError(undefined)).toBe(false);
    expect(isRoutineError("")).toBe(false);
  });
});
