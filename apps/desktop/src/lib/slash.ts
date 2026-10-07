/// Where the composer's slash-command picker opens, what it filters to, and how
/// a pick lands back in the text.
///
/// Pure and separated from the component for the same reason [streaming.ts]
/// is: the caret arithmetic is the part that can be wrong in ways a glance at
/// the UI won't catch, and it is cheap to pin.
///
/// [streaming.ts]: ./streaming.ts
import type { SlashCommand } from "@/types/events";

/// Commands owned by Lathe. Lathe's command registry is intentionally not exposed
/// in the composer; only its skills are useful prompt completions here.
export const DRAY_COMMANDS: SlashCommand[] = [
  {
    name: "model",
    description: "Switch the model",
    argumentHint: "<model>",
    aliases: ["models"],
    isSkill: false,
  },
  {
    name: "effort",
    description: "Set reasoning effort",
    argumentHint: "<effort>",
    aliases: [],
    isSkill: false,
  },
  {
    name: "new",
    description: "Start a new task",
    argumentHint: "",
    aliases: [],
    isSkill: false,
  },
  {
    name: "settle",
    description: "Archive this task",
    argumentHint: "",
    aliases: [],
    isSkill: false,
  },
];

/// `/new` and `/settle` only make sense while following up on a session. They
/// remain executable when typed by hand, but are not offered in the empty task
/// picker where both actions are already the current state or unavailable.
export function drayCommands(isNewTask: boolean): SlashCommand[] {
  return DRAY_COMMANDS.filter(
    (command) => !isNewTask || (command.name !== "new" && command.name !== "settle"),
  );
}

type InvocationSpan = {
  start: number;
  end: number;
  prefix: "/" | "$";
};

function invocationSpan(text: string, caret: number): InvocationSpan | null {
  if (caret < 1 || caret > text.length) return null;

  let start = caret - 1;
  while (start > 0 && !/\s/.test(text[start - 1])) start -= 1;

  const prefix = text[start];
  // Commands remain a whole-prompt construct. Skills can be referenced at the
  // start of any word, which mirrors how @file mentions work in prose.
  if (prefix !== "$" && !(prefix === "/" && start === 0)) return null;

  let end = start + 1;
  while (end < text.length && !/\s/.test(text[end])) end += 1;
  if (caret > end) return null;

  return { start, end, prefix };
}

/// The command or skill name being typed, or `null` when the caret isn't in one.
///
/// Slash commands must lead the prompt, while `$skills` can open at any word.
/// This keeps slashes in paths inert without limiting skills to command syntax.
export function slashQuery(text: string, caret: number): string | null {
  const span = invocationSpan(text, caret);
  return span ? text.slice(span.start + 1, span.end) : null;
}

export function slashPrefix(text: string, caret: number): "/" | "$" | null {
  return invocationSpan(text, caret)?.prefix ?? null;
}

/// Commands matching `query`, best first.
///
/// Ranked rather than filtered so a typed prefix beats a chance mention in some
/// other command's description. The sort is stable and an empty query preserves
/// the order supplied by Lathe and the skills probe rather than re-alphabetizing.
export function filterCommands(commands: SlashCommand[], query: string): SlashCommand[] {
  const q = query.toLowerCase();

  return commands
    .map((command) => ({ command, score: score(command, q) }))
    .filter((match) => match.score !== null)
    .sort((a, b) => a.score! - b.score!)
    .map((match) => match.command);
}

/// Keeps the two invocation menus disjoint: `/` is for commands and `$` is for
/// skills. The same composer serves new sessions and follow-ups, so this must
/// be based on the typed prefix rather than on the composer's state.
export function filterCommandsByPrefix(
  commands: SlashCommand[],
  prefix: "/" | "$",
): SlashCommand[] {
  return commands.filter((command) => command.isSkill === (prefix === "$"));
}

/// `null` when the command doesn't match at all. Lower is better.
function score(command: SlashCommand, query: string): number | null {
  const name = command.name.toLowerCase();
  if (name.startsWith(query)) return 0;
  if (command.aliases.some((alias) => alias.toLowerCase().startsWith(query))) return 1;

  // A namespaced command should still be findable by its bare half, so
  // `railway:deploy` matches "deploy" ahead of anything that only mentions it.
  if (name.slice(name.indexOf(":") + 1).startsWith(query)) return 2;
  if (name.includes(query)) return 3;
  if (command.description.toLowerCase().includes(query)) return 4;

  return null;
}

/// Replaces the command or skill being typed with its user-facing prefix.
export function applyCommand(
  text: string,
  name: string,
  isSkill = false,
  caret = text.length,
): { text: string; caret: number } {
  const span = invocationSpan(text, caret);
  const start = span?.start ?? 0;
  const end = span?.end ?? (text.search(/\s/) === -1 ? text.length : text.search(/\s/));
  const head = `${isSkill ? "$" : "/"}${name}`;
  const suffix = text.slice(end);
  const separator = suffix ? "" : " ";
  const caretOffset = separator || /^\s/.test(suffix) ? 1 : 0;

  return {
    text: text.slice(0, start) + head + separator + suffix,
    caret: start + head.length + caretOffset,
  };
}

/// The first argument of a leading Lathe command, used for model and effort
/// completion after `/model ` or `/effort `. The returned span excludes the
/// command and whitespace so it can be replaced without disturbing the prompt.
export function slashArgumentQuery(
  text: string,
  caret: number,
  commandNames: readonly string[],
): { commandName: string; query: string } | null {
  if (!text.startsWith("/") || caret < 1 || caret > text.length) return null;

  const commandEnd = text.search(/\s/);
  if (commandEnd === -1) return null;
  const end = commandEnd;
  const commandName = text.slice(1, end).toLowerCase();
  if (!commandNames.includes(commandName) || caret <= end) return null;

  let argumentStart = end;
  while (argumentStart < text.length && /\s/.test(text[argumentStart])) argumentStart += 1;
  let argumentEnd = argumentStart;
  while (argumentEnd < text.length && !/\s/.test(text[argumentEnd])) argumentEnd += 1;
  if (caret > argumentEnd) return null;

  return { commandName, query: text.slice(argumentStart, argumentEnd) };
}

/// Replaces the active first argument of a Lathe command and leaves the caret
/// ready for the command's remaining arguments.
export function applyCommandArgument(
  text: string,
  _commandName: string,
  value: string,
  _caret = text.length,
): { text: string; caret: number } {
  const commandEnd = text.search(/\s/);
  const end = commandEnd === -1 ? text.length : commandEnd;
  let argumentStart = end;
  while (argumentStart < text.length && /\s/.test(text[argumentStart])) argumentStart += 1;
  let argumentEnd = argumentStart;
  while (argumentEnd < text.length && !/\s/.test(text[argumentEnd])) argumentEnd += 1;

  const suffix = text.slice(argumentEnd);
  const separator = suffix ? "" : " ";
  const next = text.slice(0, argumentStart) + value + separator + suffix;
  const nextCaret = argumentStart + value.length + (separator || /^\s/.test(suffix) ? 1 : 0);
  return { text: next, caret: Math.min(nextCaret, next.length) };
}

/// Splits a sent leading command or skill into its name and arguments.
export function parseSlashCommand(text: string): { name: string; args: string } | null {
  const match = /^([/$])([^\s/]\S*)(.*)$/s.exec(text);
  if (!match) return null;

  return { name: match[2], args: match[3].trim() };
}
