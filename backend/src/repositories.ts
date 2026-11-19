import type { Context } from "@earendil-works/chord/context";
import { type ConversationId, type EntryId, type Harness, LiveDoc } from "@earendil-works/pi-durable";

import { ChatDoc } from "./agents/chat.ts";
import type { ChangeFeed } from "./changes.ts";
import type { Conversations } from "./conversations.ts";
import type { Git } from "./git/index.ts";
import { MetaDoc } from "./projects.ts";
import type { Params } from "./protocol.ts";

/** Serializes backend writes to a working tree, across chats that share it. */
export class Repositories {
  readonly #locks = new Map<string, Promise<unknown>>();

  constructor(
    private readonly git: Git,
    private readonly harness: Harness,
    private readonly root: ConversationId,
    private readonly conversations: Conversations,
    private readonly changes: ChangeFeed,
  ) {}

  async inTree<T>(cwd: string, context: Context, operation: (directory: string | null) => Promise<T>): Promise<T> {
    const directory = await this.git.root(cwd);
    const run = () => {
      context.abortSignal?.throwIfAborted();
      return operation(directory);
    };
    if (directory === null) return run();
    const job = (this.#locks.get(directory) ?? Promise.resolve()).then(run, run);
    this.#locks.set(directory, job);
    try {
      return await job;
    } finally {
      if (this.#locks.get(directory) === job) this.#locks.delete(directory);
    }
  }

  checkpoint(id: ConversationId, prompt: EntryId, cwd: string, context: Context): Promise<void> {
    return this.inTree(cwd, context, async (directory) => {
      if (directory !== null) await this.git.checkpoint(id, prompt, directory, context);
    });
  }

  async diff(id: ConversationId, spec: Omit<Params<"git.diff">, "conversation">, context: Context) {
    const cwd = await this.conversations.cwdFor(id, context);
    try {
      const meta = await this.harness.snapshot(MetaDoc, id, context);
      return await this.inTree(cwd, context, () => this.git.diff(id, cwd, meta?.worktree?.base, spec, context));
    } finally {
      this.changes.refresh(id);
    }
  }

  async branches(id: ConversationId, context: Context) {
    try {
      return await this.git.branches(await this.conversations.cwdFor(id, context));
    } finally {
      this.changes.refresh(id);
    }
  }

  checkout(id: ConversationId, branch: string, create: boolean | undefined, context: Context) {
    return this.#write(id, context, (cwd) => this.git.checkout(cwd, branch, create));
  }

  commit(id: ConversationId, message: string | undefined, all: boolean | undefined, context: Context) {
    return this.#write(id, context, (cwd) => this.git.commit(cwd, message, all, context));
  }

  push(id: ConversationId, context: Context) {
    return this.#write(id, context, (cwd) => this.git.push(cwd), false);
  }

  pr(id: ConversationId, spec: Omit<Params<"git.pr">, "conversation">, context: Context) {
    return this.#write(id, context, (cwd) => this.git.pr(cwd, spec), false);
  }

  revert(id: ConversationId, turn: number, context: Context) {
    return this.#write(id, context, (cwd) => this.git.revert(id, cwd, turn, context));
  }

  async #write<T>(
    id: ConversationId,
    context: Context,
    operation: (cwd: string) => Promise<T>,
    idle = true,
  ): Promise<T> {
    const cwd = await this.conversations.cwdFor(id, context);
    try {
      return await this.inTree(cwd, context, async (directory) => {
        if (directory === null) throw new Error("Not in a Git repository");
        if (cwd !== (await this.conversations.cwdFor(id, context))) throw new Error("Conversation directory changed");
        if (idle) await this.#requireIdle(directory, context);
        return operation(directory);
      });
    } finally {
      this.changes.refresh(id);
    }
  }

  async #requireIdle(directory: string, context: Context): Promise<void> {
    const [{ tasks }, live] = await Promise.all([
      this.harness.inspect(context),
      this.harness.snapshot(LiveDoc, this.root, context),
    ]);
    const active = new Set<ConversationId>();
    if (live?.run) active.add(this.root);
    for (const { record } of tasks) if (record.kind === ChatDoc.definition.kind) active.add(record.conversationId);
    const checked = new Set<string>();
    for (const id of active) {
      const cwd = await this.conversations.cwdFor(id, context);
      if (checked.has(cwd)) continue;
      checked.add(cwd);
      if (cwd === directory || (await this.git.root(cwd)) === directory)
        throw new Error("Stop the turn before changing the working tree");
    }
  }
}
