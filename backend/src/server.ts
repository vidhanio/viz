/**
 * The backend's listeners: a Unix socket for clients on this machine (file permissions are the auth) and an optional
 * TCP listener for remote clients (tailnet/LAN; bearer token required). Both serve the same routes:
 *
 * - `GET /v1/health`: liveness and versions; no auth on the socket, token on TCP.
 * - `GET /v1/rpc`: WebSocket upgrade, JSON-RPC 2.0, see `protocol.ts`.
 */
import { timingSafeEqual } from "node:crypto";
import { chmod, mkdir, rm } from "node:fs/promises";
import { dirname } from "node:path";

import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import {
  AssistantEntry,
  type ConversationId,
  type ConversationWatch,
  type EntryId,
  type SubmissionId,
} from "@earendil-works/pi-durable";
import type { AssistantMessage } from "@earendil-works/pi-ai";
import type { Server, ServerWebSocket } from "bun";

import { TurnEntry } from "./agents/model.ts";
import type { VizHarness } from "./harness.ts";
import { isDeleted } from "./projects.ts";
import {
  ErrorCode,
  type MethodName,
  Methods,
  type Notification,
  type NotificationName,
  type Origin,
  type Params,
  paramChecks,
  PROTOCOL_VERSION,
  type Result,
} from "./protocol.ts";

export type ServeOptions = {
  readonly socket: string;
  /** `host:port`; requires `token`. */
  readonly listen?: string;
  readonly token?: string;
  readonly version: string;
};

type Client = Params<"hello">["client"];
type Watch = { view: ConversationWatch; previous?: string; sending: boolean; dirty: boolean };
type SocketData = { client?: Client; closed?: true; readonly watches: Map<ConversationId, Watch> };
type Socket = ServerWebSocket<SocketData>;
type RpcId = string | number | null;

class RpcError extends Error {
  constructor(
    readonly code: number,
    message: string,
  ) {
    super(message);
  }
}

export type VizServer = {
  readonly socket: string;
  /** `http://host:port` of the TCP listener, when enabled. */
  readonly url?: string;
  stop(): Promise<void>;
};

export async function serve(viz: VizHarness, options: ServeOptions): Promise<VizServer> {
  if (options.listen && !options.token) throw new Error("backend.listen requires backend.token");
  const context = BACKGROUND_CONTEXT;
  const { harness, root } = viz;
  /** Connected connectors by name, for routing replies. */
  const connectors = new Map<string, Set<Socket>>();
  const clients = new Set<Socket>();
  /** Submissions whose reply is routed already: an idempotent retry returns the same submission. */
  const routed = new Set<SubmissionId>();

  const notify = <N extends NotificationName>(ws: Socket, method: N, params: Notification<N>) =>
    ws.send(JSON.stringify({ jsonrpc: "2.0", method, params }));

  const conversation = async (id: number | undefined) => {
    if (id === undefined) return root;
    const found = await harness.conversation(id as ConversationId, context);
    if (!found || (await isDeleted(harness, found.id, context)))
      throw new RpcError(ErrorCode.NotFound, `no conversation ${id}`);
    return found;
  };

  function refreshState(ws: Socket, id: ConversationId, watch: Watch) {
    watch.dirty = true;
    if (watch.sending) return;
    watch.sending = true;
    void (async () => {
      while (watch.dirty && ws.data.watches.get(id) === watch) {
        watch.dirty = false;
        const state = await viz.conversations.state(id, context);
        if (ws.data.watches.get(id) !== watch) break;
        const json = JSON.stringify(state);
        if (watch.previous !== json) {
          watch.previous = json;
          notify(ws, "conversation.state", { conversation: id, state });
        }
      }
    })()
      .catch((error: unknown) => console.error("Conversation state:", error))
      .finally(() => {
        watch.sending = false;
        if (watch.dirty && ws.data.watches.get(id) === watch) refreshState(ws, id, watch);
      });
  }

  /** Waits for a connector input's answer and delivers it to that connector. */
  const routeReply = async (conversationId: ConversationId, submission: SubmissionId, origin: Origin) => {
    const handle = await harness.submission(submission, context);
    const settled = await handle!.wait(context);
    let text: string | null = null;
    if (settled.status === "done" && settled.type === "input") {
      const answer = settled.answer;
      text = await harness.commit(async (tx) => {
        const external = await tx.entry(TurnEntry, answer);
        if (external !== undefined) return external.data.answer ?? null;
        const entry = await tx.entry(AssistantEntry, answer);
        const message = entry?.model?.[0] as AssistantMessage | undefined;
        return message?.content.flatMap((c) => (c.type === "text" ? [c.text] : [])).join("") ?? null;
      }, context);
    }
    // TODO(durable-outbox): replies to disconnected connectors are dropped; see TODO.md.
    for (const ws of connectors.get(origin.connector) ?? []) {
      notify(ws, "reply", { origin, conversation: conversationId, submission, status: settled.status, text });
    }
  };

  const handlers: { [M in MethodName]: (ws: Socket, params: Params<M>) => Promise<Result<M>> } = {
    hello: async (ws, { client }) => {
      const previous = ws.data.client;
      if (previous?.kind === "connector") connectors.get(previous.name)?.delete(ws);
      if ((previous?.kind === "app") !== (client.kind === "app")) {
        for (const id of ws.data.watches.keys()) viz.changes.watched(id, client.kind === "app");
      }
      clients.add(ws);
      ws.data.client = client;
      if (client.kind === "connector") {
        const sockets = connectors.get(client.name) ?? new Set();
        connectors.set(client.name, sockets.add(ws));
      }
      return { protocol: PROTOCOL_VERSION, version: options.version, root: root.id };
    },
    "conversations.list": async () => ({ conversations: await viz.conversations.list(context) }),
    "conversations.search": async (_ws, { query, limit }) => viz.search.search(query, limit),
    "conversation.entries": async (_ws, { conversation: id, before, limit = 50 }) => {
      const target = await conversation(id);
      if (before === 1) return { entries: [] };
      const page = await target.entries({ maxEntryId: (before - 1) as EntryId }, limit + 1, undefined, context);
      const entries = page.items.slice(0, limit).reverse();
      return { entries, ...(page.items.length > limit && { before: entries[0]!.id }) };
    },
    "queue.remove": async (_ws, { conversation: id, submission }) => {
      await viz.removeQueued((await conversation(id)).id, submission as SubmissionId, context);
      return {};
    },
    "conversation.create": async (_ws, params) => {
      const created = await viz.createChat(params, context);
      return { conversation: created.id };
    },
    "conversation.update": async (_ws, { conversation: id, ...update }) => {
      await viz.updateConversation((await conversation(id)).id, update, context);
      return {};
    },
    "conversation.configure": async (_ws, { conversation: id, option, value }) => {
      await viz.configureConversation((await conversation(id)).id, option, value, context);
      return {};
    },
    "conversation.abort": async (_ws, { conversation: id }) => {
      await viz.abortConversation((await conversation(id)).id, context);
      return {};
    },
    "conversation.delete": async (_ws, { conversation: id }) => {
      const { ids, kept } = await viz.deleteConversation((await conversation(id)).id, context);
      // Their views would never change again; apps learn of the deletion from `conversations.changed`.
      for (const ws of clients) for (const deleted of ids) await stopWatch(ws, deleted);
      return { kept };
    },
    "projects.list": async () => ({ projects: await viz.projects.list(context) }),
    "project.create": async (_ws, { path, name }) => ({ project: await viz.projects.create(path, name, context) }),
    "project.update": async (_ws, { project, ...changes }) => ({
      project: await viz.projects.update(project, changes, context),
    }),
    "project.remove": async (_ws, { project }) => {
      await viz.removeProject(project, context);
      return {};
    },
    "worktrees.list": async (_ws, { project }) => viz.listWorktrees(project, context),
    "harnesses.list": async () => viz.listHarnesses(context),
    "files.search": async (_ws, { conversation: id, query, limit }) => ({
      files: await viz.searchFiles((await conversation(id)).id, query, limit, context),
    }),
    "git.diff": async (_ws, { conversation: id, ...params }) =>
      viz.repositories.diff((await conversation(id)).id, params, context),
    "git.branches": async (_ws, target) => {
      const selected =
        "conversation" in target ? { conversation: (await conversation(target.conversation)).id } : target;
      const branches = await viz.repositories.branches(selected, context);
      if (branches === null) throw new RpcError(ErrorCode.NotARepository, "Not a Git repository");
      return branches;
    },
    "git.checkout": async (_ws, { conversation: id, branch, create }) => {
      await viz.repositories.checkout((await conversation(id)).id, branch, create, context);
      return {};
    },
    "git.commit": async (_ws, { conversation: id, message, all }) =>
      viz.repositories.commit((await conversation(id)).id, message, all, context),
    "git.push": async (_ws, { conversation: id }) => {
      await viz.repositories.push((await conversation(id)).id, context);
      return {};
    },
    "git.pr": async (_ws, { conversation: id, ...params }) =>
      viz.repositories.pr((await conversation(id)).id, params, context),
    "git.revert": async (_ws, { conversation: id, turn }) => {
      await viz.repositories.revert((await conversation(id)).id, turn, context);
      return {};
    },
    "conversation.submit": async (_ws, { conversation: id, text, requestId, origin, attachments }) => {
      const target = await conversation(id);
      const saved = attachments?.length ? await viz.saveAttachments(attachments) : undefined;
      const content = origin
        ? `[${[origin.connector, `#${origin.channel}`, origin.author && `@${origin.author}`].filter(Boolean).join(" ")}] ${text}`
        : text;
      const submission = await viz.submit(
        target,
        {
          content,
          text,
          ...(requestId !== undefined && { requestId }),
          ...(origin && { origin }),
          ...(saved && { attachments: saved }),
        },
        context,
      );
      if (origin && !routed.has(submission.id)) {
        routed.add(submission.id);
        void routeReply(target.id, submission.id, origin).catch((e) => console.error("reply routing", e));
      }
      return { submission: submission.id };
    },
    "conversation.watch": async (ws, { conversation: value }) => {
      const id = value as ConversationId;
      const target = await conversation(id);
      if (ws.data.watches.has(id)) return { watching: true };
      const view = await target.watch(context);
      if (await isDeleted(harness, id, context)) {
        await view.stop();
        throw new RpcError(ErrorCode.NotFound, `no conversation ${id}`);
      }
      if (ws.data.closed || ws.data.watches.has(id)) {
        await view.stop();
        return { watching: !ws.data.closed };
      }
      const watch = { view, sending: false, dirty: false };
      ws.data.watches.set(id, watch);
      if (ws.data.client?.kind === "app") viz.changes.watched(id, true);
      notify(ws, "conversation.view", { conversation: id, value: view.value });
      view.start(async (call) => {
        if (call.kind === "value") notify(ws, "conversation.view", { conversation: id, ops: call.ops });
      });
      refreshState(ws, id, watch);
      await viz.changes.markRead(id, context);
      return { watching: true };
    },
    "conversation.unwatch": async (ws, { conversation: value }) => {
      await stopWatch(ws, value as ConversationId);
      return { watching: false };
    },
  };

  async function stopWatch(ws: Socket, id: ConversationId) {
    const watch = ws.data.watches.get(id);
    ws.data.watches.delete(id);
    if (watch && ws.data.client?.kind === "app") viz.changes.watched(id, false);
    await watch?.view.stop();
  }

  const dispatch = async (ws: Socket, raw: string | Buffer) => {
    let id: RpcId = null;
    try {
      let message: { jsonrpc?: unknown; id?: unknown; method?: unknown; params?: unknown };
      try {
        message = JSON.parse(raw.toString());
      } catch {
        throw new RpcError(ErrorCode.ParseError, "invalid JSON");
      }
      if (typeof message.id === "string" || typeof message.id === "number") id = message.id;
      if (message.jsonrpc !== "2.0" || typeof message.method !== "string") {
        throw new RpcError(ErrorCode.InvalidRequest, "expected a JSON-RPC 2.0 request");
      }
      const method = message.method;
      if (!Object.hasOwn(Methods, method)) throw new RpcError(ErrorCode.MethodNotFound, `no method ${method}`);
      const name = method as MethodName;
      if (name !== "hello" && !ws.data.client) throw new RpcError(ErrorCode.NotHello, "call hello first");
      const params = message.params ?? {};
      const check = paramChecks[name];
      if (!check.check(params)) {
        const detail = check
          .errors(params)
          .map((e) => `${e.path || "/"}: ${e.message}`)
          .join("; ");
        throw new RpcError(ErrorCode.InvalidParams, detail);
      }
      const handler = handlers[name] as (ws: Socket, params: unknown) => Promise<unknown>;
      const result = await handler(ws, params);
      if (id !== null) ws.send(JSON.stringify({ jsonrpc: "2.0", id, result }));
    } catch (error) {
      const rpc =
        error instanceof RpcError ? error : new RpcError(ErrorCode.Internal, (error as Error).message ?? String(error));
      if (!(error instanceof RpcError)) console.error("rpc", error);
      ws.send(JSON.stringify({ jsonrpc: "2.0", id, error: { code: rpc.code, message: rpc.message } }));
    }
  };

  const expected = options.token ? Buffer.from(options.token) : undefined;
  const authorized = (request: Request) => {
    if (!expected) return false;
    const header = request.headers.get("authorization");
    const given = header?.startsWith("Bearer ") ? header.slice(7) : new URL(request.url).searchParams.get("token");
    if (!given) return false;
    const buffer = Buffer.from(given);
    return buffer.length === expected.length && timingSafeEqual(buffer, expected);
  };

  const fetch = (remote: boolean) => (request: Request, server: Server<SocketData>) => {
    if (remote && !authorized(request)) return new Response("unauthorized\n", { status: 401 });
    const { pathname } = new URL(request.url);
    if (pathname === "/v1/health") {
      return Response.json({ ok: true, version: options.version, protocol: PROTOCOL_VERSION });
    }
    if (pathname === "/v1/rpc") {
      if (server.upgrade(request, { data: { watches: new Map() } })) return undefined;
      return new Response("expected a WebSocket upgrade\n", { status: 426 });
    }
    return new Response("not found\n", { status: 404 });
  };

  const websocket = {
    // Base64 needs about 27 MiB per 20 MiB attachment; still bound a single RPC frame.
    maxPayloadLength: 128 * 1024 * 1024,
    message: (ws: Socket, raw: string | Buffer) => void dispatch(ws, raw),
    close: (ws: Socket) => {
      ws.data.closed = true;
      clients.delete(ws);
      const { client, watches } = ws.data;
      if (client?.kind === "connector") connectors.get(client.name)?.delete(ws);
      for (const [id, { view }] of watches) {
        if (client?.kind === "app") viz.changes.watched(id, false);
        void view.stop();
      }
      watches.clear();
    },
  };

  await mkdir(dirname(options.socket), { recursive: true, mode: 0o700 });
  await rm(options.socket, { force: true });
  const local = Bun.serve<SocketData>({ unix: options.socket, fetch: fetch(false), websocket });
  await chmod(options.socket, 0o600);

  let tcp: Server<SocketData> | undefined;
  if (options.listen) {
    const { hostname, port } = new URL(`http://${options.listen}`);
    tcp = Bun.serve<SocketData>({ hostname, port: Number(port), fetch: fetch(true), websocket });
  }

  const stopChanges = viz.changes.subscribe(({ list, state }) => {
    for (const ws of clients) {
      if (list && ws.data.client?.kind === "app") notify(ws, "conversations.changed", {});
      for (const id of state) {
        const watch = ws.data.watches.get(id);
        if (watch) refreshState(ws, id, watch);
      }
    }
  });

  const pollGit = setInterval(() => {
    const ids = new Set<ConversationId>();
    for (const ws of clients) for (const id of ws.data.watches.keys()) ids.add(id);
    for (const id of ids) viz.changes.refresh(id);
  }, 5_000);

  return {
    socket: options.socket,
    ...(tcp && { url: tcp.url.origin }),
    stop: async () => {
      stopChanges();
      clearInterval(pollGit);
      await Promise.all([local.stop(true), tcp?.stop(true)]);
      await rm(options.socket, { force: true });
    },
  };
}
