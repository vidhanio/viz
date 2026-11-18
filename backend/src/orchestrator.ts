/**
 * The orchestrator: viz's root conversation and the tools it uses to delegate work.
 *
 * Subagents are ordinary pi-durable conversations owned by the tool call that created them, so they survive crashes,
 * count their own usage, and show up in UIs under the call (`details.conversationId`).
 */
import type { Context } from "@earendil-works/chord/context";
import type { AssistantMessage } from "@earendil-works/pi-ai";
import { obj, str } from "@earendil-works/pi-schema";
import {
  AssistantEntry,
  configure,
  defineExtension,
  defineTool,
  type EntryId,
  type Extension,
  type ModelRef,
  section,
  type ToolExecutionApi,
} from "@earendil-works/pi-durable";

import { TURNS_PROMPT } from "./memory/prompt.ts";
import { TitleDoc, titleFrom } from "./titles.ts";

export async function answerText(api: ToolExecutionApi, answer: EntryId, context: Context): Promise<string> {
  const entry = await api.commit((tx) => tx.entry(AssistantEntry, answer), context);
  const message = entry?.model?.[0] as AssistantMessage | undefined;
  return message?.content.flatMap((c) => (c.type === "text" ? [c.text] : [])).join("") ?? "";
}

/** `rootOnly`: the orchestrator's own extensions (its prompt, its memory), which a subagent doesn't get. */
export function subagentExtension(model: ModelRef, rootOnly: readonly Extension[]): Extension {
  const Subagent: Extension = defineExtension({
    name: "viz.subagent",
    tools: [
      defineTool({
        name: "subagent",
        description: "Delegate a self-contained task to a subagent conversation and get its answer back.",
        parameters: obj({ task: str({ description: "What the subagent should do, with all context it needs" }) }),
        // A rerun after a crash finds the child it created and the submission it made.
        replay: "safe",
        execute: async ({ task }, api, context) => {
          const child = await api.commit(async (tx) => {
            const existing = (await tx.scanConversations({ ownerTaskId: api.taskId }, 1)).items[0];
            if (existing !== undefined) return existing.id;
            const created = await tx.createConversation({ ownership: { kind: "task", taskId: api.taskId } });
            // Starts as a copy of the orchestrator's agent; drop what is the orchestrator's own.
            await configure(tx, created.id, { extensions: { remove: [Subagent, ...rootOnly] }, model });
            (await tx.doc(TitleDoc, created.id)).title = titleFrom(task);
            return created.id;
          }, context);
          await api.details({ conversationId: child }, context);

          const handle = (await api.conversation(child, context))!;
          const request = { type: "input", content: task, requestId: `subagent:${api.taskId}` } as const;
          const settled = await (await handle.submit(request, context)).wait(context);
          if (settled.status !== "done" || settled.type !== "input") {
            throw new Error(`subagent ${child} ended ${settled.status}`);
          }
          const text = await answerText(api, settled.answer, context);
          return { output: [{ type: "text", text }], details: { conversationId: child } };
        },
      }),
    ],
  });
  return Subagent;
}

export const Orchestrator: Extension = defineExtension({
  name: "viz.orchestrator",
  sections: [section("turns", () => TURNS_PROMPT, { tag: false })],
});
