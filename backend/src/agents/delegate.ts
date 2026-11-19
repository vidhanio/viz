import { type Context, withoutAbortSignal } from "@earendil-works/chord/context";
import type { ConversationId, Harness, ToolExecutionApi, Tx } from "@earendil-works/pi-durable";

import type { Harness as HarnessConfig } from "../config.ts";
import { touch } from "../projects.ts";
import { TitleDoc, titleFrom } from "../titles.ts";
import { ChatDoc } from "./chat.ts";
import { AgentDoc, PromptEntry, TurnEntry } from "./model.ts";
import { AcpStateDoc, initialDesired } from "./options.ts";
import type { AgentSessions, TurnOutcome } from "./sessions.ts";

export async function delegate(
  api: ToolExecutionApi,
  context: Context,
  options: {
    select: () => { name: string; config: HarnessConfig };
    task: string;
    cwd: string;
    sessions: AgentSessions;
    harness: Harness;
  },
): Promise<{ text: string; conversationId: ConversationId; isError: boolean }> {
  const { select, task, cwd, sessions, harness } = options;
  const { id, runner, promptEntry } = await api.commit(async (tx) => {
    const existing = (await tx.scanConversations({ ownerTaskId: api.taskId }, 1)).items[0];
    if (existing) return { id: existing.id, runner: undefined, promptEntry: undefined };
    const runner = select();
    const child = await tx.createConversation({ ownership: { kind: "task", taskId: api.taskId } });
    (await tx.doc(TitleDoc, child.id)).title = titleFrom(task);
    (await tx.doc(ChatDoc, child.id)).runner = { name: runner.name, cwd };
    (await tx.doc(AcpStateDoc, child.id)).desired = initialDesired(runner.config);
    await touch(tx, child.id);
    const state = await tx.doc(AgentDoc, child.id);
    state.harness = runner.name;
    state.outcome = { kind: "running" };
    const prompt = await tx.appendEntry(PromptEntry, child.id, { data: { text: task } });
    return { id: child.id, runner, promptEntry: prompt.id };
  }, context);

  const cleanup = withoutAbortSignal(context);
  // Cancelling the parent task disallows its run-mode commits; the child still needs a terminal record.
  const commit = (write: (tx: Tx) => Promise<void>) => harness.commit(write, cleanup);
  let outcome: TurnOutcome;
  try {
    await api.details({ conversationId: id }, context);
    if (runner === undefined) {
      const state = await harness.snapshot(AgentDoc, id, context);
      if (!state?.harness) {
        return result(id, { kind: "failed", error: "This legacy native subagent cannot be replayed" });
      }
      const previous = state.outcome;
      if (previous?.kind === "done" || previous?.kind === "failed") return result(id, previous);
      // ACP prompts have no idempotency key. An interrupted delegation cannot be replayed safely.
      outcome = { kind: "failed", error: "Interrupted ACP turn; prompt not resent" };
    } else
      outcome = await sessions.turn({
        id,
        ...runner,
        cwd,
        delegation: true,
        text: task,
        promptEntry,
        commit,
        context,
        progress: (text) => api.output(`${text}\n`),
      });
  } catch (error) {
    outcome = context.abortSignal?.aborted
      ? { kind: "done", answer: "", stopReason: "cancelled" }
      : { kind: "failed", error: error instanceof Error ? error.message : String(error) };
  } finally {
    if (runner) sessions.forget(id);
  }

  await commit(async (tx) => {
    const state = await tx.doc(AgentDoc, id);
    if (state.outcome?.kind !== "running") {
      if (state.outcome) outcome = state.outcome;
      return;
    }
    state.outcome = outcome;
    await touch(tx, id);
    await tx.appendEntry(TurnEntry, id, {
      data:
        outcome.kind === "done"
          ? { stopReason: outcome.stopReason, error: null, answer: outcome.answer }
          : { stopReason: "error", error: outcome.error },
    });
  });
  sessions.delegationEnded(id);
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
