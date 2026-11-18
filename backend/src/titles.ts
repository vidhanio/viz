/**
 * Conversation titles, owned by the backend so every client shows the same one: the orchestrator's is fixed, a
 * subagent's is its task, a chat's is its first message.
 */
import { defineDoc } from "@earendil-works/pi-durable";

export const TitleDoc = defineDoc<{ title: string }>({
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

/** A title from a message: its first non-empty line, whitespace collapsed, cut at MAX characters. */
export function titleFrom(text: string): string {
  const first = text
    .split("\n")
    .map((line) => line.replace(/\s+/g, " ").trim())
    .find((line) => line !== "");
  if (first === undefined) return "";
  const chars = [...first];
  return chars.length <= MAX
    ? first
    : `${chars
        .slice(0, MAX - 1)
        .join("")
        .trimEnd()}…`;
}
