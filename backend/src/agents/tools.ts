import type { Context } from "@earendil-works/chord/context";
import type { Conversation, ConversationId, Harness, Submission } from "@earendil-works/pi-durable";
import { bool, int, obj, opt, str } from "@earendil-works/pi-schema";

import { parentOf, type Conversations } from "../conversations.ts";
import { type McpTool, tool } from "../mcp/server.ts";
import { conversationRecords, MetaDoc } from "../projects.ts";
import { titleFrom } from "../titles.ts";
import { settledAnswer, transcript } from "../transcripts.ts";
import type { ChatInput, ChatOptions } from "./chat.ts";
import type { ToolScope } from "./sessions.ts";

type Subagents = {
  harness: Harness;
  conversations: Conversations;
  create: (options: ChatOptions, context: Context) => Promise<Conversation>;
  submit: (id: ConversationId, input: ChatInput, context: Context) => Promise<Submission>;
  abort: (id: ConversationId, context: Context) => Promise<void>;
};

const conversation = int({ minimum: 1 });
const text = str({ minLength: 1 });

export function agentTools({ harness, conversations, create, submit, abort }: Subagents): McpTool<ToolScope>[] {
  async function requireChild(caller: ConversationId, target: ConversationId, context: Context) {
    const allowed = await harness.commit(async (tx) => {
      let id: ConversationId | null = target;
      while (id !== null && id > caller) {
        const record = await tx.conversation(id);
        if (!record) return false;
        const meta = await tx.doc(MetaDoc, id);
        // Deleting a conversation deletes its descendants, so a deleted ancestor implies a deleted target.
        if (meta.deleted) return false;
        const parent = parentOf(record, record.owner ? undefined : meta);
        if (parent !== null && parent >= id) return false;
        id = parent;
      }
      return target !== caller && id === caller;
    }, context);
    if (!allowed) throw new Error("Subagent not found");
    return target;
  }

  async function send(id: ConversationId, text: string, wait: boolean, context: Context) {
    const submission = await submit(id, { text, content: text }, context);
    if (!wait) return JSON.stringify({ conversation: id });
    const result = await settledAnswer(harness, await submission.wait(context), context);
    if ("answer" in result) return JSON.stringify({ conversation: id, text: result.answer });
    const { unanswered: reason, error } = result;
    return JSON.stringify({ conversation: id, status: "unanswered", reason, ...(error && { error }) });
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
    tool<ToolScope, { task: string; harness?: string; wait?: boolean }>({
      name: "spawn_subagent",
      description: "Start a subagent in this project.",
      input: obj({ task: text, harness: opt(str()), wait: opt(bool()) }),
      call: async ({ task, harness, wait = true }, scope) => {
        const { context } = scope.active();
        const child = await create(
          { parent: scope.id, title: titleFrom(task), ...(harness !== undefined && { harness }) },
          context,
        );
        return { text: await send(child.id, task, wait, context), child: child.id };
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
            if (!meta.deleted && parentOf(record, record.owner ? undefined : meta) === scope.id) children.push(record);
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
