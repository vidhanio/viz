/**
 * Feeds the orchestrator's pi-durable transcript into its memory, as uniichat logs a chat's messages as they come.
 *
 * The transcript is the source of truth; the memory's messages are derived from its entries in order, with the cursor
 * stored in the same transaction, so a crash re-derives nothing twice and loses nothing. Mapping (uniichat
 * `Anthropic.entries` / `Server.on`): a user message is `user`; an answer's text is `viz` (thinking is never logged); a
 * tool call is logged with its result, as `tool` then `echo`. `pi.reset` entries mark where a turn's context starts
 * (`turn`); system, compaction and bookkeeping entries carry nothing to remember.
 */
import type { Context } from "@earendil-works/chord/context";
import type { Message } from "@earendil-works/pi-ai";
import type { Conversation, EntryId, EntryRecord } from "@earendil-works/pi-durable";

import type { Memory, Msg } from "./memory.ts";

type State = {
  /** The last entry derived. */
  cursor: number;
  /** Tool calls awaiting their results, by call id. */
  pending: Record<string, string>;
  /** The message the current turn's context starts at: the count when the last `pi.reset` was derived. */
  turn: number;
};

const PAGE = 200;
const SKIP: Record<string, true> = { "pi.system": true, "pi.compaction": true, "pi.reset": true };

export class Sync {
  readonly #memory: Memory;
  readonly #root: Conversation;
  readonly #context: Context;
  readonly #state: State;
  #pass: Promise<void> = Promise.resolve();
  #queued: Promise<void> | null = null;

  constructor(memory: Memory, root: Conversation, context: Context) {
    this.#memory = memory;
    this.#root = root;
    this.#context = context;
    const saved = memory.meta("sync");
    this.#state = saved === null ? { cursor: 0, pending: {}, turn: 0 } : (JSON.parse(saved) as State);
  }

  /** Where the current turn's context starts; messages before it reach a turn only through the view. */
  get turn(): number {
    return this.#state.turn;
  }

  /** Derives the entries committed so far; calls while a pass is queued share it. */
  run(): Promise<void> {
    // A failed pass rejects its callers, not the passes after it.
    this.#queued ??= this.#pass = this.#pass
      .catch(() => {})
      .then(() => {
        this.#queued = null;
        return this.#derive();
      });
    return this.#queued;
  }

  async #derive(): Promise<void> {
    for (;;) {
      const query = { minEntryId: (this.#state.cursor + 1) as EntryId, order: "ascending" } as const;
      const page = await this.#root.entries(query, PAGE, undefined, this.#context);
      let msgs: Msg[] = [];
      for (const entry of page.items) {
        if (entry.kind === "pi.reset") {
          this.#commit(msgs);
          msgs = [];
          this.#state.turn = this.#memory.count;
        } else if (!SKIP[entry.kind]) {
          msgs.push(...this.#messages(entry));
        }
        this.#state.cursor = entry.id;
      }
      this.#commit(msgs);
      if (page.items.length < PAGE) return;
    }
  }

  #commit(msgs: Msg[]): void {
    this.#memory.push(msgs, ["sync", JSON.stringify(this.#state)]);
  }

  #messages(entry: EntryRecord): Msg[] {
    const out: Msg[] = [];
    const add = (kind: Msg["kind"], text: string, timestamp: number | undefined) => {
      const trimmed = text.trim();
      if (trimmed === "") return;
      const date = new Date(timestamp ?? Date.now()).toISOString();
      out.push({ kind, text: trimmed, date, entry: entry.id });
    };
    for (const message of (entry.model ?? []) as Message[]) {
      switch (message.role) {
        case "user": {
          const content = message.content;
          const text =
            typeof content === "string"
              ? content
              : content.map((block) => (block.type === "text" ? block.text : "[image]")).join("\n");
          add("user", text, message.timestamp);
          break;
        }
        case "assistant": {
          // pi-durable keeps failed answers out of model context; keep them out of memory too.
          if (message.stopReason === "error") break;
          let reply = "";
          for (const block of message.content) {
            if (block.type === "text") reply += block.text;
            if (block.type === "toolCall") {
              this.#state.pending[block.id] = `${block.name} ${JSON.stringify(block.arguments)}`;
            }
          }
          add("viz", reply, message.timestamp);
          break;
        }
        case "toolResult": {
          const call = this.#state.pending[message.toolCallId] ?? message.toolName;
          delete this.#state.pending[message.toolCallId];
          const text = message.content.map((block) => (block.type === "text" ? block.text : "[image]")).join("\n");
          add("tool", call, message.timestamp);
          add("echo", (message.isError ? "error: " : "") + (text.trim() || "(no output)"), message.timestamp);
          break;
        }
        default:
          break;
      }
    }
    return out;
  }
}
