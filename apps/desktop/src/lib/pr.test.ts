import { describe, expect, it } from "vitest";

import {
  isReadyToMerge,
  readyTransitions,
  pickPrMark,
  sessionBranch,
} from "./pr";
import type { PrMark, PullRequest } from "@/types/events";

function pr(over: Partial<PullRequest> = {}): PullRequest {
  return {
    number: 1,
    title: "t",
    url: "u",
    state: "OPEN",
    isDraft: false,
    author: "a",
    baseRefName: "main",
    headRefName: "feature",
    mergeable: "MERGEABLE",
    mergeStateStatus: "CLEAN",
    reviewDecision: null,
    checks: [],
    comments: [],
    additions: 0,
    deletions: 0,
    changedFiles: 0,
    updatedAt: "",
    ...over,
  };
}

describe("sessionBranch", () => {
  it("uses the recorded branch before a worktree Git read lands", () => {
    expect(sessionBranch({ branch: "main", worktreeName: "calm-owl" })).toBe("main");
  });

  it("uses the checked-out branch otherwise", () => {
    expect(sessionBranch({ branch: "feature", worktreeName: null })).toBe("feature");
    expect(sessionBranch({ branch: null, worktreeName: null })).toBeNull();
  });

  // Git's live reading wins over the branch recorded at creation. Anything
  // checking out another branch inside the tree should update the PR lookup.
  it("lets git's own reading of HEAD outrank the guess", () => {
    expect(
      sessionBranch({ branch: "main", worktreeName: "calm-owl" }, "fix/thing"),
    ).toBe("fix/thing");
    expect(sessionBranch({ branch: "feature", worktreeName: null }, "fix/thing")).toBe(
      "fix/thing",
    );
  });

  // The read is per-session and lands a frame late, and a non-repo has no
  // branch at all — so both fall back rather than drawing nothing.
  it("falls back while there is no reading to use", () => {
    expect(sessionBranch({ branch: "main", worktreeName: "calm-owl" }, null)).toBe("main");
    expect(sessionBranch({ branch: "feature", worktreeName: null }, undefined)).toBe(
      "feature",
    );
  });
});

describe("isReadyToMerge", () => {
  const mark = (over: Partial<PrMark> = {}): PrMark => ({
    number: 1,
    headRefName: "feat/x",
    isDraft: false,
    state: "OPEN",
    checksState: "CLEAR",
    mergeable: "MERGEABLE",
    mergeStateStatus: "CLEAN",
    ...over,
  });

  // Both compact sidebar marks and full PR data use the same readiness rule.
  it("answers for a sidebar mark and for a full pull request alike", () => {
    expect(isReadyToMerge(mark())).toBe(true);
    expect(isReadyToMerge(pr())).toBe(true);
  });

  it("is false for everything standing in the way", () => {
    expect(isReadyToMerge(mark({ isDraft: true }))).toBe(false);
    expect(isReadyToMerge(mark({ mergeable: "CONFLICTING" }))).toBe(false);
    expect(isReadyToMerge(mark({ mergeStateStatus: "BLOCKED" }))).toBe(false);
    expect(isReadyToMerge(mark({ mergeStateStatus: "BEHIND" }))).toBe(false);
    expect(isReadyToMerge(mark({ mergeStateStatus: "UNSTABLE" }))).toBe(false);
  });

  // The marks query asks for these on its open half alone, so a merged mark
  // carries nulls. Reading a null as "nothing in the way" would announce work
  // that has already landed as ready to land.
  it("reads unasked fields as not knowing, never as ready", () => {
    expect(isReadyToMerge(mark({ state: "MERGED", mergeable: null, mergeStateStatus: null }))).toBe(
      false,
    );
    expect(isReadyToMerge(mark({ mergeable: null }))).toBe(false);
    expect(isReadyToMerge(mark({ mergeStateStatus: null }))).toBe(false);
    // Same answer for GitHub's own two ways of not knowing. `mergeable` lands
    // there on the first read of a fresh PR; `mergeStateStatus` can be there
    // while `mergeable` has already settled, which is the case that fired a
    // notification sound for a PR that could not be merged.
    expect(isReadyToMerge(mark({ mergeable: "UNKNOWN" }))).toBe(false);
    expect(isReadyToMerge(mark({ mergeStateStatus: "UNKNOWN" }))).toBe(false);
  });
});

describe("readyTransitions", () => {
  // Launching onto a sidebar of work that landed last week must not open a card
  // for every row of it. The app cannot tell that from a PR that turned green a
  // second ago, so the first sighting is only ever recorded.
  it("records a first sighting without announcing it", () => {
    const { next, became } = readyTransitions(new Map(), [
      ["a", true],
      ["b", false],
    ]);
    expect(became).toEqual([]);
    expect(next.get("a")).toBe(true);
    expect(next.get("b")).toBe(false);
  });

  it("announces the change out of not-ready, once", () => {
    const first = readyTransitions(new Map([["a", false]]), [["a", true]]);
    expect(first.became).toEqual(["a"]);

    // The same answer again is not news.
    expect(readyTransitions(first.next, [["a", true]]).became).toEqual([]);
  });

  // A check re-runs, goes red and comes back green: that is a pull request
  // becoming ready again, and saying so is right.
  it("announces again after it stops being ready", () => {
    const ready = readyTransitions(new Map([["a", false]]), [["a", true]]);
    const broke = readyTransitions(ready.next, [["a", false]]);
    expect(broke.became).toEqual([]);
    expect(readyTransitions(broke.next, [["a", true]]).became).toEqual(["a"]);
  });

  // Archived, filtered away, or its repo's read simply not landed. Forgetting
  // is the safe direction: it costs an announcement that was never made, where
  // remembering the `false` would raise one for a PR that had been ready the
  // whole time the row was off screen.
  it("forgets a session it stops being told about", () => {
    const seeded = readyTransitions(new Map([["a", false]]), [["a", false]]);
    const away = readyTransitions(seeded.next, []);
    expect(away.next.size).toBe(0);
    expect(readyTransitions(away.next, [["a", true]]).became).toEqual([]);
  });
});

describe("pickPrMark", () => {
  const mark = (over: Partial<PrMark> = {}): PrMark => ({
    number: 1,
    headRefName: "feat/x",
    isDraft: false,
    state: "OPEN",
    checksState: "CLEAR",
    mergeable: null,
    mergeStateStatus: null,
    ...over,
  });

  it("has nothing to pick from an empty list", () => {
    expect(pickPrMark([])).toBeUndefined();
  });

  // A branch whose first PR landed and whose follow-up is still open reads as
  // live work, not as done.
  it("takes an open one over a merged one whichever order they arrive in", () => {
    const open = mark({ number: 9 });
    const merged = mark({ number: 4, state: "MERGED" });
    expect(pickPrMark([merged, open])).toBe(open);
    expect(pickPrMark([open, merged])).toBe(open);
  });

  it("takes a draft over a merged one, for the same reason", () => {
    const draft = mark({ number: 9, isDraft: true });
    const merged = mark({ number: 4, state: "MERGED" });
    expect(pickPrMark([merged, draft])).toBe(draft);
  });

  it("takes a real open one over a draft", () => {
    const draft = mark({ number: 9, isDraft: true });
    const open = mark({ number: 8 });
    expect(pickPrMark([draft, open])).toBe(open);
  });

  // Only where nothing is live, which is exactly when "this landed, settle it"
  // is the useful thing for the row to say.
  it("says merged when that is all there is", () => {
    const merged = mark({ number: 4, state: "MERGED" });
    expect(pickPrMark([merged])).toBe(merged);
  });

  // Each half arrives most-recently-updated first, so the first of a rank is
  // the one being worked on.
  it("keeps the first within a rank", () => {
    const newer = mark({ number: 9 });
    const older = mark({ number: 2 });
    expect(pickPrMark([newer, older])).toBe(newer);
  });
});
