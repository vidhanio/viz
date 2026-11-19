/**
 * The orchestrator: viz's root conversation and the tools it uses to delegate work.
 *
 * Subagents are external ACP sessions with durable, task-owned transcripts.
 */
import { homedir } from "node:os";

import type { Context } from "@earendil-works/chord/context";
import type { Models } from "@earendil-works/pi-ai";
import {
  type ConversationId,
  configure,
  defineDoc,
  defineExtension,
  defineTool,
  type Extension,
  type Harness,
  section,
} from "@earendil-works/pi-durable";
import { obj, opt, str } from "@earendil-works/pi-schema";

import { delegate } from "./agents/delegate.ts";
import type { Runners } from "./agents/runners.ts";
import type { AgentSessions } from "./agents/sessions.ts";
import type { BackendConfig } from "./config.ts";
import { TURNS_PROMPT } from "./memory/prompt.ts";
import type { ConfigOption } from "./protocol.ts";

const THINKING = ["off", "minimal", "low", "medium", "high", "xhigh"] as const;
type Thinking = (typeof THINKING)[number];
type Model = { provider: string; model: string };
export const modelRef = ({ provider, model }: Model) => ({ provider, modelId: model });
const modelKey = ({ provider, model }: Model) => `${provider}/${model}`;

export const SettingsDoc = defineDoc<{ model: Model | null; thinking: Thinking | null }>({
  kind: "viz.settings",
  version: 1,
  scope: "conversation",
  history: "latest",
  fork: "initial",
  initial: () => ({ model: null, thinking: null }),
  checkpointWhen: () => true,
});

export function orchestratorSettings(
  harness: Harness,
  root: ConversationId,
  models: Models,
  config: BackendConfig["orchestrator"],
) {
  let catalog: Promise<{ value: string; name: string; group: string; ref: Model }[]> | undefined;
  const choices = () =>
    (catalog ??= models
      .getAvailable()
      .then((available) =>
        available.map(({ provider, id, name }) => ({
          value: `${provider}/${id}`,
          name,
          group: provider,
          ref: { provider, model: id },
        })),
      )
      .catch((error: unknown) => {
        catalog = undefined;
        throw error;
      }));
  return {
    async options(context: Context): Promise<ConfigOption[]> {
      const state = await harness.snapshot(SettingsDoc, root, context);
      const available = await choices();
      return [
        {
          id: "model",
          name: "Model",
          category: "model",
          type: "select",
          value: modelKey(state?.model ?? config.model),
          choices: available.map(({ value, name, group }) => ({ value, name, group })),
        },
        {
          id: "thinking",
          name: "Thinking",
          category: "thought_level",
          type: "select",
          value: state?.thinking ?? config.thinking ?? "off",
          choices: THINKING.map((value) => ({ value, name: value[0]!.toUpperCase() + value.slice(1) })),
        },
      ];
    },
    async configure(option: string, value: ConfigOption["value"], context: Context) {
      if (option === "model") {
        const model = (await choices()).find((choice) => choice.value === value)?.ref;
        if (!model) throw new Error("Model not available");
        await harness.commit(async (tx) => {
          (await tx.doc(SettingsDoc, root)).model = model;
          await configure(tx, root, { model: modelRef(model) });
        }, context);
      } else if (option === "thinking") {
        const thinking = THINKING.find((level) => level === value);
        if (!thinking) throw new Error("Invalid thinking level");
        await harness.commit(async (tx) => {
          (await tx.doc(SettingsDoc, root)).thinking = thinking;
          await configure(tx, root, { thinkingLevel: thinking });
        }, context);
      } else throw new Error("Option not found");
    },
  };
}

export function subagentExtension(runners: Runners, sessions: AgentSessions, host: () => Harness): Extension {
  const names = runners.names.join(", ");
  return defineExtension({
    name: "viz.subagent",
    tools: [
      defineTool({
        name: "subagent",
        description: names ? "Run a task in an ACP subagent." : "No ACP harness is configured.",
        parameters: obj({
          task: str({ description: "Task and context" }),
          harness: opt(
            str({
              description: `ACP harness (${names || "none"}); omit for the default`,
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
            harness: host(),
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
