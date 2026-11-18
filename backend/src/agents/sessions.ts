import type { McpServer as McpDeclaration, SessionUpdate } from "@agentclientprotocol/sdk";
import { awaitWithContext, type Context, withAbortSignal, withoutAbortSignal } from "@earendil-works/chord/context";
import type { ConversationId, Tx } from "@earendil-works/pi-durable";
import { obj, str } from "@earendil-works/pi-schema";

import { type Harness as HarnessConfig, resolveSecret } from "../config.ts";
import { bridgeEnv } from "../mcp/bridge.ts";
import { McpServer, tool } from "../mcp/server.ts";
import { AcpProcess } from "./acp.ts";
import {
  type Outcome,
  PlanEntry,
  ProgressEntry,
  TextEntry,
  ThoughtEntry,
  ToolEntry,
  type ToolRecord,
} from "./model.ts";

export type TurnOutcome = Exclude<Outcome, { kind: "running" }>;

type TurnOptions = {
  id: ConversationId;
  name: string;
  config: HarnessConfig;
  cwd: string;
  sessionId: string | null;
  text: string;
  commit: (write: (tx: Tx) => Promise<void>) => Promise<void>;
  saveSession: (id: string) => Promise<void>;
  context: Context;
  progress?: (text: string) => void;
};

type Update = SessionUpdate | { sessionUpdate: "viz_progress"; text: string };
type Progress = (text: string) => Promise<void>;
type TextRun = { kind: "text" | "thought"; text: string };

type LiveSession = {
  readonly lifetime: AbortController;
  readonly token: string;
  process?: AcpProcess;
  sessionId: string | null;
  active?: { update: (update: SessionUpdate) => void; report: Progress };
};

/** One process per conversation. Callers serialize turns and persist their prompt and terminal state. */
export class AgentSessions {
  readonly #mcp: McpServer<Progress>;
  readonly #sessions = new Map<ConversationId, LiveSession>();

  constructor(version: string) {
    this.#mcp = new McpServer(
      { name: "viz", version, instructions: "Report significant progress to the viz conversation." },
      [
        tool<Progress, { text: string }>({
          name: "report_progress",
          description: "Record a short progress note in this turn's transcript.",
          input: obj({ text: str({ minLength: 1, maxLength: 4096 }) }),
          call: async ({ text }, report) => {
            await report(text);
            return "Progress recorded.";
          },
        }),
      ],
    );
  }

  start(): void {
    this.#mcp.start();
  }

  close(): void {
    for (const [id, live] of this.#sessions) this.#release(id, live);
    this.#mcp.stop();
  }

  forget(id: ConversationId): void {
    const live = this.#sessions.get(id);
    if (live !== undefined) this.#release(id, live);
  }

  #release(id: ConversationId, live: LiveSession, error?: unknown): void {
    if (this.#sessions.get(id) !== live) return;
    this.#sessions.delete(id);
    delete live.active;
    this.#mcp.revoke(live.token);
    live.lifetime.abort(error ?? new Error("ACP session closed"));
    live.process?.close();
  }

  #allocate(id: ConversationId): LiveSession {
    // The MCP declaration lasts as long as the process. Its token reaches only this session's active turn.
    const live: LiveSession = {
      lifetime: new AbortController(),
      sessionId: null,
      token: this.#mcp.grant(async (text) => {
        const active = live.active;
        if (active === undefined) throw new Error("No active ACP turn");
        await active.report(text);
      }),
    };
    this.#sessions.set(id, live);
    return live;
  }

  async #open(live: LiveSession, options: TurnOptions, context: Context): Promise<void> {
    const { id, name, config, cwd, saveSession, sessionId } = options;
    const url = this.#mcp.url;
    const signal = context.abortSignal;
    signal?.throwIfAborted();
    const env = Object.fromEntries(
      await awaitWithContext(
        Promise.all(Object.entries(config.env ?? {}).map(async ([key, value]) => [key, await resolveSecret(value)])),
        context,
      ),
    );
    const process = await AcpProcess.start(
      {
        command: [config.command, ...(config.args ?? [])],
        env,
        permissions: config.permissions ?? "reject",
        ...(signal && { signal }),
      },
      {
        onUpdate: (source, update) => {
          if (source === live.sessionId) live.active?.update(update);
        },
        onClose: (error) => {
          if (error) {
            console.error(`ACP ${name}:`, error.message);
            this.#release(id, live, error);
          }
        },
        log: (line) => console.error(`ACP ${name}: ${line}`),
      },
    );
    if (signal?.aborted) {
      process.close();
      signal.throwIfAborted();
    }
    live.process = process;
    const mcpServers = [declaration(process, url, live.token)];
    // Do not attach the turn's update sink until after setup: session/load replays historical updates.
    if (sessionId === null) {
      live.sessionId = await awaitWithContext(process.newSession(cwd, mcpServers), context);
      await saveSession(live.sessionId);
    } else {
      await awaitWithContext(process.restoreSession(sessionId, cwd, mcpServers), context);
      live.sessionId = sessionId;
    }
  }

  async turn(options: TurnOptions): Promise<TurnOutcome> {
    const { id, sessionId, text, commit, context, progress } = options;
    const transcript: { text?: TextRun } = {};
    let pending: Update[] = [];
    const answer: string[] = [];
    const tools = new Map<string, ToolRecord>();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let saved = Promise.resolve();
    let live = this.#sessions.get(id);
    const flush = (final = false) => {
      clearTimeout(timer);
      timer = undefined;
      if (pending.length === 0 && !final) return saved;
      const batch = pending;
      pending = [];
      saved = saved.then(async () => {
        if (batch.length === 0 && !transcript.text) return;
        await commit(async (tx) => {
          await apply(tx, id, batch, tools, transcript);
          if (final) await appendText(tx, id, transcript);
        });
      });
      // A failed transcript write invalidates the live turn. The final flush observes the same error.
      void saved.catch((error: unknown) => {
        if (live !== undefined) this.#release(id, live, error);
      });
      return saved;
    };
    const push = (update: Update) => {
      pending.push(update);
      timer ??= setTimeout(flush, 75);
    };

    let outcome: TurnOutcome;
    try {
      if (live === undefined || live.process?.closed !== false || live.sessionId !== sessionId) {
        if (live !== undefined) this.#release(id, live);
        live = this.#allocate(id);
      }
      const turn = withAbortSignal(live.lifetime.signal, context);
      if (live.process === undefined) await this.#open(live, options, turn);
      turn.abortSignal?.throwIfAborted();
      const { process, sessionId: session } = live;
      if (process === undefined || session === null) throw new Error("ACP session setup did not complete");
      live.active = {
        update: (update) => {
          if (update.sessionUpdate === "agent_message_chunk" && update.content.type === "text")
            answer.push(update.content.text);
          push(update);
        },
        report: async (text) => {
          push({ sessionUpdate: "viz_progress", text });
          await flush();
          progress?.(text);
        },
      };
      // ACP prompts do not have idempotency keys. Never retry one after an error.
      const stopReason = await awaitWithContext(process.prompt(session, text), turn);
      outcome = { kind: "done", answer: answer.join(""), stopReason };
    } catch (error) {
      outcome = failure(error);
      if (context.abortSignal?.aborted && live?.active && live.process && live.sessionId !== null) {
        // Give the cancellation notification a chance to reach the agent, without waiting on a blocked pipe.
        const cancel = withAbortSignal(AbortSignal.timeout(250), withoutAbortSignal(context));
        await awaitWithContext(live.process.cancel(live.sessionId), cancel).catch(() => {});
      }
    }
    if (live !== undefined) {
      delete live.active;
      if (outcome.kind === "failed" || outcome.stopReason === "cancelled") this.#release(id, live);
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
  tools: Map<string, ToolRecord>,
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
        const old = tools.get(update.toolCallId);
        const record: ToolRecord = {
          id: update.toolCallId,
          title: update.title ?? old?.title ?? update.toolCallId,
          kind: update.kind ?? old?.kind ?? null,
          status: update.status ?? old?.status ?? "pending",
        };
        if (record.status === "completed" || record.status === "failed") {
          await appendText(tx, id, transcript);
          await tx.appendEntry(ToolEntry, id, { data: record });
          tools.delete(record.id);
        } else tools.set(record.id, record);
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
