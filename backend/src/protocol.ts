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
  arr,
  bool,
  compile,
  int,
  lit,
  map,
  nullable,
  obj,
  oneOf,
  opt,
  type Static,
  str,
  union,
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
const ProjectId = str({ minLength: 1, description: "Backend-assigned project id" });

/** A directory that chats run in. */
export const Project = obj({
  id: ProjectId,
  name: str(),
  path: str({ description: "Absolute directory; the cwd of the project's chats" }),
  branch: nullable(str({ description: "Checked-out git branch, null outside a repository or when detached" })),
  createdAt: int({ description: "Unix milliseconds" }),
});
export type Project = Static<typeof Project>;

const Status = oneOf(["idle", "working", "approval", "failed"], {
  description: "working: a turn runs or inputs wait; approval: a permission request waits for the user",
});

export const ConversationSummary = obj({
  id: ConversationId,
  title: str({
    description:
      "Set by the backend: fixed for the orchestrator, a subagent's task, a chat's first message; empty until known",
  }),
  kind: oneOf(["orchestrator", "chat", "subagent"]),
  parent: opt(int({ minimum: 1, description: "For a subagent: the conversation that started it" })),
  project: opt(ProjectId),
  harness: opt(str({ description: "ACP runner name; absent for the orchestrator" })),
  status: Status,
  archived: bool(),
  updatedAt: int({ description: "Unix milliseconds of the last input or turn end; 0 when unknown" }),
});
export type ConversationSummary = Static<typeof ConversationSummary>;

const OptionValue = union([str(), bool()]);

/**
 * A per-conversation setting shown in the composer. ACP chats take them from the runner's session config options
 * (`category` model, thought_level, mode, ...); the orchestrator has `model` and `thinking`; every ACP conversation also
 * has viz's `access` (full: allow every permission request; ask: route them to approvals; read-only: reject them).
 */
export const ConfigOption = obj({
  id: str(),
  name: str(),
  description: opt(str()),
  category: nullable(str({ description: "model | thought_level | mode | access | other ACP categories" })),
  type: oneOf(["select", "boolean"]),
  value: OptionValue,
  choices: arr(
    obj({ value: str(), name: str(), description: opt(str()), group: opt(str({ description: "Group label" })) }),
  ),
});
export type ConfigOption = Static<typeof ConfigOption>;

/** An ACP permission request waiting for the user. */
export const Approval = obj({
  id: str(),
  title: str({ description: "The tool call's title" }),
  kind: nullable(str({ description: "ACP tool kind: read, edit, delete, move, search, execute, fetch, ..." })),
  detail: nullable(str({ description: "The tool call's input or a diff, as text, capped" })),
  options: arr(
    obj({
      id: str(),
      name: str(),
      kind: oneOf(["allow_once", "allow_always", "reject_once", "reject_always"]),
    }),
  ),
});
export type Approval = Static<typeof Approval>;

/** viz's own per-conversation state, beside pi-durable's view. Sent whole on every change. */
export const ConversationState = obj({
  status: Status,
  harness: nullable(str()),
  project: nullable(ProjectId),
  cwd: nullable(str()),
  readOnly: bool({ description: "Inputs are refused: a subagent while its delegating call still runs" }),
  options: arr(ConfigOption, { description: "Empty until the runner's session reports them" }),
  commands: arr(
    obj({ name: str({ description: "Without the leading /" }), description: str(), hint: nullable(str()) }),
  ),
  approvals: arr(Approval),
});
export type ConversationState = Static<typeof ConversationState>;

const Empty = obj({});

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
    result: obj({ conversations: arr(ConversationSummary) }),
  },
  "conversation.create": {
    params: obj({
      project: opt(ProjectId),
      harness: opt(str({ description: "Configured ACP runner; default [backend].default_harness" })),
      options: opt(
        map(OptionValue, {
          description: "Initial ConfigOption values by id, e.g. a model picked before the first message",
        }),
      ),
    }),
    result: obj({ conversation: ConversationId }),
  },
  "conversation.update": {
    params: obj({
      conversation: ConversationId,
      title: opt(str({ minLength: 1 })),
      archived: opt(bool()),
      project: opt(nullable(ProjectId)),
    }),
    result: Empty,
  },
  "conversation.configure": {
    params: obj({ conversation: ConversationId, option: str(), value: OptionValue }),
    result: Empty,
  },
  "conversation.abort": {
    params: obj({ conversation: ConversationId }),
    result: Empty,
  },
  "approval.respond": {
    params: obj({
      conversation: ConversationId,
      approval: str(),
      option: nullable(str({ description: "An Approval option id; null cancels the request" })),
    }),
    result: Empty,
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
  "projects.list": {
    params: Empty,
    result: obj({ projects: arr(Project) }),
  },
  "project.create": {
    params: obj({
      path: str({ minLength: 1, description: "An existing directory; `~` expands to $HOME" }),
      name: opt(str({ minLength: 1, description: "Default: the directory's name" })),
    }),
    result: obj({ project: Project }),
  },
  "project.update": {
    params: obj({ project: ProjectId, name: str({ minLength: 1 }) }),
    result: obj({ project: Project }),
  },
  "project.remove": {
    params: obj({ project: ProjectId }),
    result: Empty,
  },
  "harnesses.list": {
    params: Empty,
    result: obj({ harnesses: arr(obj({ name: str(), default: bool() })) }),
  },
  "files.search": {
    params: obj({
      conversation: ConversationId,
      query: str({ description: "Fuzzy path query; empty lists the first files" }),
      limit: opt(int({ minimum: 1, maximum: 200 })),
    }),
    result: obj({ files: arr(str({ description: "Path relative to the conversation's cwd" })) }),
  },
} as const;
export type MethodName = keyof typeof Methods;
export type Params<M extends MethodName> = Static<(typeof Methods)[M]["params"]>;
export type Result<M extends MethodName> = Static<(typeof Methods)[M]["result"]>;

export const Notifications = {
  /** A conversation view revision: the full `value` first, then Chord delta `ops` against the previous revision. */
  "conversation.view": obj({ conversation: ConversationId, value: opt(unknown()), ops: opt(unknown()) }),
  /** A watched conversation's ConversationState: once after `conversation.watch`, then whole on every change. */
  "conversation.state": obj({ conversation: ConversationId, state: ConversationState }),
  /** Something `conversations.list` or `projects.list` returns changed; apps list again. Sent to apps only. */
  "conversations.changed": Empty,
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
