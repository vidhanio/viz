/**
 * Client ⇄ backend protocol, version 1: JSON-RPC 2.0 over a WebSocket at `/v1/rpc`.
 *
 * This file is the single source of truth. `bun run schema` writes `protocol/schema.json` (JSON Schema) for the Rust
 * (`crates/viz-client`) and Swift (`ios/`) clients.
 *
 * Requests (client → server): `hello` first, then any method in `Methods`. Notifications (server → client):
 * `Notifications`. A connector sees only `reply` notifications for inputs it submitted with its own `origin.connector`;
 * apps (`client.kind = "app"`) watch whole conversations instead.
 */
import {
  bool,
  compile,
  int,
  lit,
  nullable,
  obj,
  oneOf,
  opt,
  type Static,
  str,
  unknown,
  type Validator,
} from "@earendil-works/pi-schema";

export const PROTOCOL_VERSION = 1;

/** Where an input came from, so its reply can be routed back. Stored with the transcript entry. */
export const Origin = obj({
  connector: str({ description: "Connector name, matching its `hello` name, e.g. discord" }),
  channel: str({ description: "Connector-specific channel/chat id the reply goes to" }),
  thread: opt(str()),
  message: opt(str({ description: "Connector-specific id of the message being answered" })),
  author: opt(str({ description: "Human-readable author, shown to the model" })),
});
export type Origin = Static<typeof Origin>;

/** pi-durable IDs are positive integers. */
const ConversationId = int({ minimum: 1, description: "pi-durable ConversationId" });
const SubmissionId = int({ minimum: 1, description: "pi-durable SubmissionId" });

export const Methods = {
  hello: {
    params: obj({
      protocol: lit(PROTOCOL_VERSION),
      client: obj({ kind: oneOf(["app", "connector", "cli"]), name: str() }),
    }),
    result: obj({ protocol: lit(PROTOCOL_VERSION), version: str(), root: ConversationId }),
  },
  "conversations.list": {
    params: obj({}),
    result: obj({
      conversations: unknown({ description: "ConversationRecord[] from pi-durable: id, parent?, owner?" }),
    }),
  },
  "conversation.create": {
    params: obj({}),
    result: obj({ conversation: ConversationId }),
  },
  "conversation.submit": {
    params: obj({
      conversation: opt(int({ minimum: 1, description: "Default: the root (orchestrator) conversation" })),
      text: str(),
      requestId: opt(str({ description: "Idempotency key: a retry with the same id returns the same submission" })),
      origin: opt(Origin),
    }),
    result: obj({ submission: SubmissionId }),
  },
  "conversation.watch": {
    params: obj({ conversation: ConversationId }),
    result: obj({ watching: bool() }),
  },
  "conversation.unwatch": {
    params: obj({ conversation: ConversationId }),
    result: obj({ watching: bool() }),
  },
} as const;
export type MethodName = keyof typeof Methods;
export type Params<M extends MethodName> = Static<(typeof Methods)[M]["params"]>;
export type Result<M extends MethodName> = Static<(typeof Methods)[M]["result"]>;

export const Notifications = {
  /** A conversation view revision: the full `value` first, then Chord delta `ops` against the previous revision. */
  "conversation.view": obj({ conversation: ConversationId, value: opt(unknown()), ops: opt(unknown()) }),
  /** The answer to an input a connector submitted. */
  reply: obj({
    origin: Origin,
    conversation: ConversationId,
    submission: SubmissionId,
    status: oneOf(["done", "unanswered"]),
    text: nullable(str()),
  }),
} as const;
export type NotificationName = keyof typeof Notifications;
export type Notification<N extends NotificationName> = Static<(typeof Notifications)[N]>;

export const paramChecks = Object.fromEntries(
  Object.entries(Methods).map(([name, method]) => [name, compile(method.params)]),
) as { [M in MethodName]: Validator<Params<M>> };

/** JSON-RPC error codes used by the backend. */
export const ErrorCode = {
  ParseError: -32700,
  InvalidRequest: -32600,
  MethodNotFound: -32601,
  InvalidParams: -32602,
  Internal: -32603,
  NotHello: -32000,
  NotFound: -32001,
} as const;
