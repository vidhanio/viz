import { BACKGROUND_CONTEXT, type Context } from "@earendil-works/chord/context";
import { type ConversationId, type Harness, LiveDoc } from "@earendil-works/pi-durable";

import type { Approvals } from "./agents/approvals.ts";
import { ChatDoc } from "./agents/chat.ts";
import { AgentDoc, TurnEntry } from "./agents/model.ts";
import { AcpStateDoc } from "./agents/options.ts";
import { SettingsDoc } from "./orchestrator.ts";
import { MetaDoc, ProjectsDoc, touch } from "./projects.ts";
import { TitleDoc } from "./titles.ts";

export type Changes = { list: boolean; state: ReadonlySet<ConversationId> };
export type ChangeFeed = {
  subscribe(listener: (changes: Changes) => void): () => void;
  stop(): Promise<void>;
};

export async function observeChanges(
  harness: Harness,
  root: ConversationId,
  approvals: Approvals,
  context: Context,
): Promise<ChangeFeed> {
  const live = await harness.snapshot(LiveDoc, root, context);
  let running = !!live?.run;
  let failure: string | undefined = live?.runFailure?.reason;
  let timer: Timer | undefined;
  let list = false;
  let state = new Set<ConversationId>();
  let rootEnded = false;
  const listeners = new Set<(changes: Changes) => void>();

  function schedule(id?: ConversationId, relist = false) {
    if (id !== undefined) state.add(id);
    list ||= relist;
    timer ??= setTimeout(
      () => void flush().catch((error: unknown) => console.error("Conversation changes:", error)),
      100,
    );
  }

  async function flush() {
    while (rootEnded) {
      rootEnded = false;
      await harness
        .commit((tx) => touch(tx, root), BACKGROUND_CONTEXT)
        .catch((error: unknown) => console.error("Root turn state:", error));
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
            case ChatDoc.definition.kind:
            case AgentDoc.definition.kind:
              schedule(change.conversationId, true);
              break;
            case SettingsDoc.definition.kind:
            case AcpStateDoc.definition.kind:
              schedule(change.conversationId);
              break;
            case LiveDoc.definition.kind: {
              if (change.conversationId !== root) break;
              const next = change.kind === "change" && change.value.run !== undefined;
              const runFailure =
                change.kind === "change" ? (change.value.runFailure as { reason?: string } | undefined) : undefined;
              if (running && !next) rootEnded = true;
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
  const stopApprovals = approvals.observe((id) => schedule(id, true));
  return {
    subscribe(listener: (changes: Changes) => void) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    async stop() {
      observation.detach();
      stopApprovals();
      clearTimeout(timer);
      listeners.clear();
      if (rootEnded) await harness.commit((tx) => touch(tx, root), BACKGROUND_CONTEXT);
    },
  };
}
