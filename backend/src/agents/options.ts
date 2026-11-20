import type { SessionConfigOption } from "@agentclientprotocol/sdk";
import { defineDoc } from "@earendil-works/pi-durable";
import { opt, str } from "@earendil-works/pi-schema";

import type { Harness as HarnessConfig } from "../config.ts";
import type { ConfigOption, ConversationState, ModelThinking } from "../protocol.ts";

export type OptionValue = ConfigOption["value"];
export type Desired = Record<string, OptionValue>;
export type Commands = ConversationState["commands"];

export type SpawnSelection = { model?: string; thinking?: string };

export const spawnSelectionFields = {
  model: opt(str({ minLength: 1, description: "Exact ACP model value from list_spawn_options" })),
  thinking: opt(str({ minLength: 1, description: "Exact thinking value for that model from list_spawn_options" })),
};

export type SpawnOptionsQuery = { harness?: string; model?: string; search?: string };
export const spawnOptionsFields = {
  harness: opt(str({ minLength: 1, description: "ACP harness name; omit to use the default when filtering" })),
  model: opt(
    str({ minLength: 1, description: "Exact model value; show its thinking levels instead of listing models" }),
  ),
  search: opt(str({ minLength: 1, description: "Filter models by name, value, or provider" })),
};
export const spawnOptionsDescription =
  "List configured ACP harnesses and the default. Pass a harness to list models, search to filter them, or a model to see its thinking levels.";

type ModelProfile = { model: string; thinking: ModelThinking | null };
export type HarnessCatalog = { name: string; options: ConfigOption[]; profiles: ModelProfile[] };

export const modelOption = (options: ConfigOption[]) => options.find(({ category }) => category === "model");
export const thinkingOption = (options: ConfigOption[]) =>
  options.find(({ category, type }) => category === "thought_level" && type === "select");

export const AcpStateDoc = defineDoc<{
  options: ConfigOption[];
  commands: Commands;
  desired: Desired;
  /** Explicit spawn selections must be applied before the first prompt, even after a restart. */
  spawnSelection?: SpawnSelection;
  images?: boolean;
  /** The runner's last ACP `usage_update`: tokens in context and the context window size. */
  usage?: { used: number; size: number };
}>({
  kind: "viz.acp.state",
  version: 1,
  scope: "conversation",
  history: "latest",
  fork: "initial",
  initial: () => ({ options: [], commands: [], desired: {} }),
  checkpointWhen: () => true,
});

export const HarnessOptionsDoc = defineDoc<{ harnesses: HarnessCatalog[] }>({
  kind: "viz.harness.options",
  version: 1,
  scope: "conversation",
  history: "latest",
  fork: "initial",
  initial: () => ({ harnesses: [] }),
  checkpointWhen: () => true,
});

export function validValue(option: ConfigOption, value: OptionValue): boolean {
  return option.type === "boolean"
    ? typeof value === "boolean"
    : option.choices.some((choice) => choice.value === value);
}

export function normalize(options: SessionConfigOption[]): ConfigOption[] {
  const normalized = options.map((option): ConfigOption => {
    const category = option.category ?? (["model", "thought_level", "mode"].includes(option.id) ? option.id : null);
    const display = (item: Parameters<typeof selectChoice>[0], group?: string) => {
      const result = selectChoice(item);
      if (category === "model") {
        // The runner's group, else the provider before a `/` in the value; a model with neither stays ungrouped.
        const separator = item.value.indexOf("/");
        const label = group ?? (separator < 0 ? undefined : item.value.slice(0, separator));
        if (label && result.name.toLowerCase().startsWith(`${label.toLowerCase()}/`)) {
          result.name = result.name.slice(label.length + 1);
        }
        return { ...result, ...(label !== undefined && { group: label }) };
      }
      if (category === "thought_level" && result.name.startsWith(`${option.name}: `)) {
        result.name = result.name.slice(option.name.length + 2);
      }
      return { ...result, ...(group !== undefined && { group }) };
    };
    return {
      id: option.id,
      name: option.name,
      ...(option.description != null && { description: option.description }),
      category,
      type: option.type,
      value: option.currentValue,
      choices:
        option.type === "boolean"
          ? []
          : option.options.flatMap((choice) =>
              "group" in choice ? choice.options.map((item) => display(item, choice.name)) : [display(choice)],
            ),
    };
  });
  const thinking = thinkingOption(normalized);
  if (!thinking) return normalized;
  // Older ACP clients used a legacy mode selector for thinking. Some runners expose it in both forms.
  return normalized.filter(
    (option) =>
      option.category !== "mode" ||
      option.type !== "select" ||
      option.choices.length !== thinking.choices.length ||
      option.choices.some(({ value }) => !validValue(thinking, value)),
  );
}

function selectChoice({ value, name, description }: { value: string; name: string; description?: string | null }) {
  return { value, name, ...(description != null && { description }) };
}

/** Keep the value learned on first observation, unless the runner no longer allows it. */
export function modelProfile(options: ConfigOption[], previous: ModelProfile[]): ModelProfile | undefined {
  const model = modelOption(options)?.value;
  if (typeof model !== "string") return undefined;
  const option = thinkingOption(options);
  if (!option) return { model, thinking: null };
  const prior = previous.find((entry) => entry.model === model)?.thinking;
  const { id, name, description, value, choices } = option;
  return {
    model,
    thinking: {
      id,
      name,
      ...(description !== undefined && { description }),
      value: prior && validValue(option, prior.value) ? prior.value : value,
      choices: choices.map(({ value, name, description }) => ({
        value,
        name,
        ...(description !== undefined && { description }),
      })),
    },
  };
}

export function withModelProfiles(
  options: ConfigOption[],
  profiles: ModelProfile[],
  configuredEffort?: string,
): ConfigOption[] {
  if (profiles.length === 0) return options;
  const thinkingByModel = new Map(profiles.map(({ model, thinking }) => [model, thinking]));
  return options.map((option) =>
    option.category === "model"
      ? {
          ...option,
          choices: option.choices.map((choice) => {
            const thinking = thinkingByModel.get(choice.value);
            if (!thinking) return choice;
            return {
              ...choice,
              thinking:
                configuredEffort !== undefined && thinking.choices.some(({ value }) => value === configuredEffort)
                  ? { ...thinking, value: configuredEffort }
                  : thinking,
            };
          }),
        }
      : option,
  );
}

export function catalogOptions(catalog: HarnessCatalog, config: HarnessConfig, selected?: string): ConfigOption[] {
  const model = modelOption(catalog.options);
  const requested = selected ?? config.model;
  const value = model && requested !== undefined && validValue(model, requested) ? requested : model?.value;
  const profile = catalog.profiles.find(({ model }) => model === value);
  const options = catalog.options
    .filter((option) => !profile || option.category !== "thought_level")
    .map((option) => (option === model && value !== undefined ? { ...option, value } : option));
  if (profile?.thinking) options.push({ ...profile.thinking, category: "thought_level", type: "select" });
  const preferred = defaults(config, options);
  return withModelProfiles(
    options.map((option) =>
      option.category !== "model" && Object.hasOwn(preferred, option.id)
        ? { ...option, value: preferred[option.id]! }
        : option,
    ),
    catalog.profiles,
    config.effort,
  );
}

export function defaults(config: HarnessConfig, options: ConfigOption[]): Desired {
  const result: Desired = {};
  for (const [key, category] of [
    ["model", "model"],
    ["effort", "thought_level"],
    ["mode", "mode"],
  ] as const) {
    const value = config[key];
    if (value === undefined) continue;
    const option = options.find((item) => item.category === category) ?? options.find((item) => item.id === key);
    if (option && validValue(option, value)) Object.defineProperty(result, option.id, { value, enumerable: true });
  }
  return result;
}
