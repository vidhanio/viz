import { Database, type Statement } from "bun:sqlite";
import { join } from "node:path";

import type { Context } from "@earendil-works/chord/context";
import {
  AssistantEntry,
  type CommitObservation,
  type ConversationId,
  type Cursor,
  type EntryId,
  type EntryRecord,
  type Harness,
  ToolResultEntry,
  UserEntry,
} from "@earendil-works/pi-durable";

import { PromptEntry, TextEntry, ToolEntry } from "./agents/model.ts";
import type { Result } from "./protocol.ts";

type SearchResult = Result<"conversations.search">;
const PAGE = 256;

const SCHEMA = `
CREATE VIRTUAL TABLE IF NOT EXISTS search_entries USING fts5(text, conversation UNINDEXED);
CREATE TABLE IF NOT EXISTS search_cursors (
  conversation INTEGER PRIMARY KEY,
  entry INTEGER NOT NULL
);`;

function entryText(entry: EntryRecord): string | undefined {
  if (PromptEntry.is(entry) || TextEntry.is(entry)) return entry.data.text;
  if (ToolEntry.is(entry)) return entry.data.title;

  const input = entry.kind === "viz.input";
  const data = entry.data;
  // A connector's original text excludes the routing prefix that the model receives.
  if (input && data && typeof data === "object" && !Array.isArray(data) && typeof data.text === "string") {
    return data.text;
  }
  const user = input || UserEntry.is(entry);
  const assistant = AssistantEntry.is(entry);
  const tool = ToolResultEntry.is(entry);
  if ((!user && !assistant && !tool) || !entry.model) return;

  const parts: string[] = [];
  for (const message of entry.model) {
    if (user && message.role === "user") {
      if (typeof message.content === "string") parts.push(message.content);
      else for (const block of message.content) if (block.type === "text") parts.push(block.text);
    } else if (assistant && message.role === "assistant") {
      for (const block of message.content) {
        if (block.type === "text") parts.push(block.text);
        else if (block.type === "toolCall") parts.push(block.name);
      }
    } else if (tool && message.role === "toolResult") {
      parts.push(message.toolName);
    }
  }
  return parts.join("\n");
}

export class SearchIndex {
  readonly #db: Database;
  readonly #cursor: Statement<{ entry: number }, [ConversationId]>;
  readonly #query: Statement<SearchResult["hits"][number], [string, number]>;
  readonly #store: (entries: readonly EntryRecord[]) => void;
  #observation: CommitObservation | undefined;
  #pending: EntryRecord[] = [];
  #backfilling = true;
  #closed = false;
  #failure: Error | undefined;

  private constructor(db: Database) {
    this.#db = db;
    db.exec("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;");
    db.exec(SCHEMA);
    this.#cursor = db.query("SELECT entry FROM search_cursors WHERE conversation = ?");
    this.#query = db.query(`
      SELECT conversation, rowid AS entry, snippet(search_entries, 0, char(1), char(2), '…', 32) AS snippet
      FROM search_entries
      WHERE search_entries MATCH ?
      ORDER BY bm25(search_entries), rowid DESC
      LIMIT ?`);
    const insert = db.query<unknown, [EntryId, ConversationId, string]>(
      "INSERT INTO search_entries (rowid, conversation, text) VALUES (?, ?, ?)",
    );
    const save = db.query<unknown, [ConversationId, number]>(
      "INSERT INTO search_cursors (conversation, entry) VALUES (?, ?) ON CONFLICT(conversation) DO UPDATE SET entry = excluded.entry",
    );
    this.#store = db.transaction((entries: readonly EntryRecord[]) => {
      // Publication change order is unspecified. Compare every entry to the prior cursor, then advance past the batch.
      const cursors = new Map<ConversationId, { before: number; after: number }>();
      for (const entry of entries) {
        const id = entry.conversationId;
        let cursor = cursors.get(id);
        if (!cursor) {
          const before = this.#cursor.get(id)?.entry ?? 0;
          cursor = { before, after: before };
          cursors.set(id, cursor);
        }
        if (entry.id <= cursor.before) continue;
        // The snippet's control markers must come from FTS, not the transcript.
        const text = entryText(entry)
          ?.replace(/[\0\u0001\u0002]/g, " ")
          .trim();
        if (text) insert.run(entry.id, id, text);
        cursor.after = Math.max(cursor.after, entry.id);
      }
      for (const [id, { before, after }] of cursors) if (after > before) save.run(id, after);
    });
  }

  static async open(stateDir: string, harness: Harness, context: Context): Promise<SearchIndex> {
    const db = new Database(join(stateDir, "search.sqlite"), { create: true });
    try {
      const index = new SearchIndex(db);
      await index.#start(harness, context);
      return index;
    } catch (error) {
      db.close();
      throw error;
    }
  }

  async #start(harness: Harness, context: Context): Promise<void> {
    // Attach before scanning. A live entry must not advance a cursor past an older unscanned entry.
    this.#observation = harness.observeCommits({
      advance: ({ changes }) => {
        const empty = this.#pending.length === 0;
        for (const change of changes) if (change.type === "entry") this.#pending.push(change.value);
        if (!this.#backfilling && empty && this.#pending.length) queueMicrotask(() => this.#flushLive());
      },
      end() {},
    });
    try {
      await this.#backfill(harness, context);
      this.#drain();
      this.#backfilling = false;
    } catch (error) {
      this.#observation.detach();
      throw error;
    }
  }

  async #backfill(harness: Harness, context: Context): Promise<void> {
    // Page newest conversations first so newly created ones cannot extend the scan. The observer covers their entries.
    let cursor: Cursor | undefined;
    do {
      const conversations = await harness.commit(
        (tx) => tx.scanConversations({ order: "descending" }, PAGE, cursor),
        context,
      );
      for (const { id, parent } of conversations.items) {
        // Forks inherit entries with the original conversation ID; index those only in the source conversation.
        const after = Math.max(parent?.at ?? 0, this.#cursor.get(id)?.entry ?? 0);
        const maxEntryId = await harness.commit((tx) => tx.latestEntryId(id), context);
        if (maxEntryId === undefined || maxEntryId <= after) continue;
        const query = {
          conversationId: id,
          minEntryId: (after + 1) as EntryId,
          maxEntryId,
          order: "ascending",
        } as const;
        let next: Cursor | undefined;
        do {
          const entries = await harness.commit((tx) => tx.scanEntries(query, PAGE, next), context);
          this.#store(entries.items);
          next = entries.next;
        } while (next !== undefined);
      }
      cursor = conversations.next;
    } while (cursor !== undefined);
  }

  #drain(): void {
    if (!this.#pending.length) return;
    const entries = this.#pending;
    this.#pending = [];
    this.#store(entries);
  }

  #flushLive(): void {
    try {
      this.#drain();
    } catch (cause) {
      // Do not let later entries advance a cursor past a failed batch. Reopening resumes from the durable cursors.
      this.#observation?.detach();
      this.#failure = new Error("Updating transcript search index failed", { cause });
      console.error(this.#failure);
    }
  }

  search(query: string, limit = 50): SearchResult {
    this.#flushLive();
    if (this.#failure) throw this.#failure;
    // Quote literal terms so punctuation and words such as AND or NEAR never become FTS operators.
    const terms = query.match(/[\p{L}\p{N}\p{Co}][\p{L}\p{N}\p{Co}\p{M}]*/gu);
    return { hits: terms ? this.#query.all(terms.map((term) => `"${term}"`).join(" "), limit) : [] };
  }

  close(): void {
    if (this.#closed) return;
    this.#closed = true;
    this.#observation?.detach();
    this.#flushLive();
    this.#db.close();
  }
}
