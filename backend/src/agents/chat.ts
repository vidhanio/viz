import { BACKGROUND_CONTEXT, type Context } from "@earendil-works/chord/context";
import {
  type Conversation,
  type ConversationId,
  configure,
  defineDoc,
  defineExtension,
  defineTask,
  type Harness,
  ROOT_CONVERSATION_ID,
  type SubmissionId,
} from "@earendil-works/pi-durable";

import type { Harness as HarnessConfig } from "../config.ts";
import { MetaDoc, projectIn, touch } from "../projects.ts";
import type { Origin } from "../protocol.ts";
import { TitleDoc, titleFrom } from "../titles.ts";
import { AgentDoc, PromptEntry, TurnEntry } from "./model.ts";
import { AcpStateDoc, type Desired, initialDesired } from "./options.ts";
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
  setupFailed?: boolean;
};

export const ChatDoc = defineDoc<ChatState>({
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
        const claim: { runner?: { name: string; cwd: string } } = {};
        let outcome: TurnOutcome;
        try {
          // Claim before making external calls. A rerun may recover the queue but must never resend a claimed prompt.
          await runtime.commit(async (tx) => {
            const chat = await tx.doc(ChatDoc, id);
            const runner = chat.runner;
            if (runner === null) throw new Error("ACP chat has no runner");
            if (chat.dispatched) return;
            chat.dispatched = true;
            claim.runner = { name: runner.name, cwd: runner.cwd };
          }, context);
          const active = claim.runner;
          if (active === undefined) {
            sessions.forget(id);
            outcome = { kind: "failed", error: "viz was interrupted during the ACP turn; the prompt was not resent" };
          } else {
            const { name, cwd } = active;
            const { config } = runners.require(name);
            outcome = await sessions.turn({
              id,
              name,
              config,
              cwd,
              text: item.content,
              context,
              commit: (write) =>
                runtime.commit(async (tx) => {
                  await write(tx);
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
          delete chat.setupFailed;
          await touch(tx, id);
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
        delete chat.setupFailed;
        await touch(tx, id);
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
    async create(
      harness: Harness,
      options: { harness?: string; project?: string; parent?: ConversationId; options?: Desired },
      context: Context,
    ): Promise<Conversation> {
      const binding: { runner?: { name: string; cwd: string; config: HarnessConfig } } = {};
      const created = await harness.createConversation(
        {
          ownership: { kind: "ownerless" },
          init: async (tx, id) => {
            let project = options.project ?? null;
            let directory = cwd;
            let inherited: string | undefined;
            if (options.parent !== undefined) {
              const parent = await tx.doc(MetaDoc, options.parent);
              const chat = await tx.doc(ChatDoc, options.parent);
              project = parent.project;
              directory = chat.runner?.cwd ?? cwd;
              inherited = chat.runner?.name;
            } else if (project !== null) {
              directory = (await projectIn(tx, ROOT_CONVERSATION_ID, project)).path;
            }
            const { name, config } = runners.require(options.harness ?? inherited);
            const desired = initialDesired(config, options.options);
            (await tx.doc(ChatDoc, id)).runner = { name, cwd: directory };
            (await tx.doc(AcpStateDoc, id)).desired = desired;
            const meta = await tx.doc(MetaDoc, id);
            meta.project = project;
            meta.parent = options.parent ?? null;
            await touch(tx, id);
            binding.runner = { name, cwd: directory, config };
          },
        },
        context,
      );
      const runner = binding.runner!;
      void sessions
        .prepare({ id: created.id, ...runner })
        .catch(async (error: unknown) => {
          console.error(`ACP ${runner.name}: setup`, error instanceof Error ? error.message : error);
          await harness.commit(async (tx) => {
            const chat = await tx.doc(ChatDoc, created.id);
            if (!chat.hasWorker && chat.runner?.cwd === runner.cwd) chat.setupFailed = true;
          }, BACKGROUND_CONTEXT);
        })
        .catch((error: unknown) => console.error("ACP setup state:", error));
      return created;
    },
    async submit(harness: Harness, id: ConversationId, input: ChatInput, context: Context) {
      const { requestId, ...prompt } = input;
      const submission = await harness.commit(async (tx) => {
        const conversation = await tx.conversation(id);
        if (!conversation) throw new Error("Conversation not found");
        if (requestId !== undefined) {
          const existing = await tx.submissionByRequest(id, requestId);
          if (existing) return existing.id;
        }
        const delegated = conversation.owner ? await tx.doc(AgentDoc, id) : undefined;
        if (delegated?.outcome?.kind === "running") throw new Error("The subagent is still working");
        const chat = await tx.doc(ChatDoc, id);
        if (chat.runner === null) {
          // Older delegated ACP chats have a runner in viz.agent only. Native chats keep their visible history.
          const { name, config } = runners.require(delegated?.harness || undefined);
          const { project } = await tx.doc(MetaDoc, id);
          const directory = project === null ? cwd : (await projectIn(tx, ROOT_CONVERSATION_ID, project)).path;
          chat.runner = { name, cwd: directory };
          const acp = await tx.doc(AcpStateDoc, id);
          acp.desired = initialDesired(config, acp.desired);
          await configure(tx, id, {
            model: null,
            thinkingLevel: null,
            extensions: null,
            tools: null,
            instructions: null,
          });
        } else runners.require(chat.runner.name);
        const title = await tx.doc(TitleDoc, id);
        await touch(tx, id);
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
