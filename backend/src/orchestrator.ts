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

const ORCHESTRATOR_PROMPT = `You are viz, the user's personal agent. This is your main thread: it never ends, and every
connector (Discord, iMessage, ...) feeds into it. Inputs from a connector start with a bracketed origin tag, e.g.
"[discord #general @alice]"; your answer is delivered back to that origin automatically, so answer as if replying there.
Delegate self-contained work to subagents instead of doing long tasks inline, so this thread stays responsive.`;

export async function answerText(api: ToolExecutionApi, answer: EntryId, context: Context): Promise<string> {
  const entry = await api.commit((tx) => tx.entry(AssistantEntry, answer), context);
  const message = entry?.model?.[0] as AssistantMessage | undefined;
  return message?.content.flatMap((c) => (c.type === "text" ? [c.text] : [])).join("") ?? "";
}

export function subagentExtension(model: ModelRef | undefined): Extension {
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
            await configure(tx, created.id, {
              extensions: { remove: [Subagent, Orchestrator] },
              ...(model && { model }),
            });
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
  sections: [section("viz", () => ORCHESTRATOR_PROMPT)],
});
