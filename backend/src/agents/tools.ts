import type { Context } from "@earendil-works/chord/context";
import type { Conversation, ConversationId, Harness, Submission } from "@earendil-works/pi-durable";
import { bool, int, obj, oneOf, opt, str } from "@earendil-works/pi-schema";

import type { Conversations } from "../conversations.ts";
import type { CreatedWorktree } from "../git/index.ts";
import { type McpTool, tool } from "../mcp/server.ts";
import { conversationRecords, MetaDoc } from "../projects.ts";
import { titleFrom } from "../titles.ts";
import { settledAnswer, transcript } from "../transcripts.ts";
import type { ChatInput, ChatOptions } from "./chat.ts";
import type { ToolScope } from "./sessions.ts";

type Subagents = {
  harness: Harness;
  conversations: Conversations;
  spawn: (
    options: ChatOptions & { parent: ConversationId },
    task: string,
    isolation: Isolation,
    context: Context,
  ) => Promise<{ child: Conversation; isolated?: CreatedWorktree }>;
  submit: (id: ConversationId, input: ChatInput, context: Context) => Promise<Submission>;
  abort: (id: ConversationId, context: Context) => Promise<void>;
};

const conversation = int({ minimum: 1 });
const text = str({ minLength: 1 });
type Isolation = "shared" | "worktree";

export function agentTools({ harness, conversations, spawn, submit, abort }: Subagents): McpTool<ToolScope>[] {
  async function requireChild(caller: ConversationId, target: ConversationId, context: Context) {
    const allowed = await harness.commit(async (tx) => {
      let id: ConversationId | null = target;
      while (id !== null && id > caller) {
        const record = await tx.conversation(id);
        if (!record) return false;
        const meta: { deleted?: boolean; parent: ConversationId | null } = await tx.doc(MetaDoc, id);
        // Deleting a conversation deletes its descendants, so a deleted ancestor implies a deleted target.
        if (meta.deleted) return false;
        const { parent } = meta;
        if (parent !== null && parent >= id) return false;
        id = parent;
      }
      return target !== caller && id === caller;
    }, context);
    if (!allowed) throw new Error("Subagent not found");
    return target;
  }

  async function send(id: ConversationId, text: string, wait: boolean, context: Context, isolated?: CreatedWorktree) {
    const content = isolated
      ? `Isolated worktree: ${isolated.worktree.path}\nBranch: ${isolated.branch}\nBase commit: ${isolated.commit}\nYour parent will integrate your work.\n\n${text}`
      : text;
    const submission = await submit(id, { text, content }, context);
    const details = {
      conversation: id,
      ...(isolated && { branch: isolated.branch, worktree: isolated.worktree.path }),
    };
    if (!wait) return JSON.stringify(details);
    const result = await settledAnswer(harness, await submission.wait(context), context);
    if ("answer" in result) return JSON.stringify({ ...details, text: result.answer });
    const { unanswered: reason, error } = result;
    return JSON.stringify({ ...details, status: "unanswered", reason, ...(error && { error }) });
  }

  return [
    tool<ToolScope, { text: string }>({
      name: "report_progress",
      description: "Record a progress note.",
      input: obj({ text: str({ minLength: 1, maxLength: 4096 }) }),
      call: async ({ text }, scope) => {
        await scope.active().report(text);
        return { text: "Progress recorded" };
      },
    }),
    tool<ToolScope, { task: string; harness?: string; wait?: boolean; isolation?: Isolation }>({
      name: "spawn_subagent",
      description:
        "Start a subagent. Shared (default): same checkout as you, sees your uncommitted edits, and edits can collide. " +
        "Worktree: separate checkout and branch from your HEAD, excluding your uncommitted edits. Use it for parallel or independent write work; integrate the result yourself (merge, cherry-pick, or PR).",
      input: obj({
        task: text,
        harness: opt(str()),
        wait: opt(bool()),
        isolation: opt(oneOf(["shared", "worktree"])),
      }),
      call: async ({ task, harness, wait = true, isolation = "shared" }, scope) => {
        const { context } = scope.active();
        const { child, isolated } = await spawn(
          { parent: scope.id, title: titleFrom(task), ...(harness !== undefined && { harness }) },
          task,
          isolation,
          context,
        );
        return { text: await send(child.id, task, wait, context, isolated), child: child.id };
      },
    }),
    tool<ToolScope, Record<string, never>>({
      name: "list_subagents",
      description: "List your direct subagents.",
      input: obj({}),
      call: async (_args, scope) => {
        const { context } = scope.active();
        const records = await harness.commit(async (tx) => {
          const children = [];
          for await (const record of conversationRecords(tx)) {
            if (record.id <= scope.id) continue;
            const meta = await tx.doc(MetaDoc, record.id);
            if (!meta.deleted && meta.parent === scope.id) children.push(record);
          }
          return children;
        }, context);
        const children = [];
        for (const record of records) {
          const { id, title, status } = await conversations.summary(record, context);
          children.push({ id, title, status });
        }
        return { text: JSON.stringify(children) };
      },
    }),
    tool<ToolScope, { conversation: number; limit?: number }>({
      name: "read_subagent",
      description: "Read a subagent's recent transcript.",
      input: obj({ conversation, limit: opt(int({ minimum: 1, maximum: 100 })) }),
      call: async ({ conversation, limit = 20 }, scope) => {
        const { context } = scope.active();
        const id = await requireChild(scope.id, conversation as ConversationId, context);
        return { text: await transcript(harness, id, limit, context), child: id };
      },
    }),
    tool<ToolScope, { conversation: number; text: string; wait?: boolean }>({
      name: "message_subagent",
      description: "Send a follow-up to a subagent.",
      input: obj({ conversation, text, wait: opt(bool()) }),
      call: async ({ conversation, text, wait = true }, scope) => {
        const { context } = scope.active();
        const child = await requireChild(scope.id, conversation as ConversationId, context);
        return { text: await send(child, text, wait, context), child };
      },
    }),
    tool<ToolScope, { conversation: number }>({
      name: "cancel_subagent",
      description: "Stop a subagent's turn.",
      input: obj({ conversation }),
      call: async ({ conversation }, scope) => {
        const { context } = scope.active();
        const child = await requireChild(scope.id, conversation as ConversationId, context);
        await abort(child, context);
        return { text: "Subagent stopped", child };
      },
    }),
  ];
}
