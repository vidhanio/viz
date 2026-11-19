import { BACKGROUND_CONTEXT, type Context } from "@earendil-works/chord/context";
import { type ConversationId, type Harness, InboxDoc, LiveDoc } from "@earendil-works/pi-durable";

import { ChatDoc } from "./agents/chat.ts";
import { TurnEntry } from "./agents/model.ts";
import { AcpStateDoc } from "./agents/options.ts";
import { SettingsDoc } from "./orchestrator.ts";
import { MetaDoc, ProjectsDoc, touch } from "./projects.ts";
import { TitleDoc } from "./titles.ts";

export type Changes = { list: boolean; state: ReadonlySet<ConversationId> };
export type ChangeFeed = {
  subscribe(listener: (changes: Changes) => void): () => void;
  refresh(id: ConversationId, relist?: boolean): void;
  watched(id: ConversationId, active: boolean): void;
  markRead(id: ConversationId, context: Context): Promise<void>;
  clearPendingUnread(id: ConversationId): void;
  stop(): Promise<void>;
};

export async function observeChanges(harness: Harness, root: ConversationId, context: Context): Promise<ChangeFeed> {
  const live = await harness.snapshot(LiveDoc, root, context);
  let running = !!live?.run;
  let failure: string | undefined = live?.runFailure?.reason;
  let timer: Timer | undefined;
  let list = false;
  let state = new Set<ConversationId>();
  let rootEnded = false;
  const listeners = new Set<(changes: Changes) => void>();
  const appWatches = new Map<ConversationId, number>();
  const unread = new Set<ConversationId>();
  const { tasks } = await harness.inspect(context);
  const workingChats = new Set(
    tasks.flatMap(({ record }) => (record.kind === ChatDoc.definition.kind ? [record.conversationId] : [])),
  );

  function ended(id: ConversationId) {
    if (!appWatches.has(id)) unread.add(id);
    schedule(id, true);
  }

  function schedule(id?: ConversationId, relist = false) {
    if (id !== undefined) state.add(id);
    list ||= relist;
    timer ??= setTimeout(
      () => void flush().catch((error: unknown) => console.error("Conversation changes:", error)),
      100,
    );
  }

  async function flush() {
    while (rootEnded || unread.size) {
      const touchRoot = rootEnded;
      rootEnded = false;
      const pending = Array.from(unread);
      await harness
        .commit(async (tx) => {
          if (touchRoot) await touch(tx, root);
          for (const id of pending) {
            if (!unread.delete(id)) continue;
            if (appWatches.has(id)) continue;
            const meta = await tx.doc(MetaDoc, id);
            if (!meta.deleted) meta.unread = true;
          }
        }, BACKGROUND_CONTEXT)
        .catch((error: unknown) => {
          for (const id of pending) unread.delete(id);
          console.error("Conversation turn state:", error);
        });
    }
    timer = undefined;
    const changes = { list, state };
    list = false;
    state = new Set();
    for (const listener of listeners) listener(changes);
  }

  const observation = harness.observeCommits({
    advance({ changes }) {
      for (const change of changes) {
        if (change.type === "conversation") schedule(change.value.id, true);
        else if (change.type === "entry" && change.value.kind === TurnEntry.kind)
          schedule(change.value.conversationId, true);
        else if (change.type === "document") {
          switch (change.record.kind) {
            case ProjectsDoc.definition.kind:
              schedule(undefined, true);
              break;
            case TitleDoc.definition.kind:
            case MetaDoc.definition.kind:
              schedule(change.conversationId, true);
              break;
            case ChatDoc.definition.kind: {
              const id = change.conversationId;
              if (id !== undefined) {
                if (change.kind === "change" && change.value.hasWorker === true) workingChats.add(id);
                else if (workingChats.delete(id)) ended(id);
              }
              schedule(id, true);
              break;
            }
            case InboxDoc.definition.kind:
            case SettingsDoc.definition.kind:
            case AcpStateDoc.definition.kind:
              schedule(change.conversationId);
              break;
            case LiveDoc.definition.kind: {
              if (change.conversationId !== root) break;
              const next = change.kind === "change" && change.value.run !== undefined;
              const runFailure =
                change.kind === "change" ? (change.value.runFailure as { reason?: string } | undefined) : undefined;
              if (running && !next) {
                rootEnded = true;
                ended(root);
              }
              if (running !== next || failure !== runFailure?.reason) schedule(root, true);
              running = next;
              failure = runFailure?.reason;
              break;
            }
          }
        }
      }
    },
    end() {
      clearTimeout(timer);
    },
  });
  return {
    subscribe(listener: (changes: Changes) => void) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    refresh(id, relist) {
      schedule(id, relist);
    },
    watched(id, active) {
      const count = (appWatches.get(id) ?? 0) + (active ? 1 : -1);
      if (count > 0) appWatches.set(id, count);
      else appWatches.delete(id);
      if (active) unread.delete(id);
    },
    async markRead(id, context) {
      unread.delete(id);
      await harness.commit(async (tx) => {
        const meta = await tx.doc(MetaDoc, id);
        if (meta.unread) meta.unread = false;
      }, context);
    },
    clearPendingUnread(id) {
      unread.delete(id);
    },
    async stop() {
      observation.detach();
      clearTimeout(timer);
      listeners.clear();
      if (rootEnded || unread.size) await flush();
    },
  };
}
