import type { SessionConfigOption, SessionModeState } from "@agentclientprotocol/sdk";
import { defineDoc } from "@earendil-works/pi-durable";

import type { Harness as HarnessConfig } from "../config.ts";
import type { ConfigOption, ConversationState } from "../protocol.ts";

export type OptionValue = ConfigOption["value"];
export type Desired = Record<string, OptionValue>;
export type Commands = ConversationState["commands"];
export type Access = "full" | "ask" | "read-only";

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

export function access(value: OptionValue | undefined, config?: HarnessConfig): Access {
  if (value === "full" || value === "ask" || value === "read-only") return value;
  return config?.permissions === "allow" ? "full" : config?.permissions === "ask" ? "ask" : "read-only";
}

export function accessOption(value: Access): ConfigOption {
  return {
    id: "access",
    name: "Access",
    category: "access",
    type: "select",
    value,
    choices: [
      { value: "full", name: "Full" },
      { value: "ask", name: "Ask" },
      { value: "read-only", name: "Read-only" },
    ],
  };
}

export function validValue(option: ConfigOption, value: OptionValue): boolean {
  return option.type === "boolean"
    ? typeof value === "boolean"
    : option.choices.some((choice) => choice.value === value);
}

export function initialDesired(config: HarnessConfig, options?: Desired): Desired {
  if (Object.hasOwn(options ?? {}, "access") && options?.access !== access(options?.access, config)) {
    throw new Error("Invalid access value");
  }
  return { ...options, access: access(options?.access, config) };
}

export function normalize(options: SessionConfigOption[]): ConfigOption[] {
  return options
    .filter((option) => option.id !== "access")
    .map((option) => ({
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

export function legacyMode(modes: SessionModeState): ConfigOption {
  return {
    id: "mode",
    category: "mode",
    name: "Mode",
    type: "select",
    value: modes.currentModeId,
    choices: modes.availableModes.map(({ id, name, description }) =>
      selectChoice({ value: id, name, ...(description != null && { description }) }),
    ),
  };
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
