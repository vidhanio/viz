/** Durable session identity and display-only ACP transcript entries. */
import type { ToolCallStatus, ToolKind } from "@agentclientprotocol/sdk";
import { defineDoc, defineEntry } from "@earendil-works/pi-durable";

import type { StoredAttachment } from "../attachments.ts";
import type { Origin } from "../protocol.ts";
import type { Desired } from "./options.ts";

export type ToolRecord = {
  id: string;
  title: string;
  kind: ToolKind | null;
  status: ToolCallStatus;
  viz?: string;
  child?: number;
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

export const PromptEntry = defineEntry<{ text: string; origin?: Origin; attachments?: StoredAttachment[] }>(
  "viz.agent.prompt",
);
export const TextEntry = defineEntry<{ text: string }>("viz.agent.text");
export const ThoughtEntry = defineEntry<{ text: string }>("viz.agent.thought");
export const ToolEntry = defineEntry<ToolRecord>("viz.agent.tool");
export const PlanEntry = defineEntry<{ entries: { content: string; status: string }[] }>("viz.agent.plan");
export const ProgressEntry = defineEntry<{ text: string }>("viz.agent.progress");
export const TurnEntry = defineEntry<{ stopReason: string; error: string | null; answer?: string }>("viz.agent.turn");
