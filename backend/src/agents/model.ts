/** Durable session identity and display-only ACP transcript entries. */
import type { StopReason, ToolCallStatus, ToolKind } from "@agentclientprotocol/sdk";
import { defineDoc, defineEntry } from "@earendil-works/pi-durable";

import type { Origin } from "../protocol.ts";

export type ToolRecord = {
  id: string;
  title: string;
  kind: ToolKind | null;
  status: ToolCallStatus;
  input?: string;
  output?: string;
  diffs?: { path: string; oldText: string | null; newText: string }[];
  locations?: string[];
};

export type Outcome =
  { kind: "running" } | { kind: "done"; answer: string; stopReason: StopReason } | { kind: "failed"; error: string };

type AgentState = {
  harness: string;
  outcome: Outcome | null;
};

/** A nonempty harness marks a delegated transcript, not a pi-durable model conversation. */
export const AgentDoc = defineDoc<AgentState>({
  kind: "viz.agent",
  version: 1,
  scope: "conversation",
  history: "latest",
  fork: "initial",
  initial: () => ({ harness: "", outcome: null }),
  checkpointWhen: () => true,
});

export const SessionDoc = defineDoc<{ sessionId: string | null; legacyMode?: boolean }>({
  kind: "viz.acp.session",
  version: 1,
  scope: "conversation",
  history: "latest",
  fork: "initial",
  initial: () => ({ sessionId: null }),
  checkpointWhen: () => true,
});

export const PromptEntry = defineEntry<{ text: string; origin?: Origin }>("viz.agent.prompt");
export const TextEntry = defineEntry<{ text: string }>("viz.agent.text");
export const ThoughtEntry = defineEntry<{ text: string }>("viz.agent.thought");
export const ToolEntry = defineEntry<ToolRecord>("viz.agent.tool");
export const PlanEntry = defineEntry<{ entries: { content: string; status: string }[] }>("viz.agent.plan");
export const ProgressEntry = defineEntry<{ text: string }>("viz.agent.progress");
export const TurnEntry = defineEntry<{ stopReason: string; error: string | null; answer?: string }>("viz.agent.turn");
