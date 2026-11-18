/** Opens viz's pi-durable harness over SQLite in the state directory, with the orchestrator's memory beside it. */
import { mkdir } from "node:fs/promises";
import { homedir } from "node:os";
import { join } from "node:path";

import type { Context } from "@earendil-works/chord/context";
import type { WatchHandle } from "@earendil-works/chord/watch";
import { InMemoryCredentialStore } from "@earendil-works/pi-ai";
import type { MutableModels } from "@earendil-works/pi-ai/models";
import { builtinModels } from "@earendil-works/pi-ai/providers/all";
import {
  type Conversation,
  configure,
  type ConversationView,
  createRegistry,
  Harness,
  ROOT_CONVERSATION_ID,
  LiveDoc,
  type ModelRef,
  type Submission,
} from "@earendil-works/pi-durable";
import { NodeExecutionEnv } from "@earendil-works/pi-durable/env/node";
import { openNodeSqliteStorage } from "@earendil-works/pi-durable/storage/sqlite/node";
import { CodingTools } from "@earendil-works/pi-durable/tools";

import { type ChatInput, chatScheduler } from "./agents/chat.ts";
import { Runners } from "./agents/runners.ts";
import { AgentSessions } from "./agents/sessions.ts";
import { type LoadedConfig, resolveSecret, stateDir } from "./config.ts";
import { type MemoryBinding, memoryExtension } from "./memory/extension.ts";
import { type Compactor, Memory } from "./memory/memory.ts";
import { MEMORY_PROMPT } from "./memory/prompt.ts";
import { Sync } from "./memory/sync.ts";
import { Orchestrator, subagentExtension } from "./orchestrator.ts";
import { ORCHESTRATOR_TITLE, TitleDoc } from "./titles.ts";

export type VizHarness = {
  readonly harness: Harness;
  /** The orchestrator: pi-durable's reserved root conversation. */
  readonly root: Conversation;
  /**
   * Submits an input. To the orchestrator, as uniichat turns go: an input while it works joins the running turn
   * (steered in between tool calls); one while it is idle starts a new turn from a fresh context, whose past reaches
   * the model only through the memory's view. Other conversations queue inputs as follow-ups.
   */
  submit(target: Conversation, input: ChatInput, context: Context): Promise<Submission>;
  /** A new ownerless chat bound to the default ACP harness. */
  createChat(context: Context): Promise<Conversation>;
  close(context: Context): Promise<void>;
};

export type OpenOptions = {
  /** Model access; default every built-in pi-ai provider, authenticated from the environment. */
  readonly models?: MutableModels;
  readonly version: string;
};

export async function openHarness(loaded: LoadedConfig, options: OpenOptions, context: Context): Promise<VizHarness> {
  const { config } = loaded;
  const runners = new Runners(config);
  const dir = stateDir(loaded);
  await mkdir(dir, { recursive: true, mode: 0o700 });
  // `[providers.<id>].api_key` first; pi-ai falls back to each provider's environment variable.
  const credentials = new InMemoryCredentialStore();
  for (const [provider, { api_key }] of Object.entries(config.providers ?? {})) {
    const key = await resolveSecret(api_key);
    await credentials.modify(provider, async () => ({ type: "api_key", key }));
  }
  const models = options.models ?? builtinModels({ credentials });
  const ref = ({ provider, model }: { provider: string; model: string }): ModelRef => ({ provider, modelId: model });

  const { orchestrator, memory: memoryConfig } = config;
  const compactorModel = models.getModel(memoryConfig.compactor.provider, memoryConfig.compactor.model);
  if (compactorModel === undefined) {
    throw new Error(`[memory].compactor: no model ${memoryConfig.compactor.provider}/${memoryConfig.compactor.model}`);
  }
  const reasoning = memoryConfig.thinking === "off" ? undefined : memoryConfig.thinking;
  const compactor: Compactor = async (blocks, signal) => {
    const answer = await models.completeSimple(
      compactorModel,
      {
        systemPrompt: MEMORY_PROMPT,
        messages: [
          { role: "user", content: blocks.map((b) => ({ type: "text", text: b.text })), timestamp: Date.now() },
        ],
      },
      // A stable session: providers that route or cache by session (OpenCode requires one) see one memory.
      { signal, sessionId: "viz-memory", ...(reasoning && { reasoning }) },
    );
    if (answer.stopReason === "error" || answer.stopReason === "aborted") {
      return { ok: false, error: answer.errorMessage ?? answer.stopReason };
    }
    return { ok: true, text: answer.content.flatMap((c) => (c.type === "text" ? [c.text] : [])).join("") };
  };

  const memory = new Memory(join(dir, "memory.sqlite"), compactor);
  const binding: MemoryBinding = {};
  const Memorize = memoryExtension(memory, binding);
  const cwd = orchestrator.cwd ?? homedir();
  const sessions = new AgentSessions(options.version);
  const chats = chatScheduler(runners, sessions, cwd);
  const Subagent = subagentExtension(runners, sessions);

  const registry = createRegistry();
  registry.install(CodingTools);
  registry.install(Memorize);
  registry.install(Orchestrator);
  registry.install(Subagent);
  registry.install(chats.extension);

  const harness = await Harness.open(
    await openNodeSqliteStorage(join(dir, "viz.sqlite")),
    {
      models,
      registry,
      env: (target) =>
        target.conversationId === ROOT_CONVERSATION_ID ? new NodeExecutionEnv({ cwd: target.cwd ?? cwd }) : undefined,
      conversationCreated: async (tx, { id }) => {
        if (id !== ROOT_CONVERSATION_ID) {
          // pi-durable task-owned children otherwise inherit the root's native agent.
          await configure(tx, id, {
            model: null,
            extensions: null,
            instructions: null,
            tools: null,
            thinkingLevel: null,
          });
        }
      },
    },
    context,
  );
  const root = await harness.root(context);
  // Re-applied on every start, so config changes take effect.
  await root.configure(
    {
      model: ref(orchestrator.model),
      thinkingLevel: orchestrator.thinking ?? null,
      instructions: orchestrator.instructions ?? null,
      cwd,
      extensions: [CodingTools, Memorize, Orchestrator, Subagent],
    },
    context,
  );
  await harness.commit(async (tx) => {
    const doc = await tx.doc(TitleDoc, root.id);
    if (doc.title !== ORCHESTRATOR_TITLE) doc.title = ORCHESTRATOR_TITLE;
  }, context);

  // A previous version could leave native non-root tasks in storage. Mark them before starting the scheduler.
  const { tasks } = await harness.inspect(context);
  for (const { record } of tasks) {
    if (record.conversationId !== root.id && record.kind.startsWith("pi.")) {
      await harness.abortTask(record.id, context);
    }
  }

  const sync = new Sync(memory, root, context);
  binding.sync = sync;
  binding.root = root.id;
  await sync.run();
  // Every commit to the orchestrator feeds the memory as it comes, so its summaries build ahead of the next turn.
  const watch: WatchHandle<ConversationView> = await root.watch(context);
  watch.start(async () => void sync.run().catch((error: unknown) => console.error("memory sync:", error)));
  if (runners.names.length !== 0) sessions.start();
  harness.resume();

  let lock = Promise.resolve();
  return {
    harness,
    root,
    submit: async (target, { text, content, requestId, origin }, context) => {
      const input = { content, text, ...(requestId !== undefined && { requestId }), ...(origin && { origin }) };
      if (target.id !== root.id) return chats.submit(harness, target.id, input, context);
      // One at a time, so no input starts a turn between the idle check and the reset.
      const turn = lock.then(async () => {
        const live = await harness.snapshot(LiveDoc, root.id, context);
        if (live?.run === undefined) await root.reset(undefined, context);
        return root.submit(
          {
            type: "input",
            content,
            whenBusy: "steer",
            ...(requestId !== undefined && { requestId }),
            ...(origin && { entry: { kind: "viz.input", data: { origin, text } } }),
          },
          context,
        );
      });
      lock = turn.then(
        () => {},
        () => {},
      );
      return turn;
    },
    createChat: (context) => chats.create(harness, context),
    close: async (context) => {
      await watch.stop();
      memory.stop();
      await harness.close(context);
      sessions.close();
      memory.close();
    },
  };
}
