/**
 * The orchestrator's tools for driving viz itself (t3code's thread toolkit, uniichat's `spawn`/`tell`): list, read,
 * and search conversations; start a chat or message one, each reporting back to the orchestrator as a new input when
 * it settles; and stop a conversation's turn.
 *
 * Starting or messaging a chat is one commit of the tool's task: it creates the chat (or queues the input), creates
 * the background report task on the orchestrator, and marks the call done in a task-scoped document. A rerun after a
 * restart finds the mark and returns the same conversation, so the tools are replay-safe without resending anything.
 */
import type { Context } from "@earendil-works/chord/context";
import {
  type ConversationId,
  defineDoc,
  defineExtension,
  defineTask,
  defineTool,
  type Extension,
  type Harness,
  type SubmissionId,
  type ToolExecutionApi,
  type Tx,
} from "@earendil-works/pi-durable";
import { bool, int, obj, opt, str } from "@earendil-works/pi-schema";

import type { ChatScheduler } from "./agents/chat.ts";
import type { Runners } from "./agents/runners.ts";
import { parentOf, type Conversations } from "./conversations.ts";
import { type ChatWorkspace, conversationRecords, MetaDoc, type ProjectStore } from "./projects.ts";
import { Workspace } from "./protocol.ts";
import type { SearchIndex } from "./search.ts";
import { TitleDoc } from "./titles.ts";
import { settledAnswer, transcript } from "./transcripts.ts";

export type ToolkitHost = {
  readonly harness: Harness;
  readonly root: ConversationId;
  readonly conversations: Conversations;
  readonly projects: ProjectStore;
  readonly search: SearchIndex;
  readonly runners: Runners;
  readonly chats: ChatScheduler;
  workspace(
    project: string | undefined,
    requested: Workspace | undefined,
    context: Context,
  ): Promise<ChatWorkspace | null>;
  editWorkspaces<T>(use: () => Promise<T>): Promise<T>;
  /** Submits `text` to the orchestrator once per `requestId`. */
  report(text: string, requestId: string, context: Context): Promise<void>;
  abort(id: ConversationId, context: Context): Promise<void>;
};

/** Marks a start/message call done: the conversation it reached. Lives as long as the tool's task. */
const CallDoc = defineDoc<{ conversation: number | null }>({
  kind: "viz.orchestrator.call",
  version: 1,
  scope: "task",
  initial: () => ({ conversation: null }),
  checkpointWhen: () => true,
});

const conversation = int({ minimum: 1, description: "Conversation id" });
const limit = (maximum: number) => opt(int({ minimum: 1, maximum }));
const text = (value: string) => ({ output: [{ type: "text" as const, text: value }] });

export function orchestratorToolkit(host: () => ToolkitHost): Extension {
  const topLevelRecord = async (tx: Tx, id: ConversationId) => {
    if (id === host().root) return;
    const record = await tx.conversation(id);
    if (!record) return;
    const meta = await tx.doc(MetaDoc, id);
    if (!meta.deleted && parentOf(record, meta) === null) return record;
  };

  const topLevel = async (tx: Tx, id: ConversationId) => {
    if (id === host().root) throw new Error("That is this conversation");
    const record = await topLevelRecord(tx, id);
    if (!record) throw new Error("Conversation not found");
    return record;
  };

  const Report = defineTask<{ conversation: number; submission: number }, { phase: "wait" }, null>({
    name: "viz.orchestrator.report",
    version: 1,
    initial: () => ({ phase: "wait" }),
    phases: {
      // Waits across restarts: shutdown interrupts the wait, which resumes in the next run.
      wait: async (task, runtime, context) => {
        const { harness, conversations, report } = host();
        const id = task.input.conversation as ConversationId;
        const submission = await harness.submission(task.input.submission as SubmissionId, context);
        if (!submission) throw new Error("Input not found");
        const result = await settledAnswer(harness, await submission.wait(context), context);
        const record = await harness.commit((tx) => topLevelRecord(tx, id), context);
        if (record) {
          const { title, branch, worktree } = await conversations.summary(record, context);
          const head = `[chat ${id}${title ? ` "${title}"` : ""}]`;
          const checkout = worktree ? `\nWorktree: ${worktree}${branch ? `\nBranch: ${branch}` : ""}\n` : "";
          await report(
            "answer" in result
              ? `${head} finished:\n${checkout}\n${result.answer}`
              : `${head} stopped (${result.unanswered})${result.error ? `: ${result.error}` : ""}${checkout}`,
            `viz.report:${runtime.taskId}`,
            context,
          );
        }
        await runtime.commit(() => ({ status: "terminal", outcome: { status: "completed", result: null } }), context);
      },
    },
    abort: (_task, runtime, context) =>
      runtime.commit(() => ({ status: "terminal", outcome: { status: "aborted" } }), context),
  });

  /** Runs `start` once per tool call, with the report task that tells the orchestrator how it went. */
  const admit = (
    api: ToolExecutionApi,
    context: Context,
    start: (tx: Tx) => Promise<{ conversation: ConversationId; submission: SubmissionId }>,
  ) =>
    api.commit(async (tx) => {
      const call = await tx.doc(CallDoc, api.taskId);
      if (call.conversation !== null) {
        const id = call.conversation as ConversationId;
        await topLevel(tx, id);
        return id;
      }
      const { conversation, submission } = await start(tx);
      await tx.createTask(
        Report,
        { conversation, submission },
        { conversationId: host().root, ownership: { kind: "conversation" }, background: true },
      );
      call.conversation = conversation;
      return conversation;
    }, context);

  return defineExtension({
    name: "viz.orchestrator.tools",
    tasks: [Report],
    tools: [
      defineTool({
        name: "list_conversations",
        description: "List projects and top-level ACP chats, newest first.",
        parameters: obj({ archived: opt(bool({ description: "Include archived" })), limit: limit(200) }),
        replay: "safe",
        execute: async ({ archived = false, limit = 50 }, _api, context) => {
          const { projects, conversations } = host();
          const [all, summaries] = await Promise.all([
            projects.list(context),
            conversations.list(context, "top-level"),
          ]);
          const listed = summaries
            .filter((summary) => archived || !summary.archived)
            .sort((a, b) => b.updatedAt - a.updatedAt)
            .slice(0, limit)
            .map(({ updatedAt, archived, pinned, ...summary }) => ({
              ...summary,
              ...(archived && { archived }),
              ...(pinned && { pinned }),
              updated: new Date(updatedAt).toISOString(),
            }));
          return text(
            JSON.stringify({
              projects: all.map(({ id, name, path, branch, defaultWorkspace }) => ({
                id,
                name,
                path,
                branch,
                defaultWorkspace,
              })),
              conversations: listed,
            }),
          );
        },
      }),
      defineTool({
        name: "read_conversation",
        description: "Read a top-level chat's recent transcript.",
        parameters: obj({ conversation, limit: limit(200) }),
        replay: "safe",
        execute: async ({ conversation, limit = 30 }, _api, context) => {
          const { harness } = host();
          const id = conversation as ConversationId;
          await harness.commit((tx) => topLevel(tx, id), context);
          return text((await transcript(harness, id, limit, context)) || "(empty)");
        },
      }),
      defineTool({
        name: "search_conversations",
        description: "Search chats and their subagents, returning one match per top-level chat.",
        parameters: obj({ query: str({ minLength: 1 }), limit: limit(50) }),
        replay: "safe",
        execute: async ({ query, limit = 20 }, _api, context) => {
          const { harness, root, search } = host();
          const groups = await harness.commit(async (tx) => {
            const groups = new Map<number, number>();
            for await (const record of conversationRecords(tx)) {
              if (record.id === root) continue;
              const meta = await tx.doc(MetaDoc, record.id);
              if (meta.deleted) continue;
              const parent = parentOf(record, meta);
              const chat = parent === null ? record.id : groups.get(parent);
              if (chat !== undefined) groups.set(record.id, chat);
            }
            return groups;
          }, context);
          const lines: string[] = [];
          for (const { conversation, snippet } of search.search(query, limit, groups).hits) {
            const title = (await harness.snapshot(TitleDoc, conversation as ConversationId, context))?.title;
            lines.push(`[${conversation}${title ? ` "${title}"` : ""}] ${snippet.replace(/[\u0001\u0002]/g, "**")}`);
          }
          return text(lines.join("\n") || "No matches");
        },
      }),
      defineTool({
        name: "spawn_chat",
        description:
          "Start an ACP chat on a task in the background. It reports back here when done. Choose a project workspace to override its default. A new worktree excludes uncommitted edits; you integrate the work yourself.",
        parameters: obj({
          task: str({ minLength: 1, description: "Task and context" }),
          project: opt(str({ description: "Project id or name; omit for none" })),
          harness: opt(str({ description: "ACP harness; omit for the default" })),
          workspace: opt(Workspace),
        }),
        replay: "safe",
        execute: async ({ task, project, harness, workspace }, api, context) => {
          const { projects, runners, chats, workspace: prepare, editWorkspaces } = host();
          const { id, selected, binding } = await editWorkspaces(async () => {
            const selected =
              project === undefined
                ? undefined
                : (await projects.list(context)).find(({ id, name }) => id === project || name === project);
            if (project !== undefined && !selected) throw new Error("Project not found");
            runners.require(harness);
            // A rerun reuses the same workspace rather than creating another checkout.
            const binding =
              workspace?.kind === "existing"
                ? await prepare(selected?.id, workspace, context)
                : (
                    (await api.memo<{ binding: ChatWorkspace | null }>("workspace", context)) ??
                    (await api.memo("workspace", { binding: await prepare(selected?.id, workspace, context) }, context))
                  ).binding;
            const id = await admit(api, context, async (tx) => {
              const record = await tx.createConversation({ ownership: { kind: "ownerless" } });
              await chats.setup(tx, record.id, {
                ...(harness !== undefined && { harness }),
                ...(selected && { project: selected.id }),
                ...binding,
              });
              const submission = await chats.enqueue(tx, record, { text: task, content: task });
              return { conversation: record.id, submission };
            });
            return { id, selected, binding };
          });
          await api.details({ conversationId: id }, context);
          const checkout = binding
            ? `\nWorktree: ${binding.worktree.path}${binding.branchToName ? `\nInitial branch: ${binding.branchToName}` : ""}`
            : "";
          return text(`Started chat ${id}${selected ? ` in ${selected.name}` : ""}.${checkout}`);
        },
      }),
      defineTool({
        name: "message_conversation",
        description: "Queue a message to a top-level chat. Its reply reports back here.",
        parameters: obj({ conversation, text: str({ minLength: 1 }) }),
        replay: "safe",
        execute: async ({ conversation, text: message }, api, context) => {
          const id = conversation as ConversationId;
          await admit(api, context, async (tx) => {
            const record = await topLevel(tx, id);
            return {
              conversation: id,
              submission: await host().chats.enqueue(tx, record, { text: message, content: message }),
            };
          });
          return text(`Queued for ${id}.`);
        },
      }),
      defineTool({
        name: "cancel_conversation",
        description: "Stop a top-level chat's turn and drop its queue.",
        parameters: obj({ conversation }),
        replay: "safe",
        execute: async ({ conversation }, _api, context) => {
          const { harness, abort } = host();
          const id = conversation as ConversationId;
          await harness.commit((tx) => topLevel(tx, id), context);
          await abort(id, context);
          return text(`Stopped ${id}.`);
        },
      }),
    ],
  });
}
