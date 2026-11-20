/** The orchestrator: viz's root conversation and its turn instructions. */

import type { Context } from "@earendil-works/chord/context";
import {
  clampThinkingLevel,
  getSupportedThinkingLevels,
  type ModelThinkingLevel,
  type Models,
} from "@earendil-works/pi-ai";
import {
  type ConversationId,
  configure,
  defineDoc,
  defineExtension,
  type Extension,
  type Harness,
  section,
} from "@earendil-works/pi-durable";

import type { BackendConfig } from "./config.ts";
import { TURNS_PROMPT } from "./memory/prompt.ts";
import type { ConfigOption, ModelThinking } from "./protocol.ts";

type Model = { provider: string; model: string };
export const modelRef = ({ provider, model }: Model) => ({ provider, modelId: model });
const modelKey = ({ provider, model }: Model) => `${provider}/${model}`;

export const SettingsDoc = defineDoc<{ model: Model | null; thinking: ModelThinkingLevel | null }>({
  kind: "viz.settings",
  version: 1,
  scope: "conversation",
  history: "latest",
  fork: "initial",
  initial: () => ({ model: null, thinking: null }),
  checkpointWhen: () => true,
});

/** Clamp the persisted or configured level before starting the root (which may have changed providers). */
export function orchestratorThinking(models: Models, { provider, model }: Model, value?: ModelThinkingLevel | null) {
  const entry = models.getModel(provider, model);
  return entry ? clampThinkingLevel(entry, value ?? "off") : "off";
}

export function orchestratorSettings(
  harness: Harness,
  root: ConversationId,
  models: Models,
  config: BackendConfig["orchestrator"],
) {
  let catalog:
    Promise<{ value: string; name: string; group: string; ref: Model; levels: ModelThinkingLevel[] }[]> | undefined;
  const choices = () =>
    (catalog ??= models
      .getAvailable()
      .then((available) =>
        available.map((entry) => {
          const { provider, id, name } = entry;
          return {
            value: `${provider}/${id}`,
            name: name.startsWith(`${provider}/`) ? name.slice(provider.length + 1) : name,
            group: models.getProvider(provider)?.name ?? provider,
            ref: { provider, model: id },
            levels: getSupportedThinkingLevels(entry),
          };
        }),
      )
      .catch((error: unknown) => {
        catalog = undefined;
        throw error;
      }));
  const thinkingFor = (
    model: Model,
    levels: ModelThinkingLevel[],
    preferred: ModelThinkingLevel,
  ): ModelThinking | undefined =>
    levels.length > 1
      ? {
          id: "thinking",
          name: "Thinking",
          value: orchestratorThinking(models, model, preferred),
          choices: levels.map((value) => ({ value, name: value[0]!.toUpperCase() + value.slice(1) })),
        }
      : undefined;
  return {
    async options(context: Context): Promise<ConfigOption[]> {
      const state = await harness.snapshot(SettingsDoc, root, context);
      const available = await choices();
      const current = modelKey(state?.model ?? config.model);
      const preferred = state?.thinking ?? config.thinking ?? "off";
      const choicesByModel = available.map(({ value, name, group, ref, levels }) => {
        const thinking = thinkingFor(ref, levels, preferred);
        return { value, name, group, ...(thinking && { thinking }) };
      });
      const thinking = choicesByModel.find(({ value }) => value === current)?.thinking;
      return [
        {
          id: "model",
          name: "Model",
          category: "model",
          type: "select",
          value: current,
          choices: choicesByModel,
        },
        ...(thinking ? [{ ...thinking, category: "thought_level", type: "select" } as const] : []),
      ];
    },
    async configure(option: string, value: ConfigOption["value"], context: Context) {
      if (option === "model") {
        const model = (await choices()).find((choice) => choice.value === value)?.ref;
        if (!model) throw new Error("Model not available");
        await harness.commit(async (tx) => {
          const settings = await tx.doc(SettingsDoc, root);
          const thinking = orchestratorThinking(models, model, settings.thinking ?? config.thinking);
          settings.model = model;
          settings.thinking = thinking;
          await configure(tx, root, { model: modelRef(model), thinkingLevel: thinking });
        }, context);
      } else if (option === "thinking") {
        if (typeof value !== "string") throw new Error("Invalid thinking level");
        await harness.commit(async (tx) => {
          const settings = await tx.doc(SettingsDoc, root);
          const { provider, model } = settings.model ?? config.model;
          const entry = models.getModel(provider, model);
          const levels = entry ? getSupportedThinkingLevels(entry) : [];
          const thinking = levels.find((level) => level === value);
          if (!thinking || levels.length < 2) throw new Error("Invalid thinking level for this model");
          settings.thinking = thinking;
          await configure(tx, root, { thinkingLevel: thinking });
        }, context);
      } else throw new Error("Option not found");
    },
  };
}

export const Orchestrator: Extension = defineExtension({
  name: "viz.orchestrator",
  sections: [section("turns", () => TURNS_PROMPT, { tag: false })],
});
