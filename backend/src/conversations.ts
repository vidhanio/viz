import type { Context } from "@earendil-works/chord/context";
import {
  type ConversationId,
  type ConversationRecord,
  type Cursor,
  type Harness,
  InboxDoc,
  LiveDoc,
  type Tx,
} from "@earendil-works/pi-durable";

import { ChatDoc } from "./agents/chat.ts";
import { TurnEntry } from "./agents/model.ts";
import { AcpStateDoc, HarnessOptionsDoc, withModelProfiles } from "./agents/options.ts";
import { conversationRecords, MetaDoc } from "./projects.ts";
import type { ConfigOption, ConversationState, ConversationSummary } from "./protocol.ts";
import { ORCHESTRATOR_TITLE, TitleDoc } from "./titles.ts";

export async function moveConversation(tx: Tx, id: ConversationId, project: string | null, cwd: string) {
  const meta = await tx.doc(MetaDoc, id);
  if (meta.project === project) return false;
  meta.project = project;
  const chat = await tx.doc(ChatDoc, id);
  if (chat.runner && chat.runner.cwd !== cwd) {
    if (chat.hasWorker || chat.dispatched || chat.queue.length) {
      throw new Error("Stop the conversation before moving it");
    }
    chat.runner.cwd = cwd;
    delete chat.setupFailed;
  }
  return chat.runner !== null;
}

export async function* descendants(tx: Tx, conversation: ConversationId) {
  // Parent IDs always precede their children.
  const scope = new Set([conversation]);
  for await (const record of conversationRecords(tx)) {
    if (record.id <= conversation) continue;
    const { parent } = await tx.doc(MetaDoc, record.id);
    if (parent !== null && scope.has(parent)) {
      scope.add(record.id);
      yield { record, parent };
    }
  }
}

async function failed(harness: Harness, conversationId: ConversationId, context: Context): Promise<boolean> {
  let cursor: Cursor | undefined;
  do {
    const page = await harness.commit((tx) => tx.scanEntries({ conversationId }, 32, cursor), context);
    const turn = page.items.find(TurnEntry.is);
    if (turn) return turn.data.error != null;
    cursor = page.next;
  } while (cursor !== undefined);
  return false;
}

export class Conversations {
  constructor(
    private readonly harness: Harness,
    private readonly root: ConversationId,
    private readonly cwd: string,
    private readonly rootOptions: (context: Context) => Promise<ConfigOption[]>,
    private readonly rootImages: (context: Context) => Promise<boolean>,
  ) {}

  async list(context: Context, scope: "all" | "top-level" = "all"): Promise<ConversationSummary[]> {
    const records = await this.harness.commit(async (tx) => {
      const records: ConversationRecord[] = [];
      for await (const record of conversationRecords(tx)) {
        const meta = await tx.doc(MetaDoc, record.id);
        if (meta.deleted) continue;
        if (scope === "top-level" && (record.id === this.root || meta.parent !== null)) continue;
        records.push(record);
      }
      return records;
    }, context);
    // Bound concurrent storage reads regardless of the number of conversations.
    const summaries: ConversationSummary[] = [];
    for (const record of records) summaries.push(await this.summary(record, context));
    return summaries;
  }

  async #info(record: ConversationRecord, context: Context) {
    const { harness, root, cwd } = this;
    const { id } = record;
    const meta = await harness.snapshot(MetaDoc, id, context);
    const parent = meta?.parent ?? null;
    if (id === root) {
      const live = await harness.snapshot(LiveDoc, root, context);
      const status = live?.run
        ? "working"
        : live?.runFailure && !["aborted", "reset"].includes(live.runFailure.reason)
          ? "failed"
          : "idle";
      return { status, meta, parent, harness: null, cwd } as const;
    }
    const chat = await harness.snapshot(ChatDoc, id, context);
    if (!chat?.runner) throw new Error("ACP chat has no runner");
    const status =
      chat.hasWorker || chat.dispatched || chat.queue.length
        ? "working"
        : chat.setupFailed || (await failed(harness, id, context))
          ? "failed"
          : "idle";
    return { status, meta, parent, harness: chat.runner.name, cwd: chat.runner.cwd } as const;
  }

  async summary(record: ConversationRecord, context: Context): Promise<ConversationSummary> {
    const info = await this.#info(record, context);
    const title =
      record.id === this.root
        ? ORCHESTRATOR_TITLE
        : ((await this.harness.snapshot(TitleDoc, record.id, context))?.title ?? "");
    return {
      id: record.id,
      title,
      kind: record.id === this.root ? "orchestrator" : info.parent !== null ? "subagent" : "chat",
      ...(info.parent !== null && { parent: info.parent }),
      ...(info.meta?.project && { project: info.meta.project }),
      ...(info.harness !== null && { harness: info.harness }),
      status: info.status,
      // The orchestrator is not a thread the user manages: never archived or pinned.
      archived: (record.id !== this.root && info.meta?.archived) || false,
      pinned: (record.id !== this.root && info.meta?.pinned) || false,
      unread: info.meta?.unread ?? false,
      updatedAt: info.meta?.updatedAt ?? 0,
    };
  }

  async state(id: ConversationId, context: Context): Promise<ConversationState> {
    const record = await this.harness.commit((tx) => tx.conversation(id), context);
    if (!record) throw new Error("Conversation not found");
    const info = await this.#info(record, context);
    const base = {
      status: info.status,
      harness: info.harness,
      project: info.meta?.project ?? null,
      cwd: info.cwd,
    };
    if (id === this.root) {
      const [options, images, inbox] = await Promise.all([
        this.rootOptions(context),
        this.rootImages(context),
        this.harness.snapshot(InboxDoc, id, context),
      ]);
      const queue = (inbox?.items ?? []).flatMap((item) => {
        if (item.mode === "write") return [];
        const { data } = item.entry ?? {};
        const text =
          data && typeof data === "object" && !Array.isArray(data) && typeof data.text === "string"
            ? data.text
            : typeof item.content === "string"
              ? item.content
              : item.content.flatMap((block) => (block.type === "text" ? [block.text] : [])).join("");
        return [{ submission: item.id, text }];
      });
      return { ...base, commands: [], options, queue, attachments: { images } };
    }
    const [acp, chat, catalog] = await Promise.all([
      this.harness.snapshot(AcpStateDoc, id, context),
      this.harness.snapshot(ChatDoc, id, context),
      this.harness.snapshot(HarnessOptionsDoc, this.root, context),
    ]);
    const profiles = catalog?.harnesses.find(({ name }) => name === info.harness)?.profiles ?? [];
    return {
      ...base,
      attachments: { images: acp?.images ?? false },
      queue: chat?.queue.map(({ submission, text }) => ({ submission, text })) ?? [],
      commands: acp?.commands ?? [],
      options: withModelProfiles(acp?.options ?? [], profiles),
    };
  }

  async cwdFor(id: ConversationId, context: Context): Promise<string> {
    if (id === this.root) return this.cwd;
    const chat = await this.harness.snapshot(ChatDoc, id, context);
    if (!chat?.runner) throw new Error("ACP chat has no runner");
    return chat.runner.cwd;
  }
}
