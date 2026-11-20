import { pathToFileURL } from "node:url";

import type {
  ContentBlock,
  LoadSessionResponse,
  McpServer as McpDeclaration,
  SessionUpdate,
  StopReason,
} from "@agentclientprotocol/sdk";
import {
  awaitWithContext,
  BACKGROUND_CONTEXT,
  type Context,
  withAbortSignal,
  withCancel,
} from "@earendil-works/chord/context";
import {
  type ConversationId,
  type EntryId,
  type Harness,
  ROOT_CONVERSATION_ID,
  type Tx,
} from "@earendil-works/pi-durable";

import { type StoredAttachment, withAttachmentPaths } from "../attachments.ts";
import { type Harness as HarnessConfig, resolveSecret } from "../config.ts";
import { bridgeEnv } from "../mcp/bridge.ts";
import { McpServer, type McpTool } from "../mcp/server.ts";
import type { ConfigOption } from "../protocol.ts";
import { AcpProcess } from "./acp.ts";
import { PlanEntry, ProgressEntry, SessionDoc, TextEntry, ThoughtEntry, ToolEntry } from "./model.ts";
import {
  AcpStateDoc,
  defaults,
  type Desired,
  HarnessOptionsDoc,
  normalize,
  type OptionValue,
  validValue,
} from "./options.ts";
import { ToolContent, ToolReceipts } from "./tool-record.ts";

export type TurnOutcome = { kind: "done"; answer: string; stopReason: StopReason } | { kind: "failed"; error: string };

type SessionOptions = { id: ConversationId; name: string; config: HarnessConfig; cwd: string };
type TurnOptions = SessionOptions & {
  text: string;
  attachments?: StoredAttachment[];
  promptEntry: EntryId;
  commit: (write: (tx: Tx) => Promise<void>) => Promise<void>;
  context: Context;
};

type Checkpoint = (id: ConversationId, prompt: EntryId, cwd: string, context: Context) => Promise<void>;

type Update = SessionUpdate | { sessionUpdate: "viz_progress"; text: string };
type TextRun = { kind: "text" | "thought"; text: string };

type ActiveTurn = { context: Context; report: (text: string) => Promise<void> };
export type ToolScope = {
  id: ConversationId;
  active: () => ActiveTurn;
  receipt: () => ToolReceipts["issue"] | undefined;
};

type LiveSession = {
  readonly id: ConversationId;
  readonly name: string;
  readonly cwd: string;
  readonly lifetime: AbortController;
  readonly config: HarnessConfig;
  readonly token: string;
  readonly ready: Promise<void>;
  process?: AcpProcess;
  sessionId: string | null;
  options: ConfigOption[];
  openValues?: Desired;
  configuring: Promise<void>;
  cancelled: boolean;
  active?: ActiveTurn & { update: (update: SessionUpdate) => void; receipts: ToolReceipts };
};

/** One process per conversation. Callers serialize turns and persist their prompt and terminal state. */
export class AgentSessions {
  readonly #sessions = new Map<ConversationId, LiveSession>();
  #mcp!: McpServer<ToolScope>;
  #harness!: Harness;
  #checkpoint!: Checkpoint;

  start(version: string, harness: Harness, tools: readonly McpTool<ToolScope>[], checkpoint: Checkpoint): void {
    this.#checkpoint = checkpoint;
    this.#harness = harness;
    this.#mcp = new McpServer(
      { name: "viz", version, instructions: "Manage your subagents and record progress." },
      tools,
      (scope) => scope.receipt(),
    );
    this.#mcp.start();
  }

  close(): void {
    for (const live of this.#sessions.values()) this.#release(live);
    this.#mcp?.stop();
  }

  forget(id: ConversationId): void {
    const live = this.#sessions.get(id);
    if (live) this.#release(live);
  }

  relocate(id: ConversationId, cwd: string): void {
    const live = this.#sessions.get(id);
    if (live && live.cwd !== cwd) this.#release(live);
  }

  async #cancel(live: LiveSession): Promise<void> {
    if (live.cancelled) return;
    live.cancelled = true;
    delete live.active;
    if (live.process && live.sessionId !== null) {
      const cancel = withAbortSignal(AbortSignal.timeout(250), BACKGROUND_CONTEXT);
      await awaitWithContext(live.process.cancel(live.sessionId), cancel).catch(() => {});
    }
    this.#release(live);
  }

  #release(live: LiveSession, error?: unknown): void {
    const { id } = live;
    if (this.#sessions.get(id) !== live) return;
    this.#sessions.delete(id);
    delete live.active;
    this.#mcp.revoke(live.token);
    live.lifetime.abort(error ?? new Error("ACP session closed"));
    live.process?.close();
  }

  #session(options: SessionOptions): LiveSession {
    const { id, name, cwd, config } = options;
    const existing = this.#sessions.get(id);
    if (existing && existing.name === name && existing.cwd === cwd) return existing;
    if (existing) this.#release(existing);
    const live: LiveSession = {
      id,
      name,
      cwd,
      config,
      lifetime: new AbortController(),
      sessionId: null,
      options: [],
      configuring: Promise.resolve(),
      cancelled: false,
      ready: Promise.resolve()
        .then(() => this.#open(live))
        .catch((error: unknown) => {
          this.#release(live, error);
          throw error;
        }),
      token: this.#mcp.grant({
        id,
        receipt: () => live.active?.receipts.issue,
        active: () => {
          if (!live.active || live.active.context.abortSignal?.aborted) throw new Error("No active ACP turn");
          return live.active;
        },
      }),
    };
    this.#sessions.set(id, live);
    return live;
  }

  async prepare(options: SessionOptions): Promise<void> {
    await this.#session(options).ready;
  }

  async configure(id: ConversationId, option: string, value: OptionValue, context: Context) {
    await this.#harness.commit(async (tx) => {
      const doc = await tx.doc(AcpStateDoc, id);
      const setting = doc.options.find((item) => item.id === option);
      if (!setting) throw new Error("Option not found");
      if (!validValue(setting, value)) throw new Error("Invalid option value");
      doc.desired = { ...doc.desired, [option]: value };
      setting.value = value;
    }, context);
    const live = this.#sessions.get(id);
    if (!live) return;
    const job = live.configuring.then(async () => {
      await live.ready;
      await this.#saveOptions(live, await this.#setOption(live, option, value));
    });
    live.configuring = job.catch(() => {});
    await job;
  }

  async #open(live: LiveSession): Promise<void> {
    const {
      id,
      name,
      cwd,
      config,
      lifetime: { signal },
    } = live;
    const context = withAbortSignal(signal, BACKGROUND_CONTEXT);
    const [session, saved] = await Promise.all([
      this.#harness.snapshot(SessionDoc, id, context),
      this.#harness.snapshot(AcpStateDoc, id, context),
    ]);
    const sessionId = session?.sessionId ?? null;
    const env = Object.fromEntries(
      await awaitWithContext(
        Promise.all(Object.entries(config.env ?? {}).map(async ([key, value]) => [key, await resolveSecret(value)])),
        context,
      ),
    );
    const early: [string, SessionUpdate][] = [];
    let opening = true;
    const process = await AcpProcess.start(
      { command: [config.command, ...(config.args ?? [])], cwd, env, signal },
      {
        onUpdate: (source, update) => {
          if (opening) {
            if (update.sessionUpdate === "config_option_update" || update.sessionUpdate === "available_commands_update")
              early.push([source, update]);
          } else if (source === live.sessionId && !signal.aborted) {
            void this.#onUpdate(live, update).catch((error: unknown) => this.#release(live, error));
          }
        },
        onClose: (error) => {
          if (error) {
            console.error(`ACP ${name}:`, error.message);
            this.#release(live, error);
          }
        },
        log: (line) => console.error(`ACP ${name}: ${line}`),
      },
    );
    if (signal.aborted) {
      process.close();
      signal.throwIfAborted();
    }
    live.process = process;
    const mcp = [declaration(process, this.#mcp.url, live.token)];
    // No transcript sink during restore: session/load replays history.
    let reported: boolean;
    if (sessionId === null) {
      const response = await awaitWithContext(process.newSession(cwd, mcp), context);
      live.sessionId = response.sessionId;
      await this.#harness.commit(async (tx) => {
        (await tx.doc(SessionDoc, id)).sessionId = response.sessionId;
      }, context);
      reported = this.#refresh(live, response);
    } else {
      const response = await awaitWithContext(process.restoreSession(sessionId, cwd, mcp), context);
      live.sessionId = sessionId;
      live.options = saved?.options ?? [];
      if (session?.openValues) live.openValues = session.openValues;
      reported = this.#refresh(live, response);
    }
    for (const [source, update] of early) {
      if (source === live.sessionId && (await this.#onUpdate(live, update, false))) reported = true;
    }
    opening = false;
    const { options } = live;
    if (reported) live.openValues ??= Object.fromEntries(options.map(({ id, value }) => [id, value]));
    const desired = await this.#harness.commit(async (tx) => {
      const doc = await tx.doc(AcpStateDoc, id);
      doc.images = process.supportsImages;
      doc.desired = {
        ...(sessionId === null && defaults(config, options)),
        ...doc.desired,
      };
      if (reported) {
        if (live.openValues && !session?.openValues) (await tx.doc(SessionDoc, id)).openValues = live.openValues;
        await this.#saveHarnessOptions(tx, live, options);
      }
      return { ...doc.desired };
    }, context);
    // Model changes can change or reset the runner's available effort choices.
    const model = options.find((option) => option.category === "model")?.id ?? "model";
    let configured = false;
    const apply = async (id: string, value: OptionValue) => {
      if (await awaitWithContext(this.#setOption(live, id, value), context)) configured = true;
    };
    if (Object.hasOwn(desired, model)) await apply(model, desired[model]!);
    for (const [option, value] of Object.entries(desired)) if (option !== model) await apply(option, value);
    await this.#saveOptions(live, configured);
  }

  #refresh(live: LiveSession, { configOptions }: LoadSessionResponse) {
    if (configOptions == null) return false;
    live.options = normalize(configOptions);
    return true;
  }

  async #saveHarnessOptions(tx: Tx, live: LiveSession, options: ConfigOption[]) {
    const configured = defaults(live.config, options);
    const opened = live.openValues;
    const catalog = options.map((option) => {
      const value = Object.hasOwn(configured, option.id)
        ? configured[option.id]
        : opened && Object.hasOwn(opened, option.id)
          ? opened[option.id]
          : undefined;
      return value !== undefined && option.value !== value && validValue(option, value) ? { ...option, value } : option;
    });
    const { harnesses } = await tx.doc(HarnessOptionsDoc, ROOT_CONVERSATION_ID);
    const previous = harnesses.find(({ name }) => name === live.name);
    if (previous === undefined) harnesses.push({ name: live.name, options: catalog });
    else if (JSON.stringify(previous.options) !== JSON.stringify(catalog)) previous.options = catalog;
  }

  async #saveOptions(live: LiveSession, reported: boolean): Promise<void> {
    const context = withAbortSignal(live.lifetime.signal, BACKGROUND_CONTEXT);
    const { options } = live;
    await this.#harness.commit(async (tx) => {
      const doc = await tx.doc(AcpStateDoc, live.id);
      if (JSON.stringify(doc.options) !== JSON.stringify(options)) doc.options = options;
      if (reported) await this.#saveHarnessOptions(tx, live, options);
    }, context);
  }

  async #onUpdate(live: LiveSession, update: SessionUpdate, save = true) {
    switch (update.sessionUpdate) {
      case "config_option_update": {
        const reported = this.#refresh(live, update);
        if (save) await this.#saveOptions(live, reported);
        return reported;
      }
      case "available_commands_update":
        await this.#harness.commit(
          async (tx) => {
            const commands = update.availableCommands.map(({ name, description, input }) => ({
              name,
              description,
              hint: input?.hint ?? null,
            }));
            const doc = await tx.doc(AcpStateDoc, live.id);
            if (JSON.stringify(doc.commands) !== JSON.stringify(commands)) doc.commands = commands;
          },
          withAbortSignal(live.lifetime.signal, BACKGROUND_CONTEXT),
        );
        break;
      default:
        live.active?.update(update);
    }
    return false;
  }

  async #setOption(live: LiveSession, id: string, value: OptionValue) {
    if (!live.process || live.sessionId === null) throw new Error("ACP session is closed");
    live.lifetime.signal.throwIfAborted();
    const option = live.options.find((item) => item.id === id);
    if (!option || option.value === value || !validValue(option, value)) return false;
    const response = await live.process.setOption(live.sessionId, id, value);
    return this.#refresh(live, response);
  }

  async turn(options: TurnOptions): Promise<TurnOutcome> {
    const { id, text, commit, context } = options;
    const transcript: { text?: TextRun } = {};
    let pending: Update[] = [];
    const answer: string[] = [];
    const tools = new Map<string, ToolContent>();
    const receipts = new ToolReceipts();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let saved = Promise.resolve();
    let live: LiveSession | undefined;
    const flush = (final = false) => {
      clearTimeout(timer);
      timer = undefined;
      if (pending.length === 0 && !final) return saved;
      const batch = pending;
      pending = [];
      saved = saved.then(async () => {
        if (batch.length === 0 && !transcript.text && !(final && tools.size)) return;
        await commit(async (tx) => {
          await apply(tx, id, batch, tools, receipts, transcript);
          if (final) {
            await appendText(tx, id, transcript);
            for (const { record } of tools.values()) await tx.appendEntry(ToolEntry, id, { data: record });
          }
        });
      });
      // A failed transcript write invalidates the live turn. The final flush observes the same error.
      void saved.catch((error: unknown) => {
        if (live !== undefined) this.#release(live, error);
      });
      return saved;
    };
    const push = (update: Update) => {
      pending.push(update);
      timer ??= setTimeout(flush, 75);
    };

    let outcome: TurnOutcome;
    const toolTurn = withCancel(context);
    try {
      context.abortSignal?.throwIfAborted();
      live = this.#session(options);
      const turn = withAbortSignal(live.lifetime.signal, context);
      await awaitWithContext(live.ready, turn);
      await awaitWithContext(live.configuring, turn);
      turn.abortSignal?.throwIfAborted();
      const { process, sessionId: session } = live;
      if (process === undefined || session === null) throw new Error("ACP session setup did not complete");
      const attachments = options.attachments ?? [];
      const prompt: ContentBlock[] = [{ type: "text", text: withAttachmentPaths(text, attachments) }];
      for (const { name, mime, path } of attachments) {
        const uri = pathToFileURL(path).href;
        if (process.supportsImages && mime.startsWith("image/")) {
          const data = Buffer.from(await Bun.file(path).arrayBuffer()).toString("base64");
          prompt.push({ type: "image", data, mimeType: mime });
        }
        prompt.push({ type: "resource_link", uri, name, mimeType: mime });
      }
      await this.#checkpoint(id, options.promptEntry, options.cwd, turn);
      turn.abortSignal?.throwIfAborted();
      live.active = {
        context: withAbortSignal(live.lifetime.signal, toolTurn.context),
        receipts,
        update: (update) => {
          if (update.sessionUpdate === "agent_message_chunk" && update.content.type === "text")
            answer.push(update.content.text);
          push(update);
        },
        report: async (text) => {
          push({ sessionUpdate: "viz_progress", text });
          await flush();
        },
      };
      // ACP prompts do not have idempotency keys. Never retry one after an error.
      const stopReason = await awaitWithContext(process.prompt(session, prompt), turn);
      outcome = { kind: "done", answer: answer.join(""), stopReason };
    } catch (error) {
      if (context.abortSignal?.aborted && live) await this.#cancel(live);
      outcome =
        live?.cancelled || context.abortSignal?.aborted
          ? { kind: "done", answer: answer.join(""), stopReason: "cancelled" }
          : failure(error);
    }
    toolTurn.cancel();
    if (live !== undefined) {
      delete live.active;
      if (outcome.kind === "failed" || outcome.stopReason === "cancelled") this.#release(live);
    }
    try {
      await flush(true);
    } catch (error) {
      return failure(error);
    }
    return outcome;
  }
}

function failure(error: unknown): TurnOutcome {
  return { kind: "failed", error: error instanceof Error ? error.message : String(error) };
}

function declaration(process: AcpProcess, url: string, token: string): McpDeclaration {
  if (process.supportsHttpMcp) {
    return { name: "viz", type: "http", url, headers: [{ name: "Authorization", value: `Bearer ${token}` }] };
  }
  // Bun exposes the source entry point as argv[1] in development, but not in a standalone executable.
  const entry = Bun.main.startsWith("/$bunfs/") ? [] : [Bun.main];
  return {
    name: "viz",
    command: globalThis.process.execPath,
    args: [...entry, "mcp-bridge"],
    env: Object.entries(bridgeEnv(url, token)).map(([name, value]) => ({ name, value })),
  };
}

async function appendText(tx: Tx, id: ConversationId, transcript: { text?: TextRun }) {
  const run = transcript.text;
  if (!run) return;
  await tx.appendEntry(run.kind === "text" ? TextEntry : ThoughtEntry, id, { data: { text: run.text } });
  delete transcript.text;
}

async function apply(
  tx: Tx,
  id: ConversationId,
  updates: Update[],
  tools: Map<string, ToolContent>,
  receipts: ToolReceipts,
  transcript: { text?: TextRun },
) {
  for (const update of updates) {
    switch (update.sessionUpdate) {
      case "agent_message_chunk":
      case "agent_thought_chunk": {
        if (update.content.type !== "text") break;
        const kind = update.sessionUpdate === "agent_message_chunk" ? "text" : "thought";
        if (transcript.text?.kind === kind) transcript.text.text += update.content.text;
        else {
          await appendText(tx, id, transcript);
          transcript.text = { kind, text: update.content.text };
        }
        if (transcript.text.text.length >= 4096) await appendText(tx, id, transcript);
        break;
      }
      case "tool_call":
      case "tool_call_update": {
        const tool = tools.get(update.toolCallId) ?? new ToolContent(update.toolCallId);
        const record = tool.update(update, receipts);
        if (record.status === "completed" || record.status === "failed") {
          await appendText(tx, id, transcript);
          await tx.appendEntry(ToolEntry, id, { data: record });
          tools.delete(record.id);
        } else tools.set(record.id, tool);
        break;
      }
      case "plan":
      case "viz_progress": {
        await appendText(tx, id, transcript);
        if (update.sessionUpdate === "plan") {
          await tx.appendEntry(PlanEntry, id, {
            data: { entries: update.entries.map(({ content, status }) => ({ content, status })) },
          });
        } else await tx.appendEntry(ProgressEntry, id, { data: { text: update.text } });
        break;
      }
    }
  }
}
