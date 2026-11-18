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
  type SubmissionId,
} from "@earendil-works/pi-durable";
import type { AssistantMessage } from "@earendil-works/pi-ai";
import type { Server, ServerWebSocket } from "bun";

import type { VizHarness } from "./harness.ts";
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
import { TitleDoc } from "./titles.ts";

export type ServeOptions = {
  readonly socket: string;
  /** `host:port`; requires `token`. */
  readonly listen?: string;
  readonly token?: string;
  readonly version: string;
};

type Client = Params<"hello">["client"];
type SocketData = { readonly remote: boolean; client?: Client; readonly watches: Map<number, ConversationWatch> };
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
  /** Submissions whose reply is routed already: an idempotent retry returns the same submission. */
  const routed = new Set<SubmissionId>();

  const notify = <N extends NotificationName>(ws: Socket, method: N, params: Notification<N>) =>
    ws.send(JSON.stringify({ jsonrpc: "2.0", method, params }));

  const conversation = async (id: number | undefined) => {
    if (id === undefined) return root;
    const found = await harness.conversation(id as ConversationId, context);
    if (!found) throw new RpcError(ErrorCode.NotFound, `no conversation ${id}`);
    return found;
  };

  /** Waits for a connector input's answer and delivers it to that connector. */
  const routeReply = async (conversationId: ConversationId, submission: SubmissionId, origin: Origin) => {
    const handle = await harness.submission(submission, context);
    const settled = await handle!.wait(context);
    let text: string | null = null;
    if (settled.status === "done" && settled.type === "input") {
      const entry = await harness.commit((tx) => tx.entry(AssistantEntry, settled.answer), context);
      const message = entry?.model?.[0] as AssistantMessage | undefined;
      text = message?.content.flatMap((c) => (c.type === "text" ? [c.text] : [])).join("") ?? null;
    }
    // TODO(durable-outbox): replies to disconnected connectors are dropped; see TODO.md.
    for (const ws of connectors.get(origin.connector) ?? []) {
      notify(ws, "reply", { origin, conversation: conversationId, submission, status: settled.status, text });
    }
  };

  const handlers: { [M in MethodName]: (ws: Socket, params: Params<M>) => Promise<Result<M>> } = {
    hello: async (ws, { client }) => {
      ws.data.client = client;
      if (client.kind === "connector") {
        const sockets = connectors.get(client.name) ?? new Set();
        connectors.set(client.name, sockets.add(ws));
      }
      return { protocol: PROTOCOL_VERSION, version: options.version, root: root.id };
    },
    "conversations.list": async () => {
      const page = await harness.commit((tx) => tx.scanConversations({}, 1000), context);
      const conversations = await Promise.all(
        page.items.map(async (record) => ({
          id: record.id,
          title: (await harness.snapshot(TitleDoc, record.id, context))?.title ?? "",
          kind:
            record.id === root.id
              ? ("orchestrator" as const)
              : record.owner
                ? ("subagent" as const)
                : ("chat" as const),
          ...(record.owner && { parent: record.owner.conversationId }),
        })),
      );
      return { conversations };
    },
    "conversation.create": async () => {
      const created = await viz.createChat(context);
      return { conversation: created.id };
    },
    "conversation.submit": async (_ws, { conversation: id, text, requestId, origin }) => {
      const target = await conversation(id);
      const content = origin
        ? `[${[origin.connector, `#${origin.channel}`, origin.author && `@${origin.author}`].filter(Boolean).join(" ")}] ${text}`
        : text;
      const submission = await viz.submit(
        target,
        {
          type: "input",
          content,
          ...(requestId && { requestId }),
          // `text` is what the user wrote, for UIs; the model gets it behind the origin tag.
          ...(origin && { entry: { kind: "viz.input", data: { origin, text } } }),
        },
        context,
      );
      if (origin && !routed.has(submission.id)) {
        routed.add(submission.id);
        void routeReply(target.id, submission.id, origin).catch((e) => console.error("reply routing", e));
      }
      return { submission: submission.id };
    },
    "conversation.watch": async (ws, { conversation: id }) => {
      if (ws.data.watches.has(id)) return { watching: true };
      const watch = await (await conversation(id)).watch(context);
      ws.data.watches.set(id, watch);
      notify(ws, "conversation.view", { conversation: id, value: watch.value });
      watch.start(async (call) => {
        if (call.kind === "value") notify(ws, "conversation.view", { conversation: id, ops: call.ops });
      });
      return { watching: true };
    },
    "conversation.unwatch": async (ws, { conversation: id }) => {
      await ws.data.watches.get(id)?.stop();
      ws.data.watches.delete(id);
      return { watching: false };
    },
  };

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
      if (server.upgrade(request, { data: { remote, watches: new Map() } })) return undefined;
      return new Response("expected a WebSocket upgrade\n", { status: 426 });
    }
    return new Response("not found\n", { status: 404 });
  };

  const websocket = {
    message: (ws: Socket, raw: string | Buffer) => void dispatch(ws, raw),
    close: (ws: Socket) => {
      const { client, watches } = ws.data;
      if (client?.kind === "connector") connectors.get(client.name)?.delete(ws);
      for (const watch of watches.values()) void watch.stop();
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

  return {
    socket: options.socket,
    ...(tcp && { url: tcp.url.origin }),
    stop: async () => {
      await Promise.all([local.stop(true), tcp?.stop(true)]);
      await rm(options.socket, { force: true });
    },
  };
}
