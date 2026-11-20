import type { SessionConfigOption } from "@agentclientprotocol/sdk";
import { defineDoc } from "@earendil-works/pi-durable";

import type { Harness as HarnessConfig } from "../config.ts";
import type { ConfigOption, ConversationState } from "../protocol.ts";

export type OptionValue = ConfigOption["value"];
export type Desired = Record<string, OptionValue>;
export type Commands = ConversationState["commands"];

export const AcpStateDoc = defineDoc<{
  options: ConfigOption[];
  commands: Commands;
  desired: Desired;
  images?: boolean;
}>({
  kind: "viz.acp.state",
  version: 1,
  scope: "conversation",
  history: "latest",
  fork: "initial",
  initial: () => ({ options: [], commands: [], desired: {} }),
  checkpointWhen: () => true,
});

export const HarnessOptionsDoc = defineDoc<{ harnesses: { name: string; options: ConfigOption[] }[] }>({
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
  return options.map((option) => ({
    id: option.id,
    name: option.name,
    ...(option.description != null && { description: option.description }),
    category: option.category ?? null,
    type: option.type,
    value: option.currentValue,
    choices:
      option.type === "boolean"
        ? []
        : option.options.flatMap((choice) =>
            "group" in choice
              ? choice.options.map((item) => ({ ...selectChoice(item), group: choice.name }))
              : [selectChoice(choice)],
          ),
  }));
}

function selectChoice({ value, name, description }: { value: string; name: string; description?: string | null }) {
  return { value, name, ...(description != null && { description }) };
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
