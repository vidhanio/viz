/** Opens viz's pi-durable harness over SQLite in the state directory. */
import { mkdir } from "node:fs/promises";
import { homedir } from "node:os";
import { join } from "node:path";

import type { Context } from "@earendil-works/chord/context";
import type { MutableModels } from "@earendil-works/pi-ai/models";
import { builtinModels } from "@earendil-works/pi-ai/providers/all";
import { type Conversation, createRegistry, Harness, type ModelRef } from "@earendil-works/pi-durable";
import { NodeExecutionEnv } from "@earendil-works/pi-durable/env/node";
import { openNodeSqliteStorage } from "@earendil-works/pi-durable/storage/sqlite/node";
import { CodingTools } from "@earendil-works/pi-durable/tools";

import type { LoadedConfig } from "./config.ts";
import { Orchestrator, subagentExtension } from "./orchestrator.ts";

export const DEFAULT_MODEL: ModelRef = { provider: "anthropic", modelId: "claude-opus-5-5" };

export type VizHarness = {
  readonly harness: Harness;
  /** The orchestrator: pi-durable's reserved root conversation. */
  readonly root: Conversation;
};

export type OpenOptions = {
  /** Model access; default every built-in pi-ai provider, authenticated from the environment. */
  readonly models?: MutableModels;
  /** Default `<stateDir>/viz.sqlite`; `:memory:` for tests. */
  readonly database?: string;
};

export async function openHarness(
  { paths, config }: LoadedConfig,
  options: OpenOptions,
  context: Context,
): Promise<VizHarness> {
  const database = options.database ?? join(paths.stateDir, "viz.sqlite");
  if (database !== ":memory:") await mkdir(paths.stateDir, { recursive: true, mode: 0o700 });

  const toRef = (m: { provider: string; model: string } | undefined) => m && { provider: m.provider, modelId: m.model };
  const orchestrator = config.orchestrator ?? {};
  const Subagent = subagentExtension(toRef(config.subagents?.model));

  const registry = createRegistry();
  registry.install(CodingTools);
  registry.install(Orchestrator);
  registry.install(Subagent);

  const harness = await Harness.open(
    await openNodeSqliteStorage(database),
    {
      models: options.models ?? builtinModels(),
      registry,
      env: ({ cwd }) => new NodeExecutionEnv({ cwd: cwd ?? homedir() }),
    },
    context,
  );
  // Created with these settings on first open; later config changes are applied below.
  const root = await harness.root(context, { agent: { extensions: [CodingTools, Orchestrator, Subagent] } });
  await root.configure(
    {
      model: toRef(orchestrator.model) ?? DEFAULT_MODEL,
      thinkingLevel: orchestrator.thinking ?? "medium",
      instructions: orchestrator.instructions ?? null,
      cwd: orchestrator.cwd ?? homedir(),
    },
    context,
  );
  harness.resume();
  return { harness, root };
}
