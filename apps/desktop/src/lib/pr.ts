import type { PrMark } from "@/types/events";

/// The branch a session's work lands on, for the PR lookup and the header.
///
/// `observed` is the current checkout branch, including worktrees. The recorded
/// branch keeps the header stable while that Git read is pending.
///
/// One function because the header and the PR lookup have to agree about which
/// branch the session is on; two rebuilding it apart is how they come to
/// disagree about which PR it has.
export function sessionBranch(
  session: {
    branch: string | null;
    worktreeName?: string | null;
  },
  observed?: string | null,
): string | null {
  if (observed) return observed;
  return session.branch;
}

/// Which pull request a sidebar row draws, out of every one on its branch.
///
/// One branch can carry several — the same fix opened against `main` and a
/// release branch, or a follow-up opened after the first one landed — and the
/// row has space for exactly one glyph. Live work outranks a record: an open PR
/// wins over a merged one however recently that merged, because the mark's job
/// is "what is there to do here", and a draft beats a merged one for the same
/// reason. Only where *nothing* is open does the row say merged, which is
/// precisely when that is the useful thing to say: the work landed and the
/// session can be settled.
///
/// Ties inside a rank go to the first, and the backend hands both halves back
/// ordered by most recently updated — so a branch with two open PRs marks
/// itself from the one being worked on.
export function pickPrMark(prs: PrMark[]): PrMark | undefined {
  const rank = (pr: PrMark) => (pr.state !== "OPEN" ? 2 : pr.isDraft ? 1 : 0);

  let best: PrMark | undefined;
  for (const pr of prs) {
    if (!best || rank(pr) < rank(best)) best = pr;
  }
  return best;
}

/// The fields the verdict below turns on, and no more — so a sidebar mark can
/// be judged without fetching detailed pull request data. Nullable because the
/// marks query asks for them on its open half alone.
export type MergeState = {
  state: string;
  isDraft: boolean;
  mergeable: string | null;
  mergeStateStatus: string | null;
};

/// Why a pull request can or cannot land, as one word.
///
/// GitHub answers this across three fields that overlap — `state`, `mergeable`,
/// `mergeStateStatus` — and the order they're read in is the whole of the
/// logic: a closed PR's merge state is stale, a conflict outranks a failing
/// check, and `UNKNOWN` means GitHub hasn't worked it out rather than that the
/// answer is no. Reading them in any other order puts "Ready to merge" above a
/// conflict.
///
export type MergeVerdict =
  | "merged"
  | "closed"
  | "conflict"
  | "unknown"
  | "draft"
  | "behind"
  | "blocked"
  | "unstable"
  | "ready";

export function mergeVerdict(pr: MergeState): MergeVerdict {
  if (pr.state === "MERGED") return "merged";
  if (pr.state === "CLOSED") return "closed";

  if (pr.mergeable === "CONFLICTING" || pr.mergeStateStatus === "DIRTY") return "conflict";

  // `UNKNOWN` is asked lazily by GitHub, so the first read of a fresh PR lands
  // here and the next poll settles it. A *null* is the other way of not
  // knowing — the sidebar's marks carry these for open pull requests only — and
  // it has to land in the same place: not having asked is not an answer, and
  // reading it as one would announce a merged PR as ready to merge.
  if (pr.mergeable === "UNKNOWN" || pr.mergeable === null || pr.mergeStateStatus === null) {
    return "unknown";
  }

  if (pr.isDraft) return "draft";

  switch (pr.mergeStateStatus) {
    case "BEHIND":
      return "behind";
    case "BLOCKED":
      return "blocked";
    case "UNSTABLE":
      return "unstable";
    // GitHub's own word for "I cannot work this out right now", and the second
    // way this field says nothing — `mergeable` settling to `MERGEABLE` does not
    // settle this one, so it has to be named rather than left to the arm below.
    // Read as ready it puts a merge button under a PR that cannot take one, and
    // now also sounds a notification for it.
    case "UNKNOWN":
      return "unknown";
    // What is left is `CLEAN` and `HAS_HOOKS` — nothing in the way, with or
    // without a pre-receive hook to run on the way in.
    default:
      return "ready";
  }
}

/// Whether it can land right now — nothing in the way, nothing left to wait
/// for. What the "Ready to merge" notice fires on.
export function isReadyToMerge(pr: MergeState): boolean {
  return mergeVerdict(pr) === "ready";
}

/// One step of "which pull requests have just become ready", against what they
/// last said. `observed` is every session being watched paired with its answer
/// now; the result is what to remember and which sessions changed.
///
/// **A session seen for the first time changes nothing.** Its answer is
/// recorded and no more, because the app cannot tell a pull request that turned
/// green a moment ago from one that has been green for a week — so announcing
/// on a first sighting would open a card for every landed branch in the list
/// each time the app starts, or a project is switched back to.
///
/// **Pruning falls out of building the map from `observed` alone.** A session
/// that stops being watched — archived, filtered away, its repo's read not
/// landed — is forgotten rather than carried, so it comes back as a first
/// sighting. That is the safe direction: re-seeding costs one announcement that
/// was never made, where remembering a `false` across an absence would raise
/// one for a pull request that was ready the whole time the row was gone.
export function readyTransitions(
  prev: Map<string, boolean>,
  observed: Iterable<readonly [string, boolean]>,
): { next: Map<string, boolean>; became: string[] } {
  const next = new Map<string, boolean>();
  const became: string[] = [];

  for (const [sessionId, ready] of observed) {
    next.set(sessionId, ready);
    if (ready && prev.get(sessionId) === false) became.push(sessionId);
  }

  return { next, became };
}
