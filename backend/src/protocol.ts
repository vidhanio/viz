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

const Status = oneOf(["idle", "working", "failed"], { description: "working: a turn runs or inputs wait" });

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
  unread: bool({ description: "A turn ended while no app watched the conversation; watching clears it" }),
  pinned: bool(),
  worktree: opt(str({ description: "Path of the viz-created git worktree the conversation runs in" })),
});
export type ConversationSummary = Static<typeof ConversationSummary>;

const OptionValue = union([str(), bool()]);

/**
 * A per-conversation setting shown in the composer. ACP chats take them from the runner's session config options
 * (`category` model, thought_level, mode, ...); the orchestrator has `model` and `thinking`.
 */
export const ConfigOption = obj({
  id: str(),
  name: str(),
  description: opt(str()),
  category: nullable(str({ description: "model | thought_level | mode | other ACP categories" })),
  type: oneOf(["select", "boolean"]),
  value: OptionValue,
  choices: arr(
    obj({ value: str(), name: str(), description: opt(str()), group: opt(str({ description: "Group label" })) }),
  ),
});
export type ConfigOption = Static<typeof ConfigOption>;

/** The conversation's working tree, when its cwd is in a git repository. */
export const GitState = obj({
  branch: nullable(str({ description: "Null when detached" })),
  upstream: nullable(str()),
  ahead: int({ minimum: 0 }),
  behind: int({ minimum: 0 }),
  worktree: nullable(str({ description: "Path of the viz-created worktree this conversation runs in" })),
  files: arr(
    obj({
      path: str({ description: "Relative to the repository root" }),
      status: oneOf(["modified", "added", "deleted", "renamed", "untracked", "conflicted"]),
      staged: bool(),
    }),
  ),
});
export type GitState = Static<typeof GitState>;

/** A file sent with an input. Images go to runners that take them; everything is also saved for the agent to read. */
export const Attachment = obj({
  name: str({ minLength: 1 }),
  mime: str({ description: "e.g. image/png, text/plain" }),
  data: str({ description: "Base64, at most 20 MiB decoded" }),
});
export type Attachment = Static<typeof Attachment>;

const ConversationTarget = obj({ conversation: ConversationId });

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
  queue: arr(obj({ submission: SubmissionId, text: str() }), { description: "Inputs waiting for the next turn" }),
  git: nullable(GitState),
  attachments: obj(
    { images: bool({ description: "The runner takes image prompts" }) },
    { description: "What attachments the runner accepts; files always go as paths" },
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
      worktree: opt(
        obj(
          { branch: str({ minLength: 1 }), base: opt(str({ description: "Default: the project's current branch" })) },
          { description: "Run the chat in a new git worktree of the project on a new branch" },
        ),
      ),
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
    result: obj({
      kept: arr(str(), {
        description: "Worktree paths left on disk rather than removed",
      }),
    }),
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
  "git.diff": {
    params: obj({
      conversation: ConversationId,
      scope: oneOf(["worktree", "turn", "branch"], {
        description: "worktree: uncommitted changes; turn: since the start of `turn`; branch: against the base branch",
      }),
      turn: opt(int({ minimum: 1, description: "For scope turn: the prompt entry id; default the latest turn" })),
      path: opt(str()),
    }),
    result: obj({
      files: arr(
        obj({
          path: str(),
          oldPath: nullable(str()),
          status: oneOf(["modified", "added", "deleted", "renamed"]),
          additions: int(),
          deletions: int(),
          patch: str({ description: "Unified diff of this file, capped at 200 KB" }),
        }),
      ),
    }),
  },
  "git.branches": {
    params: ConversationTarget,
    result: obj({ current: nullable(str()), branches: arr(str()) }),
  },
  "git.checkout": {
    params: obj({ conversation: ConversationId, branch: str({ minLength: 1 }), create: opt(bool()) }),
    result: Empty,
  },
  "git.commit": {
    params: obj({
      conversation: ConversationId,
      message: opt(str({ description: "Empty or absent: written by the [memory].compactor model from the diff" })),
      all: opt(bool({ description: "Stage every change first; default true" })),
    }),
    result: obj({ commit: str(), message: str() }),
  },
  "git.push": {
    params: ConversationTarget,
    result: Empty,
  },
  "git.pr": {
    params: obj({ conversation: ConversationId, title: opt(str()), body: opt(str()), draft: opt(bool()) }),
    result: obj({ url: str() }),
  },
  "git.revert": {
    params: obj({
      conversation: ConversationId,
      turn: int({ minimum: 1, description: "Restore the working tree to the start of this turn (prompt entry id)" }),
    }),
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
