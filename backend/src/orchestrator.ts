/**
 * The orchestrator: viz's root conversation and the tools it uses to delegate work.
 *
 * Subagents are external ACP sessions with durable, task-owned transcripts.
 */
import { homedir } from "node:os";

import { defineExtension, defineTool, type Extension, section } from "@earendil-works/pi-durable";
import { obj, opt, str } from "@earendil-works/pi-schema";

import { delegate } from "./agents/delegate.ts";
import type { Runners } from "./agents/runners.ts";
import type { AgentSessions } from "./agents/sessions.ts";
import { TURNS_PROMPT } from "./memory/prompt.ts";

export function subagentExtension(runners: Runners, sessions: AgentSessions): Extension {
  const names = runners.names.join(", ");
  return defineExtension({
    name: "viz.subagent",
    tools: [
      defineTool({
        name: "subagent",
        description:
          names === ""
            ? "External delegation is unavailable until an ACP harness is configured."
            : "Run a self-contained task in an external ACP harness and get its answer back.",
        parameters: obj({
          task: str({ description: "What the subagent should do, with all context it needs" }),
          harness: opt(
            str({
              description: `Configured ACP harness name (${names || "none"}); omit to use the default runner`,
            }),
          ),
        }),
        // A rerun returns a committed outcome or reports interruption; it never resends a prompt.
        replay: "safe",
        execute: async ({ task, harness }, api, context) => {
          const { text, conversationId, isError } = await delegate(api, context, {
            select: () => runners.require(harness),
            task,
            cwd: (await api.agent(context)).cwd ?? homedir(),
            sessions,
          });
          return { output: [{ type: "text", text }], details: { conversationId }, isError };
        },
      }),
    ],
  });
}

export const Orchestrator: Extension = defineExtension({
  name: "viz.orchestrator",
  sections: [section("turns", () => TURNS_PROMPT, { tag: false })],
});
