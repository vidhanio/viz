import type { Context } from "@earendil-works/chord/context";
import {
  type Conversation,
  type ConversationId,
  configure,
  defineDoc,
  defineExtension,
  defineTask,
  type Harness,
  type SubmissionId,
} from "@earendil-works/pi-durable";

import type { Origin } from "../protocol.ts";
import { TitleDoc, titleFrom } from "../titles.ts";
import { PromptEntry, SessionDoc, TurnEntry } from "./model.ts";
import type { Runners } from "./runners.ts";
import type { AgentSessions, TurnOutcome } from "./sessions.ts";

export type ChatInput = {
  readonly text: string;
  readonly content: string;
  readonly origin?: Origin;
  readonly requestId?: string;
};

type Pending = Omit<ChatInput, "requestId"> & { submission: SubmissionId };
type Phase = { phase: "next" } | { phase: "prompt"; item: Pending };
type ChatState = {
  runner: { name: string; cwd: string } | null;
  queue: Pending[];
  hasWorker: boolean;
  dispatched: boolean;
};

const ChatDoc = defineDoc<ChatState>({
  kind: "viz.acp.chat",
  version: 1,
  scope: "conversation",
  history: "latest",
  fork: "initial",
  initial: () => ({ runner: null, queue: [], hasWorker: false, dispatched: false }),
  checkpointWhen: (_value, _ops, { deltasSinceBase }) => deltasSinceBase >= 32,
});

const completed = { status: "terminal", outcome: { status: "completed", result: null } } as const;

export function chatScheduler(runners: Runners, sessions: AgentSessions, cwd: string) {
  const Worker = defineTask<null, Phase, null>({
    name: "viz.acp.chat",
    version: 1,
    initial: () => ({ phase: "next" }),
    phases: {
      next: (_task, runtime, context) =>
        runtime.commit(async (tx) => {
          const chat = await tx.doc(ChatDoc, runtime.conversationId);
          const item = chat.queue.shift();
          if (item === undefined) {
            chat.hasWorker = false;
            return completed;
          }
          const prompt = await tx.appendEntry(PromptEntry, runtime.conversationId, {
            data: { text: item.text, ...(item.origin && { origin: item.origin }) },
          });
          tx.placeSubmission(item.submission, prompt.id);
          chat.dispatched = false;
          return { status: "running", checkpoint: { phase: "prompt", item } };
        }, context),
      prompt: async (task, runtime, context) => {
        const id = runtime.conversationId;
        const { item } = task.state.checkpoint;
        const claim: { runner?: { name: string; cwd: string; sessionId: string | null } } = {};
        let outcome: TurnOutcome;
        try {
          // Claim before making external calls. A rerun may recover the queue but must never resend a claimed prompt.
          await runtime.commit(async (tx) => {
            const chat = await tx.doc(ChatDoc, id);
            const runner = chat.runner;
            if (runner === null) throw new Error("ACP chat has no runner");
            if (chat.dispatched) return;
            const { sessionId } = await tx.doc(SessionDoc, id);
            chat.dispatched = true;
            claim.runner = { name: runner.name, cwd: runner.cwd, sessionId };
          }, context);
          const active = claim.runner;
          if (active === undefined) {
            sessions.forget(id);
            outcome = { kind: "failed", error: "viz was interrupted during the ACP turn; the prompt was not resent" };
          } else {
            const { name, cwd, sessionId } = active;
            const { config } = runners.require(name);
            outcome = await sessions.turn({
              id,
              name,
              config,
              cwd,
              sessionId,
              text: item.content,
              context,
              commit: (write) =>
                runtime.commit(async (tx) => {
                  await write(tx);
                }, context),
              saveSession: (sessionId) =>
                runtime.commit(async (tx) => {
                  (await tx.doc(SessionDoc, id)).sessionId = sessionId;
                }, context),
            });
          }
        } catch (error) {
          runtime.signal.throwIfAborted();
          outcome = { kind: "failed", error: error instanceof Error ? error.message : String(error) };
        }
        await runtime.commit(async (tx) => {
          const chat = await tx.doc(ChatDoc, id);
          chat.dispatched = false;
          const answer = await tx.appendEntry(TurnEntry, id, {
            data:
              outcome.kind === "done"
                ? { stopReason: outcome.stopReason, error: null, answer: outcome.answer }
                : { stopReason: "error", error: outcome.error },
          });
          const successful =
            outcome.kind === "done" && outcome.stopReason !== "cancelled" && outcome.stopReason !== "refusal";
          tx.settleSubmission(
            item.submission,
            successful
              ? { status: "done", answer: answer.id }
              : {
                  status: "unanswered",
                  reason: outcome.kind === "done" ? outcome.stopReason : "acp_error",
                },
          );
          return { status: "running", checkpoint: { phase: "next" } };
        }, context);
      },
    },
    abort: async (task, runtime, context) => {
      const id = runtime.conversationId;
      sessions.forget(id);
      await runtime.commit(async (tx) => {
        const chat = await tx.doc(ChatDoc, id);
        for (const { submission } of chat.queue)
          tx.settleSubmission(submission, { status: "unanswered", reason: "aborted" });
        chat.queue = [];
        chat.hasWorker = false;
        chat.dispatched = false;
        const checkpoint = task.state.checkpoint;
        if (checkpoint.phase === "prompt") {
          await tx.appendEntry(TurnEntry, id, { data: { stopReason: "cancelled", error: null } });
          tx.settleSubmission(checkpoint.item.submission, { status: "unanswered", reason: "aborted" });
        }
        return { status: "terminal", outcome: { status: "aborted" } };
      }, context);
    },
  });

  return {
    extension: defineExtension({ name: "viz.acp.chat", tasks: [Worker] }),
    create(harness: Harness, context: Context): Promise<Conversation> {
      const { name } = runners.require();
      return harness.createConversation(
        {
          ownership: { kind: "ownerless" },
          init: async (tx, id) => {
            (await tx.doc(ChatDoc, id)).runner = { name, cwd };
          },
        },
        context,
      );
    },
    async submit(harness: Harness, id: ConversationId, input: ChatInput, context: Context) {
      const { requestId, ...prompt } = input;
      const submission = await harness.commit(async (tx) => {
        const conversation = await tx.conversation(id);
        if (!conversation || conversation.owner) throw new Error("delegation transcripts are read-only");
        if (requestId !== undefined) {
          const existing = await tx.submissionByRequest(id, requestId);
          if (existing) return existing.id;
        }
        const chat = await tx.doc(ChatDoc, id);
        if (chat.runner === null) {
          // Existing native app chats keep their transcript, but never make another native model request.
          const { name } = runners.require();
          chat.runner = { name, cwd };
          await configure(tx, id, {
            model: null,
            thinkingLevel: null,
            extensions: null,
            tools: null,
            instructions: null,
          });
        } else runners.require(chat.runner.name);
        const title = await tx.doc(TitleDoc, id);
        if (title.title === "") title.title = titleFrom(prompt.text);
        const queued = await tx.createSubmission({
          type: "input",
          conversationId: id,
          status: "queued",
          ...(requestId !== undefined && { requestId }),
        });
        chat.queue.push({ ...prompt, submission: queued.id });
        if (!chat.hasWorker) {
          await tx.createTask(Worker, null, { conversationId: id, ownership: { kind: "conversation" } });
          chat.hasWorker = true;
        }
        return queued.id;
      }, context);
      return (await harness.submission(submission, context))!;
    },
  };
}
