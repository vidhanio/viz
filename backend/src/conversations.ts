import type { Context } from "@earendil-works/chord/context";
import {
  type ConversationId,
  type ConversationRecord,
  type Cursor,
  type Harness,
  LiveDoc,
  type Tx,
} from "@earendil-works/pi-durable";

import { ChatDoc } from "./agents/chat.ts";
import { AgentDoc, TurnEntry } from "./agents/model.ts";
import { access, accessOption, AcpStateDoc } from "./agents/options.ts";
import type { Runners } from "./agents/runners.ts";
import type { AgentSessions } from "./agents/sessions.ts";
import { conversationRecords, MetaDoc, ProjectsDoc } from "./projects.ts";
import type { ConfigOption, ConversationState, ConversationSummary } from "./protocol.ts";
import { ORCHESTRATOR_TITLE, TitleDoc } from "./titles.ts";

export function parentOf(
  record: ConversationRecord,
  meta: { parent: number | null } | undefined,
): ConversationId | null {
  return record.owner?.conversationId ?? (meta?.parent as ConversationId | undefined) ?? null;
}

export async function moveConversation(tx: Tx, id: ConversationId, project: string | null, cwd: string) {
  const meta = await tx.doc(MetaDoc, id);
  if (meta.project === project) return;
  const chat = await tx.doc(ChatDoc, id);
  if (chat.runner && chat.runner.cwd !== cwd) {
    if (
      chat.hasWorker ||
      chat.dispatched ||
      chat.queue.length ||
      (await tx.doc(AgentDoc, id)).outcome?.kind === "running"
    ) {
      throw new Error("Stop the conversation before moving it");
    }
    chat.runner.cwd = cwd;
    delete chat.setupFailed;
  }
  meta.project = project;
}

export async function* descendants(tx: Tx, conversation: ConversationId) {
  // Parent IDs always precede their children, so one scan handles both ownership kinds.
  const scope = new Set([conversation]);
  for await (const record of conversationRecords(tx)) {
    if (record.id <= conversation) continue;
    const parent = parentOf(record, record.owner ? undefined : await tx.doc(MetaDoc, record.id));
    if (parent !== null && scope.has(parent)) {
      scope.add(record.id);
      yield { record, parent };
    }
  }
}

async function failed(
  harness: Harness,
  conversationId: ConversationId,
  context: Context,
): Promise<boolean | undefined> {
  let cursor: Cursor | undefined;
  do {
    const page = await harness.commit((tx) => tx.scanEntries({ conversationId }, 32, cursor), context);
    const turn = page.items.find(TurnEntry.is);
    if (turn) return turn.data.error != null;
    cursor = page.next;
  } while (cursor !== undefined);
  return undefined;
}

export class Conversations {
  constructor(
    private readonly harness: Harness,
    private readonly root: ConversationId,
    private readonly cwd: string,
    private readonly runners: Runners,
    private readonly sessions: AgentSessions,
    private readonly rootOptions: (context: Context) => Promise<ConfigOption[]>,
  ) {}

  async list(context: Context): Promise<ConversationSummary[]> {
    const records = await this.harness.commit(async (tx) => {
      const records: ConversationRecord[] = [];
      for await (const record of conversationRecords(tx)) records.push(record);
      return records;
    }, context);
    // Bound concurrent storage reads regardless of the number of conversations.
    const summaries: ConversationSummary[] = [];
    for (const record of records) summaries.push(await this.summary(record, context));
    return summaries;
  }

  async #info(record: ConversationRecord, context: Context) {
    const { harness, root, sessions, cwd } = this;
    const { id } = record;
    const approvals = sessions.approvals.list(id);
    const meta = await harness.snapshot(MetaDoc, id, context);
    const parent = parentOf(record, meta);
    if (id === root) {
      const live = await harness.snapshot(LiveDoc, root, context);
      const status = live?.run
        ? "working"
        : live?.runFailure && !["aborted", "reset"].includes(live.runFailure.reason)
          ? "failed"
          : "idle";
      return { status, meta, parent, harness: null, cwd, approvals, readOnly: false } as const;
    }
    const [chat, agent] = await Promise.all([
      harness.snapshot(ChatDoc, id, context),
      record.owner ? harness.snapshot(AgentDoc, id, context) : undefined,
    ]);
    const name = chat?.runner?.name ?? (agent?.harness || null);
    const readOnly = agent?.outcome?.kind === "running";
    const status = approvals.length
      ? "approval"
      : readOnly || chat?.hasWorker || chat?.dispatched || chat?.queue.length
        ? "working"
        : chat?.setupFailed ||
            ((chat || agent) && ((await failed(harness, id, context)) ?? agent?.outcome?.kind === "failed"))
          ? "failed"
          : "idle";
    return { status, meta, parent, harness: name, cwd: chat?.runner?.cwd, approvals, readOnly } as const;
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
      archived: info.meta?.archived ?? false,
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
      cwd: info.cwd ?? (await this.#projectCwd(info.meta?.project, context)),
      approvals: info.approvals,
      readOnly: info.readOnly,
    };
    if (id === this.root) return { ...base, commands: [], options: await this.rootOptions(context) };
    const acp = await this.harness.snapshot(AcpStateDoc, id, context);
    return {
      ...base,
      commands: acp?.commands ?? [],
      options: [
        ...(acp?.options ?? []),
        accessOption(access(acp?.desired.access, this.runners.get(info.harness ?? undefined))),
      ],
    };
  }

  async cwdFor(id: ConversationId, context: Context): Promise<string> {
    if (id === this.root) return this.cwd;
    const chat = await this.harness.snapshot(ChatDoc, id, context);
    if (chat?.runner) return chat.runner.cwd;
    const meta = await this.harness.snapshot(MetaDoc, id, context);
    return this.#projectCwd(meta?.project, context);
  }

  async #projectCwd(project: string | null | undefined, context: Context): Promise<string> {
    if (!project) return this.cwd;
    const projects = await this.harness.snapshot(ProjectsDoc, this.root, context);
    return projects?.projects.find(({ id }) => id === project)?.path ?? this.cwd;
  }
}
