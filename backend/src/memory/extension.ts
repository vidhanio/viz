/**
 * The memory as a pi-durable extension for the orchestrator: uniichat's turn shape on pi-durable's run loop.
 *
 * Each turn starts from a fresh context (`VizHarness.submit` resets the idle root before a new input), so a request's
 * own messages are the turn's: its input, its tool rounds, and messages steered in meanwhile. `beforeRequest` waits
 * until every message before the turn is summarized (uniichat `Memory.settle`), then puts the view of them in front of
 * the turn's first message, as uniichat's turn call is the view followed by "Turn: the user's new message". On
 * Anthropic, `beforeProviderRequest` marks the view's last whole block for the prompt cache, the fourth breakpoint
 * beside pi-ai's system, tools and last-message marks.
 */
import { int, obj, opt } from "@earendil-works/pi-schema";
import {
  type ConversationId,
  defineExtension,
  defineTool,
  type Extension,
  GenerationTask,
  hook,
  section,
  type TaskId,
} from "@earendil-works/pi-durable";

import type { Memory } from "./memory.ts";
import { MEMORY_PROMPT, TURN_CONTINUE, TURN_HEAD } from "./prompt.ts";
import type { Sync } from "./sync.ts";

/** What the extension reaches once the harness is open and its root exists. */
export type MemoryBinding = { sync?: Sync; root?: ConversationId };

type TextBlock = { type: "text"; text: string; cache_control?: unknown };
type AnthropicPayload = {
  system?: string | { cache_control?: unknown }[];
  messages?: { role: string; content: string | { type: string; text?: string; cache_control?: unknown }[] }[];
};

export function memoryExtension(memory: Memory, binding: MemoryBinding): Extension {
  /** The view block to mark for the prompt cache, by generation task. */
  const marks = new Map<TaskId, string>();

  return defineExtension({
    name: "viz.memory",
    sections: [section("memory", () => MEMORY_PROMPT, { tag: false })],
    tools: [
      defineTool({
        name: "zoom",
        description:
          "Open the line id+n of the view into the two lines of n/2 under it; n = 1 gives the message whole. A long text comes in pages from character at, saying where to go on.",
        parameters: obj({
          id: int({ minimum: 0 }),
          n: opt(int({ minimum: 1, description: "Default 1" })),
          at: opt(int({ minimum: 0, description: "Default 0" })),
        }),
        replay: "safe",
        execute: async ({ id, n, at }) => ({ output: [{ type: "text", text: memory.zoom(id, n ?? 1, at ?? 0) }] }),
      }),
      defineTool({
        name: "date",
        description: "The date and time of message id.",
        parameters: obj({ id: int({ minimum: 0 }) }),
        replay: "safe",
        execute: async ({ id }) => ({ output: [{ type: "text", text: memory.date(id) }] }),
      }),
    ],
    hooks: [
      hook(GenerationTask, {
        beforeRequest: async (request, api, context) => {
          const { sync, root } = binding;
          if (sync === undefined || api.conversationId !== root) return undefined;
          await sync.run();
          const from = sync.turn;
          if (!(await memory.settle(from, context.abortSignal))) return undefined;

          const blocks = memory.blocks(from);
          const mark = blocks.find((block) => block.mark)?.text;
          if (mark === undefined) marks.delete(api.taskId);
          else marks.set(api.taskId, mark);
          const view: TextBlock[] = blocks.map((block) => ({ type: "text", text: block.text }));

          const messages = [...request.messages];
          const k = messages.findIndex((message) => message.role !== "system");
          const first = messages[k];
          if (first?.role === "user") {
            const content =
              typeof first.content === "string" ? [{ type: "text" as const, text: first.content }] : first.content;
            messages[k] = { ...first, content: [...view, { type: "text", text: TURN_HEAD }, ...content] };
          } else {
            const turn = {
              role: "user" as const,
              content: [...view, { type: "text" as const, text: TURN_CONTINUE }],
              timestamp: Date.now(),
            };
            messages.splice(k === -1 ? messages.length : k, 0, turn);
          }
          return { messages };
        },
        beforeProviderRequest: async ({ model, payload }, api) => {
          const mark = marks.get(api.taskId);
          if (mark === undefined || model.api !== "anthropic-messages") return undefined;
          const body = payload as AnthropicPayload;
          // Mark only when pi-ai caches at all, with its own marker (its retention included).
          const control = Array.isArray(body.system)
            ? body.system.findLast((b) => b.cache_control)?.cache_control
            : undefined;
          const content = body.messages?.find((message) => message.role === "user")?.content;
          const block = Array.isArray(content) ? content.find((b) => b.type === "text" && b.text === mark) : undefined;
          if (control === undefined || block === undefined) return undefined;
          block.cache_control = control;
          return { payload: body };
        },
        afterResponse: async (_message, api) => {
          marks.delete(api.taskId);
        },
      }),
    ],
  });
}
