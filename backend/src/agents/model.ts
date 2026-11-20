/** Durable session identity and display-only ACP transcript entries. */
import type { ToolCallStatus, ToolKind } from "@agentclientprotocol/sdk";
import { defineDoc, defineEntry } from "@earendil-works/pi-durable";

import type { StoredAttachment } from "../attachments.ts";
import type { Origin } from "../protocol.ts";
import type { Desired } from "./options.ts";

export type ToolRecord = {
  id: string;
  title: string;
  name?: string;
  kind: ToolKind | null;
  status: ToolCallStatus;
  viz?: string;
  child?: number;
  /** Unix milliseconds when viz first observed the call running. */
  startedAt?: number;
  input?: string;
  output?: string;
  diffs?: { path: string; oldText: string | null; newText: string }[];
  locations?: string[];
};

export const SessionDoc = defineDoc<{ sessionId: string | null; openValues?: Desired }>({
  kind: "viz.acp.session",
  version: 1,
  scope: "conversation",
  history: "latest",
  fork: "initial",
  initial: () => ({ sessionId: null }),
  checkpointWhen: () => true,
});

export type LiveText = { key: string; kind: "text" | "thought"; text: string };

/**
 * The running ACP turn's uncommitted tail: its text run and unfinished tools. `turn` is the turn's prompt entry ID.
 * The commit that appends these as entries, or ends the turn, clears it.
 */
export const LiveTurnDoc = defineDoc<{ turn?: string; text?: LiveText; tools: ToolRecord[] }>({
  kind: "viz.acp.live",
  version: 1,
  scope: "conversation",
  history: "latest",
  fork: "initial",
  initial: () => ({ tools: [] }),
  checkpointWhen: (_value, _ops, { deltasSinceBase }) => deltasSinceBase >= 32,
});

export const PromptEntry = defineEntry<{ text: string; origin?: Origin; attachments?: StoredAttachment[] }>(
  "viz.agent.prompt",
);
/** `key` matches the `LiveTurnDoc` text run it commits. */
export const TextEntry = defineEntry<{ text: string; key?: string }>("viz.agent.text");
export const ThoughtEntry = defineEntry<{ text: string; key?: string }>("viz.agent.thought");
export const ToolEntry = defineEntry<ToolRecord>("viz.agent.tool");
export const PlanEntry = defineEntry<{ entries: { content: string; status: string }[] }>("viz.agent.plan");
export const TurnEntry = defineEntry<{ stopReason: string; error: string | null; answer?: string }>("viz.agent.turn");
