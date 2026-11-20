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
  createdAt: int({ description: "Unix milliseconds" }),
});
export type Project = Static<typeof Project>;

const Status = oneOf(["idle", "working", "failed"], { description: "working: a turn runs or inputs wait" });

export const ConversationSummary = obj({
  id: ConversationId,
  title: str({
    description: "A user rename, an ACP session title, or a title from the first prompt; fixed for the orchestrator",
  }),
  kind: oneOf(["orchestrator", "chat", "subagent"]),
  parent: opt(int({ minimum: 1, description: "For a subagent: the conversation that started it" })),
  project: opt(ProjectId),
  harness: opt(str({ description: "ACP runner name; absent for the orchestrator" })),
  status: Status,
  archived: bool(),
  updatedAt: int({ description: "Unix milliseconds of the last input or turn end; 0 when unknown" }),
  unread: bool({ description: "A turn ended while no app watched the conversation; watching clears it" }),
  pinned: bool(),
  model: opt(str({ description: "Display name of the selected model, once the runner or orchestrator reports it" })),
  thinking: opt(str({ description: "Display name of the selected thinking level, when the model has one" })),
});
export type ConversationSummary = Static<typeof ConversationSummary>;

const OptionValue = union([str(), bool()]);

const choiceFields = { value: str(), name: str(), description: opt(str()) };
const configFields = {
  id: str(),
  name: str(),
  description: opt(str()),
  value: OptionValue,
};
export const ModelThinking = obj(
  { ...configFields, choices: arr(obj(choiceFields)) },
  { description: "Thinking levels reported for this model" },
);
export type ModelThinking = Static<typeof ModelThinking>;

/**
 * A per-conversation setting shown in the composer. ACP chats take them from the runner's session config options
 * (`category` model, thought_level, mode, ...); the orchestrator has `model` and `thinking`.
 */
export const ConfigOption = obj({
  ...configFields,
  category: nullable(str({ description: "model | thought_level | mode | other ACP categories" })),
  type: oneOf(["select", "boolean"]),
  choices: arr(
    obj({
      ...choiceFields,
      group: opt(str({ description: "Group label, e.g. the model's provider" })),
      thinking: opt(ModelThinking),
    }),
  ),
});
export type ConfigOption = Static<typeof ConfigOption>;

/** A file sent with an input. Images go to runners that take them; everything is also saved for the agent to read. */
export const Attachment = obj({
  name: str({ minLength: 1 }),
  mime: str({ description: "e.g. image/png, text/plain" }),
  data: str({ description: "Base64, at most 20 MiB decoded" }),
});
export type Attachment = Static<typeof Attachment>;

/** An ACP tool call: the data of a `viz.agent.tool` entry, or an unfinished call in `LiveTurn.tools`. */
export const ToolRecord = obj({
  id: str({ description: "ACP toolCallId; unique within one turn" }),
  title: str(),
  name: opt(str({ description: "Programmatic tool name, when reported by the runner" })),
  kind: nullable(str({ description: "ACP ToolKind: read | edit | execute | search | fetch | think | other ..." })),
  status: oneOf(["pending", "in_progress", "completed", "failed"]),
  viz: opt(str({ description: "The viz tool this call ran, when it was one" })),
  child: opt(ConversationId),
  startedAt: opt(int({ description: "Unix milliseconds when viz first observed the call running" })),
  input: opt(str({ description: "Raw input: a string as sent, else JSON (fields such as command, path, timeout)" })),
  output: opt(str({ description: "ACP text content, else readable raw output" })),
  diffs: opt(arr(obj({ path: str(), oldText: nullable(str()), newText: str() }))),
  locations: opt(arr(str())),
});
export type ToolRecord = Static<typeof ToolRecord>;

/**
 * The running ACP turn's uncommitted tail, cleared by the commit that appends it as entries or ends the turn. Live text
 * comes before live tools, the order they will be committed in.
 */
export const LiveTurn = obj({
  turn: str({ description: "EntryId of the turn's viz.agent.prompt entry" }),
  text: opt(
    obj({
      key: str({ description: "Also `data.key` of the viz.agent.text or viz.agent.thought entry that commits it" }),
      kind: oneOf(["text", "thought"]),
      text: str(),
    }),
  ),
  tools: arr(ToolRecord, { description: "Unfinished tool calls, in the order they started" }),
});

/** viz's own per-conversation state, beside pi-durable's view. Sent whole on every change. */
export const ConversationState = obj({
  status: Status,
  harness: nullable(str()),
  project: nullable(ProjectId),
  cwd: nullable(str()),
  options: arr(ConfigOption, { description: "Empty until the runner's session reports them" }),
  commands: arr(
    obj({ name: str({ description: "Without the leading /" }), description: str(), hint: nullable(str()) }),
  ),
  queue: arr(obj({ submission: SubmissionId, text: str() }), { description: "Inputs waiting for the next turn" }),
  attachments: obj(
    { images: bool({ description: "The runner takes image prompts" }) },
    { description: "What attachments the runner accepts; files always go as paths" },
  ),
  live: nullable(LiveTurn, { description: "An ACP turn's in-progress text and tools; null when none runs" }),
  usage: nullable(
    obj({ used: int({ description: "Tokens in context" }), size: int({ description: "Context window in tokens" }) }),
    { description: "The ACP runner's last usage_update; null until it sends one" },
  ),
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
      pinned: opt(bool()),
      unread: opt(bool()),
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
  "conversation.delete": {
    params: obj({ conversation: ConversationId }),
    result: Empty,
  },
  "conversation.submit": {
    params: obj({
      conversation: opt(int({ minimum: 1, description: "Default: the root (orchestrator) conversation" })),
      text: str(),
      requestId: opt(str({ description: "Idempotency key: a retry with the same id returns the same submission" })),
      origin: opt(Origin),
      attachments: opt(arr(Attachment)),
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
    params: obj({ project: ProjectId, name: opt(str({ minLength: 1 })) }),
    result: obj({ project: Project }),
  },
  "project.remove": {
    params: obj({ project: ProjectId }),
    result: Empty,
  },
  "harnesses.list": {
    params: Empty,
    result: obj({
      harnesses: arr(obj({ name: str(), default: bool(), options: arr(ConfigOption) })),
    }),
  },
  "harnesses.options": {
    params: obj({ harness: str(), model: str() }),
    result: obj({ options: arr(ConfigOption) }),
  },
  "files.search": {
    params: obj({
      conversation: ConversationId,
      query: str({ description: "Fuzzy path query; empty lists the first files" }),
      limit: opt(int({ minimum: 1, maximum: 200 })),
    }),
    result: obj({ files: arr(str({ description: "Path relative to the conversation's cwd" })) }),
  },
  "conversations.search": {
    params: obj({ query: str({ minLength: 1 }), limit: opt(int({ minimum: 1, maximum: 200 })) }),
    result: obj({
      hits: arr(
        obj({
          conversation: ConversationId,
          entry: int({ minimum: 1, description: "pi-durable EntryId" }),
          snippet: str({ description: "Matching text with the match wrapped in \u0001 \u0002 markers" }),
        }),
      ),
    }),
  },
  "conversation.entries": {
    params: obj({
      conversation: ConversationId,
      before: int({ minimum: 1, description: "Return entries with smaller ids than this" }),
      limit: opt(int({ minimum: 1, maximum: 200 })),
    }),
    result: obj({
      entries: arr(unknown(), { description: "pi-durable EntryRecords in ascending id order, as in the view" }),
      before: opt(int({ description: "Set when older entries remain" })),
    }),
  },
  "queue.remove": {
    params: obj({ conversation: ConversationId, submission: SubmissionId }),
    result: Empty,
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
  /** A conversation, project, or harness list changed. Sent to apps only. */
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
