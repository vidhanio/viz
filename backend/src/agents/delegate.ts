import { type Context, withoutAbortSignal } from "@earendil-works/chord/context";
import type { ConversationId, ToolExecutionApi, Tx } from "@earendil-works/pi-durable";

import type { Harness as HarnessConfig } from "../config.ts";
import { TitleDoc, titleFrom } from "../titles.ts";
import { AgentDoc, PromptEntry, SessionDoc, TurnEntry } from "./model.ts";
import type { AgentSessions, TurnOutcome } from "./sessions.ts";

export async function delegate(
  api: ToolExecutionApi,
  context: Context,
  options: {
    select: () => { name: string; config: HarnessConfig };
    task: string;
    cwd: string;
    sessions: AgentSessions;
  },
): Promise<{ text: string; conversationId: ConversationId; isError: boolean }> {
  const { select, task, cwd, sessions } = options;
  const { id, runner } = await api.commit(async (tx) => {
    const existing = (await tx.scanConversations({ ownerTaskId: api.taskId }, 1)).items[0];
    if (existing) return { id: existing.id, runner: undefined };
    const runner = select();
    const child = await tx.createConversation({ ownership: { kind: "task", taskId: api.taskId } });
    (await tx.doc(TitleDoc, child.id)).title = titleFrom(task);
    const state = await tx.doc(AgentDoc, child.id);
    state.harness = runner.name;
    state.outcome = { kind: "running" };
    await tx.appendEntry(PromptEntry, child.id, { data: { text: task } });
    return { id: child.id, runner };
  }, context);
  await api.details({ conversationId: id }, context);

  const cleanup = withoutAbortSignal(context);
  const commit = (write: (tx: Tx) => Promise<void>) => api.commit(write, cleanup);
  let outcome: TurnOutcome;
  if (runner === undefined) {
    const state = await api.snapshot(AgentDoc, id, context);
    if (!state?.harness) {
      return result(id, { kind: "failed", error: "This legacy native subagent cannot be replayed" });
    }
    const previous = state.outcome;
    if (previous?.kind === "done" || previous?.kind === "failed") return result(id, previous);
    // ACP prompts have no idempotency key. An interrupted delegation cannot be replayed safely.
    outcome = { kind: "failed", error: "viz restarted during the ACP turn; it was not replayed" };
  } else {
    try {
      outcome = await sessions.turn({
        id,
        ...runner,
        cwd,
        sessionId: null,
        text: task,
        commit,
        saveSession: (sessionId) =>
          commit(async (tx) => {
            (await tx.doc(SessionDoc, id)).sessionId = sessionId;
          }),
        context,
        progress: (text) => api.output(`${text}\n`),
      });
    } finally {
      sessions.forget(id);
    }
  }

  await commit(async (tx) => {
    (await tx.doc(AgentDoc, id)).outcome = outcome;
    await tx.appendEntry(TurnEntry, id, {
      data:
        outcome.kind === "done"
          ? { stopReason: outcome.stopReason, error: null }
          : { stopReason: "error", error: outcome.error },
    });
  });
  return result(id, outcome);
}

function result(conversationId: ConversationId, outcome: TurnOutcome) {
  if (outcome.kind === "failed") return { conversationId, isError: true, text: outcome.error };
  const { stopReason, answer } = outcome;
  return {
    conversationId,
    isError: stopReason === "refusal" || stopReason === "cancelled",
    text: stopReason === "end_turn" ? answer : `[ACP turn ended: ${stopReason}]\n${answer}`,
  };
}
