/**
 * Conversation titles, owned by the backend so every client shows the same one: the orchestrator's is fixed, a user's
 * rename wins, then the ACP runner's own `session_info_update` title, else a subagent's task or a chat's first message.
 */
import { defineDoc } from "@earendil-works/pi-durable";

/** `source` is who set `title`: `user` (a rename, never replaced) or `agent` (the runner); absent for a fallback. */
export const TitleDoc = defineDoc<{ title: string; source?: "user" | "agent" }>({
  kind: "viz.title",
  version: 1,
  scope: "conversation",
  history: "latest",
  fork: "initial",
  initial: () => ({ title: "" }),
  checkpointWhen: () => true,
});

export const ORCHESTRATOR_TITLE = "Orchestrator";

const MAX = 60;

/**
 * A title from a message: its first line of prose outside code fences, without Markdown markup, whitespace
 * collapsed; a long line keeps its first sentence when that fits, else is cut at a word boundary within MAX characters.
 */
export function titleFrom(text: string): string {
  let fenced = false;
  let code: string | undefined;
  for (const raw of text.split("\n")) {
    if (/^\s*(```|~~~)/.test(raw)) {
      fenced = !fenced;
      continue;
    }
    const line = clean(raw);
    if (line === "") continue;
    if (!fenced) return shorten(line);
    code ??= line;
  }
  return code === undefined ? "" : shorten(code);
}

function clean(line: string): string {
  return line
    .replace(/^\s*(?:#{1,6}\s+|>\s*|[-*+]\s+|\d+[.)]\s+|\[[ xX]\]\s+)+/, "")
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/`+|\*\*/g, "")
    .replace(/\s+/g, " ")
    .trim()
    .replace(/[\s:;,]+$/, "");
}

function shorten(line: string): string {
  const chars = [...line];
  if (chars.length <= MAX) return line;
  const head = chars.slice(0, MAX).join("");
  const sentence = /^(.{20,}?[.?!])\s/.exec(head);
  if (sentence) return sentence[1]!.replace(/\.$/, "");
  const cut = chars.slice(0, MAX - 1).join("");
  const space = cut.lastIndexOf(" ");
  return `${(space >= MAX / 2 ? cut.slice(0, space) : cut).replace(/[\s.,:;-]+$/, "")}…`;
}
