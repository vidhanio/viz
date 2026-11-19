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
import type { Conversations } from "./conversations.ts";
import type { ProjectStore, WorktreeBinding } from "./projects.ts";
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
  /** A new worktree of `project` on new branch `branch`. */
  worktree(project: string, branch: string, context: Context): Promise<WorktreeBinding>;
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
  const Report = defineTask<{ conversation: number; submission: number }, { phase: "wait" }, null>({
    name: "viz.orchestrator.report",
    version: 1,
    initial: () => ({ phase: "wait" }),
    phases: {
      // Waits across restarts: shutdown interrupts the wait, which resumes in the next run.
      wait: async (task, runtime, context) => {
        const { harness, report } = host();
        const id = task.input.conversation as ConversationId;
        const submission = await harness.submission(task.input.submission as SubmissionId, context);
        if (!submission) throw new Error("Input not found");
        const result = await settledAnswer(harness, await submission.wait(context), context);
        const title = (await harness.snapshot(TitleDoc, id, context))?.title;
        const head = `[chat ${id}${title ? ` "${title}"` : ""}]`;
        await report(
          "answer" in result
            ? `${head} finished:\n\n${result.answer}`
            : `${head} stopped (${result.unanswered})${result.error ? `: ${result.error}` : ""}`,
          `viz.report:${runtime.taskId}`,
          context,
        );
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
      if (call.conversation !== null) return call.conversation as ConversationId;
      const { conversation, submission } = await start(tx);
      await tx.createTask(
        Report,
        { conversation, submission },
        { conversationId: host().root, ownership: { kind: "conversation" }, background: true },
      );
      call.conversation = conversation;
      return conversation;
    }, context);

  const target = (id: number) => {
    if (id === host().root) throw new Error("That is this conversation");
    return id as ConversationId;
  };

  return defineExtension({
    name: "viz.orchestrator.tools",
    tasks: [Report],
    tools: [
      defineTool({
        name: "list_conversations",
        description: "List projects and conversations, newest first.",
        parameters: obj({ archived: opt(bool({ description: "Include archived" })), limit: limit(200) }),
        replay: "safe",
        execute: async ({ archived = false, limit = 50 }, _api, context) => {
          const { projects, conversations } = host();
          const [all, summaries] = await Promise.all([projects.list(context), conversations.list(context)]);
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
              projects: all.map(({ id, name, path, branch }) => ({ id, name, path, branch })),
              conversations: listed,
            }),
          );
        },
      }),
      defineTool({
        name: "read_conversation",
        description: "Read a conversation's recent transcript.",
        parameters: obj({ conversation, limit: limit(200) }),
        replay: "safe",
        execute: async ({ conversation, limit = 30 }, _api, context) =>
          text((await transcript(host().harness, conversation as ConversationId, limit, context)) || "(empty)"),
      }),
      defineTool({
        name: "search_conversations",
        description: "Search all transcripts.",
        parameters: obj({ query: str({ minLength: 1 }), limit: limit(50) }),
        replay: "safe",
        execute: async ({ query, limit = 20 }, _api, context) => {
          const { harness, search } = host();
          const titles = new Map<number, string>();
          const lines: string[] = [];
          for (const { conversation, snippet } of search.search(query, limit).hits) {
            let title = titles.get(conversation);
            if (title === undefined) {
              title = (await harness.snapshot(TitleDoc, conversation as ConversationId, context))?.title ?? "";
              titles.set(conversation, title);
            }
            lines.push(`[${conversation}${title ? ` "${title}"` : ""}] ${snippet.replace(/[\u0001\u0002]/g, "**")}`);
          }
          return text(lines.join("\n") || "No matches");
        },
      }),
      defineTool({
        name: "spawn_chat",
        description: "Start an ACP chat on a task in the background. It reports back here when done.",
        parameters: obj({
          task: str({ minLength: 1, description: "Task and context" }),
          project: opt(str({ description: "Project id or name; omit for none" })),
          harness: opt(str({ description: "ACP harness; omit for the default" })),
          worktree: opt(str({ description: "Run in a new worktree on this new branch" })),
        }),
        replay: "safe",
        execute: async ({ task, project, harness, worktree }, api, context) => {
          const { projects, runners, chats } = host();
          const selected =
            project === undefined
              ? undefined
              : (await projects.list(context)).find(({ id, name }) => id === project || name === project);
          if (project !== undefined && !selected) throw new Error("Project not found");
          if (worktree !== undefined && !selected) throw new Error("A worktree requires a project");
          runners.require(harness);
          // A rerun reuses the worktree an earlier run created.
          const binding =
            worktree === undefined
              ? undefined
              : ((await api.memo<WorktreeBinding>("worktree", context)) ??
                (await api.memo("worktree", await host().worktree(selected!.id, worktree, context), context)));
          const id = await admit(api, context, async (tx) => {
            const record = await tx.createConversation({ ownership: { kind: "ownerless" } });
            await chats.setup(tx, record.id, {
              ...(harness !== undefined && { harness }),
              ...(selected && { project: selected.id }),
              ...(binding && { worktree: binding }),
            });
            const submission = await chats.enqueue(tx, record, { text: task, content: task });
            return { conversation: record.id, submission };
          });
          await api.details({ conversationId: id }, context);
          return text(`Started chat ${id}${selected ? ` in ${selected.name}` : ""}.`);
        },
      }),
      defineTool({
        name: "message_conversation",
        description: "Queue a message to a chat or subagent. Its reply reports back here.",
        parameters: obj({ conversation, text: str({ minLength: 1 }) }),
        replay: "safe",
        execute: async ({ conversation, text: message }, api, context) => {
          const id = target(conversation);
          await admit(api, context, async (tx) => {
            const record = await tx.conversation(id);
            if (!record) throw new Error("Conversation not found");
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
        description: "Stop a conversation's turn and drop its queue.",
        parameters: obj({ conversation }),
        replay: "safe",
        execute: async ({ conversation }, _api, context) => {
          const id = target(conversation);
          await host().abort(id, context);
          return text(`Stopped ${id}.`);
        },
      }),
    ],
  });
}
