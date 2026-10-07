import { describe, expect, it } from "vitest";

import { streamingCall } from "@/lib/streaming";

describe("streamingCall", () => {
  it("counts the lines of a Write's content as they arrive", () => {
    expect(streamingCall("Write", '{"file_path":"/a/b/c.py","content":"one\\ntwo\\nthree"')).toEqual(
      { target: "b/c.py", added: 3 },
    );
  });

  it("supports Lathe's lower-case write tool", () => {
    expect(streamingCall("write", '{"path":"src/main.ts","content":"one\\ntwo"}')).toEqual({
      target: "src/main.ts",
      added: 2,
    });
  });

  it("shows a Lathe research task while its arguments stream", () => {
    expect(streamingCall("libarian", '{"task":"research the protocol"}')).toEqual({
      target: "research the protocol",
      added: null,
    });
  });

  it("shows a background check's ID once the complete value arrives", () => {
    expect(
      streamingCall("background_command", '{"action":"check","id":"bg-1"'),
    ).toEqual({ target: "bg-1", action: "check", added: null });
  });

  it("withholds a background check's ID until its closing quote arrives", () => {
    expect(streamingCall("background_command", '{"action":"check","id":"bg-1')).toEqual({
      target: null,
      action: "check",
      added: null,
    });
  });

  it("shows background input and stop IDs once their values are complete", () => {
    expect(streamingCall("background_command", '{"action":"input","id":"bg-2"')).toEqual({
      target: "bg-2",
      action: "input",
      added: null,
    });
    expect(streamingCall("background_command", '{"action":"stop","id":"bg-3"')).toEqual({
      target: "bg-3",
      action: "stop",
      added: null,
    });
  });

  it("withholds background input and stop IDs until their closing quotes arrive", () => {
    expect(streamingCall("background_command", '{"action":"input","id":"bg-2')).toEqual({
      target: null,
      action: "input",
      added: null,
    });
    expect(streamingCall("background_command", '{"action":"stop","id":"bg-3')).toEqual({
      target: null,
      action: "stop",
      added: null,
    });
  });

  it("does not open a line a trailing newline hasn't started", () => {
    expect(streamingCall("Write", '{"file_path":"/a/c.py","content":"one\\ntwo\\n"').added).toBe(2);
  });

  it("reports zero for content that has only just opened", () => {
    expect(streamingCall("Write", '{"file_path":"/a/c.py","content":""').added).toBe(0);
  });

  it("survives a fragment that ends mid-escape", () => {
    // The tail is half an escape sequence, which is not valid JSON on its own.
    expect(streamingCall("Write", '{"file_path":"/a/c.py","content":"tail\\').added).toBe(1);
  });

  it("does not let an escaped quote end the value early", () => {
    expect(
      streamingCall("Write", '{"file_path":"/a/c.py","content":"say \\"hi\\" now\\nnext').added,
    ).toBe(2);
  });

  it("counts NotebookEdit's new_source", () => {
    expect(
      streamingCall("NotebookEdit", '{"notebook_path":"/a/n.ipynb","new_source":"x = 1\\ny = 2"')
        .added,
    ).toBe(2);
  });

  // The reviewer's bug. `content` is scanned by substring, so a nested one used
  // to match: field-keying counted the first todo as a line of an added file on
  // a call that writes no file at all.
  it("ignores a content key nested inside TodoWrite's todos", () => {
    expect(
      streamingCall("TodoWrite", '{"todos":[{"content":"Fix the bug","status":"pending"'),
    ).toEqual({ target: null, added: null });
  });

  it("ignores a content param on an unenumerated MCP tool", () => {
    expect(streamingCall("mcp__notes__save", '{"content":"one\\ntwo","title":"t"}').added).toBe(
      null,
    );
  });

  it("gives an Edit its path but no line count", () => {
    expect(
      streamingCall("Edit", '{"file_path":"/a/b/c.py","old_string":"a","new_string":"b\\nc"}'),
    ).toEqual({ target: "b/c.py", added: null });
  });

  it("keeps a command whole rather than shortening it as a path", () => {
    expect(streamingCall("Bash", '{"command":"find /a/b -name x"}').target).toBe(
      "find /a/b -name x",
    );
  });

  it("prefers the command over the description when both have landed", () => {
    expect(
      streamingCall("Bash", '{"command":"ls -la","description":"list the files"}').target,
    ).toBe("ls -la");
  });

  it("withholds a target until its closing quote arrives", () => {
    // A path printed while still arriving would grow character by character,
    // which reads as a glitch rather than as progress.
    expect(streamingCall("Write", '{"file_path":"/a/b/part').target).toBe(null);
  });

  it("reads nothing out of an empty prefix", () => {
    expect(streamingCall("Write", "")).toEqual({ target: null, added: null });
  });
});
