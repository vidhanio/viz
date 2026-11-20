/** Opens viz's pi-durable harness over SQLite in the state directory, with the orchestrator's memory beside it. */
import { mkdir } from "node:fs/promises";
import { homedir } from "node:os";
import { join, resolve } from "node:path";

import type { Context } from "@earendil-works/chord/context";
import type { WatchHandle } from "@earendil-works/chord/watch";
import { InMemoryCredentialStore, type UserMessage } from "@earendil-works/pi-ai";
import type { MutableModels } from "@earendil-works/pi-ai/models";
import { builtinModels } from "@earendil-works/pi-ai/providers/all";
import {
  type Conversation,
  type ConversationId,
  type ConversationView,
  createRegistry,
  Harness,
  InboxDoc,
  ROOT_CONVERSATION_ID,
  LiveDoc,
  type Submission,
  type SubmissionId,
} from "@earendil-works/pi-durable";
import { NodeExecutionEnv } from "@earendil-works/pi-durable/env/node";
import { openNodeSqliteStorage } from "@earendil-works/pi-durable/storage/sqlite/node";
import { CodingTools } from "@earendil-works/pi-durable/tools";

import { ChatDoc, type ChatInput, chatScheduler, withdrawChatInputs } from "./agents/chat.ts";
import { modelOption, type SpawnOptionsQuery, thinkingOption } from "./agents/options.ts";
import { Runners } from "./agents/runners.ts";
import { AgentSessions } from "./agents/sessions.ts";
import { agentTools } from "./agents/tools.ts";
import { saveAttachments, type StoredAttachment, withAttachmentPaths } from "./attachments.ts";
import { type ChangeFeed, observeChanges } from "./changes.ts";
import { type LoadedConfig, resolveSecret, stateDir } from "./config.ts";
import { Conversations, descendants, moveConversation } from "./conversations.ts";
import { FileSearch } from "./files.ts";
import { type MemoryBinding, memoryExtension } from "./memory/extension.ts";
import { type Compactor, Memory } from "./memory/memory.ts";
import { MEMORY_PROMPT } from "./memory/prompt.ts";
import { Sync } from "./memory/sync.ts";
import { modelRef, Orchestrator, orchestratorSettings, orchestratorThinking, SettingsDoc } from "./orchestrator.ts";
import { orchestratorToolkit, type ToolkitHost } from "./orchestrator-tools.ts";
import { isDeleted, MetaDoc, projectIn, projects, type ProjectStore, touch } from "./projects.ts";
import type { Attachment, Params, Result } from "./protocol.ts";
import { SearchIndex } from "./search.ts";
import { ORCHESTRATOR_TITLE, TitleDoc } from "./titles.ts";

export type VizHarness = {
  readonly harness: Harness;
  /** The orchestrator: pi-durable's reserved root conversation. */
  readonly root: Conversation;
  readonly changes: ChangeFeed;
  readonly conversations: Conversations;
  readonly projects: ProjectStore;
  readonly search: SearchIndex;
  saveAttachments(attachments: Attachment[]): Promise<StoredAttachment[]>;
  removeQueued(id: ConversationId, submissionId: SubmissionId, context: Context): Promise<void>;
  /**
   * Submits an input. To the orchestrator, as uniichat turns go: an input while it works joins the running turn
   * (steered in between tool calls); one while it is idle starts a new turn from a fresh context, whose past reaches
   * the model only through the memory's view. Other conversations queue inputs as follow-ups.
   */
  submit(target: Conversation, input: ChatInput, context: Context): Promise<Submission>;
  createChat(options: Params<"conversation.create">, context: Context): Promise<Conversation>;
  listHarnesses(context: Context): Promise<Result<"harnesses.list">>;
  harnessOptions(harness: string, model: string, context: Context): Promise<Result<"harnesses.options">>;
  updateConversation(
    id: ConversationId,
    options: Omit<Params<"conversation.update">, "conversation">,
    context: Context,
  ): Promise<void>;
  configureConversation(
    id: ConversationId,
    option: string,
    value: Params<"conversation.configure">["value"],
    context: Context,
  ): Promise<void>;
  abortConversation(id: ConversationId, context: Context): Promise<void>;
  /**
   * Deletes a conversation and its descendants: stops their turns, releases their runners, and hides them everywhere.
   * Returns the deleted IDs.
   */
  deleteConversation(id: ConversationId, context: Context): Promise<ConversationId[]>;
  removeProject(id: string, context: Context): Promise<void>;
  searchFiles(id: ConversationId, query: string, limit: number | undefined, context: Context): Promise<string[]>;
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
  const cwd = resolve(orchestrator.cwd ?? homedir());
  const sessions = new AgentSessions();
  const chats = chatScheduler(runners, sessions, cwd);
  let toolkit: ToolkitHost | undefined;
  const Toolkit = orchestratorToolkit(() => {
    if (!toolkit) throw new Error("viz is still starting");
    return toolkit;
  });

  const registry = createRegistry();
  registry.install(CodingTools);
  registry.install(Memorize);
  registry.install(Orchestrator);
  registry.install(Toolkit);
  registry.install(chats.extension);

  const harness: Harness = await Harness.open(
    await openNodeSqliteStorage(join(dir, "viz.sqlite")),
    {
      models,
      registry,
      env: (target) =>
        target.conversationId === ROOT_CONVERSATION_ID ? new NodeExecutionEnv({ cwd: target.cwd ?? cwd }) : undefined,
    },
    context,
  );
  const root = await harness.root(context, { init: touch });
  const overrides = await harness.snapshot(SettingsDoc, root.id, context);
  const selectedModel = overrides?.model ?? orchestrator.model;
  // The selected options survive restart; config supplies defaults and host settings.
  await root.configure(
    {
      model: modelRef(selectedModel),
      thinkingLevel: orchestratorThinking(models, selectedModel, overrides?.thinking ?? orchestrator.thinking),
      instructions: orchestrator.instructions ?? null,
      cwd,
      extensions: [CodingTools, Memorize, Orchestrator, Toolkit],
    },
    context,
  );
  await harness.commit(async (tx) => {
    const doc = await tx.doc(TitleDoc, root.id);
    if (doc.title !== ORCHESTRATOR_TITLE) doc.title = ORCHESTRATOR_TITLE;
  }, context);

  const sync = new Sync(memory, root, context);
  binding.sync = sync;
  binding.root = root.id;
  await sync.run();
  // Every commit to the orchestrator feeds the memory as it comes, so its summaries build ahead of the next turn.
  const watch: WatchHandle<ConversationView> = await root.watch(context);
  watch.start(async () => void sync.run().catch((error: unknown) => console.error("memory sync:", error)));
  const settings = orchestratorSettings(harness, root.id, models, orchestrator);
  const rootImages = async (context: Context) => {
    const { model } = await root.agent(context);
    return model !== undefined && models.getModel(model.provider, model.modelId)?.input.includes("image") === true;
  };
  const conversations = new Conversations(harness, root.id, cwd, settings.options, rootImages);
  const projectStore = projects(harness, root.id);
  const files = new FileSearch();
  const changes = await observeChanges(harness, root.id, context);
  const search = await SearchIndex.open(dir, harness, context);

  const abortConversation = async (id: ConversationId, context: Context, deleting = false) => {
    const target = await harness.conversation(id, context);
    if (!target) throw new Error("Conversation not found");
    await target.abort(context, { background: deleting });
  };
  const deleteConversation: VizHarness["deleteConversation"] = async (id, context) => {
    if (id === root.id) throw new Error("The orchestrator can't be deleted");
    // This commit is the cutoff: afterwards no input, subagent, tool, or RPC reaches these conversations.
    const ids = await harness.commit(async (tx) => {
      if (!(await tx.conversation(id)) || (await tx.doc(MetaDoc, id)).deleted) {
        throw new Error("Conversation not found");
      }
      const ids = [id];
      for await (const { record } of descendants(tx, id)) {
        if (!(await tx.doc(MetaDoc, record.id)).deleted) ids.push(record.id);
      }
      for (const target of ids) {
        const meta = await tx.doc(MetaDoc, target);
        meta.deleted = true;
        meta.updatedAt = Date.now();
        await withdrawChatInputs(tx, target);
      }
      return ids;
    }, context);
    // Children first: a parent's turn may be waiting on one of them. They are deleted already, so a failed stop is
    // only logged; releasing the session still closes the runner.
    for (const target of [...ids].reverse()) {
      await abortConversation(target, context, true).catch((error: unknown) =>
        console.error(`Stopping deleted conversation ${target}:`, error instanceof Error ? error.message : error),
      );
      sessions.forget(target);
    }
    return ids;
  };
  const listSpawnOptions = async (
    { harness: name, model, search }: SpawnOptionsQuery,
    context: Context,
    parent?: ConversationId,
  ) => {
    const parentRunner = parent === undefined ? undefined : (await harness.snapshot(ChatDoc, parent, context))?.runner;
    const available = runners.list();
    const defaultHarness = parentRunner?.name ?? available.find((runner) => runner.default)?.name ?? null;
    if (name === undefined && model === undefined && search === undefined) {
      return JSON.stringify({ defaultHarness, harnesses: available.map(({ name }) => name) });
    }
    const runner = runners.require(name ?? parentRunner?.name);
    const options = await sessions.catalog(runner.name, runner.config, parentRunner?.cwd ?? cwd, context, model);
    const models = modelOption(options);
    const thinking = thinkingOption(options);
    const selected = {
      harness: runner.name,
      model: models?.value ?? null,
      thinking: thinking
        ? { value: thinking.value, choices: thinking.choices.map(({ value, name }) => ({ value, name })) }
        : null,
    };
    if (model !== undefined) return JSON.stringify(selected);
    // Keep the tool result small even on runners with hundreds of models.
    const query = search?.trim().toLowerCase();
    const matching =
      models?.choices.filter(
        ({ value, name, group }) => !query || `${value} ${name} ${group ?? ""}`.toLowerCase().includes(query),
      ) ?? [];
    const limit = 24;
    return JSON.stringify({
      ...selected,
      models: matching.slice(0, limit).map(({ value, name, group }) => ({ value, name, group })),
      omittedModels: Math.max(0, matching.length - limit),
    });
  };
  sessions.start(
    options.version,
    harness,
    agentTools({
      harness,
      conversations,
      spawn: (options, context) => chats.create(harness, options, context),
      listSpawnOptions,
      submit: (id, input, context) => chats.submit(harness, id, input, context),
      abort: abortConversation,
    }),
  );
  let lock = Promise.resolve();
  const submit: VizHarness["submit"] = async (target, input, context) => {
    const { text, content, requestId, origin, attachments = [] } = input;
    if (target.id !== root.id) return chats.submit(harness, target.id, input, context);
    // One at a time, so no input starts a turn between the idle check and the reset.
    const turn = lock.then(async () => {
      if (requestId !== undefined) {
        const existing = await harness.commit((tx) => tx.submissionByRequest(root.id, requestId), context);
        if (existing) return (await harness.submission(existing.id, context))!;
      }
      const live = await harness.snapshot(LiveDoc, root.id, context);
      if (live?.run === undefined) await root.reset(undefined, context);
      let prompt: UserMessage["content"] = withAttachmentPaths(content, attachments);
      if (attachments.some(({ mime }) => mime.startsWith("image/")) && (await rootImages(context))) {
        const blocks: Exclude<UserMessage["content"], string> = [{ type: "text", text: prompt }];
        for (const { mime, path } of attachments)
          if (mime.startsWith("image/"))
            blocks.push({
              type: "image",
              mimeType: mime,
              data: Buffer.from(await Bun.file(path).arrayBuffer()).toString("base64"),
            });
        prompt = blocks;
      }
      const submission = await root.submit(
        {
          type: "input",
          content: prompt,
          whenBusy: "steer",
          ...(requestId !== undefined && { requestId }),
          ...((origin || attachments.length) && {
            entry: {
              kind: "viz.input",
              data: { text, ...(origin && { origin }), ...(attachments.length && { attachments }) },
            },
          }),
        },
        context,
      );
      await harness.commit((tx) => touch(tx, root.id), context);
      return submission;
    });
    lock = turn.then(
      () => {},
      () => {},
    );
    return turn;
  };
  toolkit = {
    harness,
    root: root.id,
    conversations,
    projects: projectStore,
    search,
    runners,
    chats,
    listSpawnOptions,
    report: async (text, requestId, context) => void (await submit(root, { text, content: text, requestId }, context)),
    // A deleted conversation is not found; `abortConversation` itself also serves the deletion.
    abort: async (id, context) => {
      if (await isDeleted(harness, id, context)) throw new Error("Conversation not found");
      await abortConversation(id, context);
    },
  };
  // A crash between tombstoning a chat and stopping it must not replay its tasks.
  const { tasks } = await harness.inspect(context);
  for (const { record } of tasks) {
    if (record.conversationId !== root.id && (await isDeleted(harness, record.conversationId, context)))
      await harness.abortTask(record.id, context);
  }
  harness.resume();

  return {
    harness,
    root,
    changes,
    conversations,
    projects: projectStore,
    search,
    listHarnesses: async (context) => ({
      harnesses: await Promise.all(
        runners.list().map(async (runner) => ({
          ...runner,
          options: await sessions
            .catalog(runner.name, runners.require(runner.name).config, cwd, context)
            .catch((error: unknown) => {
              if (context.abortSignal?.aborted) context.abortSignal.throwIfAborted();
              console.error(`ACP ${runner.name} catalog:`, error instanceof Error ? error.message : error);
              return [];
            }),
        })),
      ),
    }),
    harnessOptions: async (name, model, context) => ({
      options: await sessions.catalog(name, runners.require(name).config, cwd, context, model),
    }),
    saveAttachments: (attachments) => saveAttachments(dir, attachments),
    removeQueued: (id, submission, context) =>
      harness.commit(async (tx) => {
        const record = await tx.submission(submission);
        if (record?.conversationId !== id || record.type !== "input") throw new Error("Queued input not found");
        if (record.status !== "queued") throw new Error("Input is no longer queued");
        if (id === root.id) {
          const inbox = await tx.doc(InboxDoc, id);
          const index = inbox.items.findIndex((item) => item.id === submission);
          if (index === -1) throw new Error("Input is no longer queued");
          inbox.items.splice(index, 1);
        } else {
          const chat = await tx.doc(ChatDoc, id);
          const index = chat.queue.findIndex((item) => item.submission === submission);
          if (index === -1) throw new Error("Input is no longer queued");
          chat.queue.splice(index, 1);
        }
        tx.settleSubmission(submission, { status: "unanswered", reason: "removed" });
      }, context),
    abortConversation,
    deleteConversation,
    updateConversation: async (id, options, context) => {
      const { title, archived, project, pinned, unread } = options;
      if (
        id === root.id &&
        (title !== undefined || archived !== undefined || project !== undefined || pinned !== undefined)
      )
        throw new Error("The orchestrator can't be changed");
      if (unread !== undefined) changes.clearPendingUnread(id);
      const moved = await harness.commit(async (tx) => {
        if (title !== undefined) {
          // A rename outranks the runner's titles; clearing it lets the runner title the chat again.
          const doc = await tx.doc(TitleDoc, id);
          doc.title = title;
          if (title === "") delete doc.source;
          else doc.source = "user";
        }
        if (archived !== undefined || pinned !== undefined || unread !== undefined) {
          const meta = await tx.doc(MetaDoc, id);
          if (archived !== undefined) meta.archived = archived;
          if (pinned !== undefined) meta.pinned = pinned;
          if (unread !== undefined) meta.unread = unread;
        }
        if (project === undefined) return null;
        const directory = project === null ? cwd : (await projectIn(tx, root.id, project)).path;
        const ids = new Set([id]);
        for await (const { record } of descendants(tx, id)) ids.add(record.id);
        const relocated: ConversationId[] = [];
        for (const target of ids) if (await moveConversation(tx, target, project, directory)) relocated.push(target);
        return { ids: relocated, directory };
      }, context);
      if (moved) for (const target of moved.ids) sessions.relocate(target, moved.directory);
    },
    configureConversation: async (id, option, value, context) => {
      if (id === root.id) return settings.configure(option, value, context);
      const chat = await harness.snapshot(ChatDoc, id, context);
      if (!chat?.runner) throw new Error("ACP chat has no runner");
      const { name, cwd } = chat.runner;
      await sessions.configure({ id, name, cwd, config: runners.require(name).config }, option, value, context);
    },
    removeProject: async (id, context) => {
      const moved: ConversationId[] = [];
      await projectStore.remove(
        id,
        async (tx, target) => {
          if (await moveConversation(tx, target, null, cwd)) moved.push(target);
        },
        context,
      );
      for (const target of moved) sessions.relocate(target, cwd);
    },
    searchFiles: async (id, query, limit, context) =>
      files.search(await conversations.cwdFor(id, context), query, limit),
    submit,
    createChat: (options, context) => {
      runners.require(options.harness);
      return chats.create(harness, options, context);
    },
    close: async (context) => {
      await watch.stop();
      memory.stop();
      await changes.stop();
      await harness.close(context);
      search.close();
      sessions.close();
      memory.close();
    },
  };
}
