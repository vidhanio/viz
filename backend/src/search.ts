import { Database, type Statement } from "bun:sqlite";
import { join } from "node:path";

import type { Context } from "@earendil-works/chord/context";
import {
  type CommitObservation,
  type ConversationId,
  type Cursor,
  type EntryId,
  type EntryRecord,
  type Harness,
} from "@earendil-works/pi-durable";

import { isDeleted, MetaDoc } from "./projects.ts";
import type { Result } from "./protocol.ts";
import { entryLine } from "./transcripts.ts";

type SearchResult = Result<"conversations.search">;
const PAGE = 256;

const SCHEMA = `
CREATE VIRTUAL TABLE IF NOT EXISTS search_entries USING fts5(text, conversation UNINDEXED);
CREATE TABLE IF NOT EXISTS search_cursors (
  conversation INTEGER PRIMARY KEY,
  entry INTEGER NOT NULL
);`;

export class SearchIndex {
  readonly #db: Database;
  readonly #cursor: Statement<{ entry: number }, [ConversationId]>;
  readonly #query: Statement<SearchResult["hits"][number], [string, number]>;
  readonly #store: (entries: readonly EntryRecord[]) => void;
  readonly #remove: (ids: readonly ConversationId[]) => void;
  /** Deleted conversations: never indexed again. Rebuilt from `viz.meta` on every start. */
  readonly #deleted = new Set<ConversationId>();
  #observation: CommitObservation | undefined;
  #pending: EntryRecord[] = [];
  #pendingDeletes: ConversationId[] = [];
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
        if (this.#deleted.has(id)) continue;
        let cursor = cursors.get(id);
        if (!cursor) {
          const before = this.#cursor.get(id)?.entry ?? 0;
          cursor = { before, after: before };
          cursors.set(id, cursor);
        }
        if (entry.id <= cursor.before) continue;
        // The snippet's control markers must come from FTS, not the transcript.
        const text = entryLine(entry)
          ?.text.replace(/[\0\u0001\u0002]/g, " ")
          .trim();
        if (text) insert.run(entry.id, id, text);
        cursor.after = Math.max(cursor.after, entry.id);
      }
      for (const [id, { before, after }] of cursors) if (after > before) save.run(id, after);
    });
    const drop = db.query<unknown, [ConversationId]>("DELETE FROM search_entries WHERE conversation = ?");
    const forget = db.query<unknown, [ConversationId]>("DELETE FROM search_cursors WHERE conversation = ?");
    // Without a cursor a conversation has no rows, so backfill knows which deleted conversations are already clean.
    this.#remove = db.transaction((ids: readonly ConversationId[]) => {
      for (const id of ids) {
        drop.run(id);
        forget.run(id);
      }
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
        const empty = this.#pending.length === 0 && this.#pendingDeletes.length === 0;
        for (const change of changes) {
          if (change.type === "entry") this.#pending.push(change.value);
          else if (
            change.type === "document" &&
            change.kind === "change" &&
            change.record.kind === MetaDoc.definition.kind &&
            change.conversationId !== undefined &&
            change.value.deleted === true &&
            !this.#deleted.has(change.conversationId)
          ) {
            this.#deleted.add(change.conversationId);
            this.#pendingDeletes.push(change.conversationId);
          }
        }
        if (!this.#backfilling && empty && (this.#pending.length || this.#pendingDeletes.length))
          queueMicrotask(() => this.#flushLive());
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
        if (await isDeleted(harness, id, context)) {
          // A crash between the deletion and `remove` leaves rows behind; drop them now.
          this.#deleted.add(id);
          if (this.#cursor.get(id)) this.#remove([id]);
          continue;
        }
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
    if (this.#pendingDeletes.length) {
      const deleted = this.#pendingDeletes;
      this.#pendingDeletes = [];
      this.#remove(deleted);
    }
    if (!this.#pending.length) return;
    const entries = this.#pending;
    this.#pending = [];
    this.#store(entries);
  }

  #flushLive(): void {
    if (this.#failure) return;
    try {
      this.#drain();
    } catch (cause) {
      // Do not let later entries advance a cursor past a failed batch. Reopening resumes from the durable cursors.
      this.#observation?.detach();
      this.#failure = new Error("Updating transcript search index failed", { cause });
      console.error(this.#failure);
    }
  }

  search(query: string, limit = 50, groups?: ReadonlyMap<number, number>): SearchResult {
    this.#flushLive();
    if (this.#failure) throw this.#failure;
    // Quote literal terms so punctuation and words such as AND or NEAR never become FTS operators.
    const terms = query.match(/[\p{L}\p{N}\p{Co}][\p{L}\p{N}\p{Co}\p{M}]*/gu);
    if (!terms) return { hits: [] };
    const match = terms.map((term) => `"${term}"`).join(" ");
    if (!groups) return { hits: this.#query.all(match, limit) };
    const hits: SearchResult["hits"] = [];
    const seen = new Set<number>();
    // Limit after grouping so strong matches in one subtree cannot crowd out other chats.
    for (const hit of this.#query.iterate(match, -1)) {
      const conversation = groups.get(hit.conversation);
      if (conversation === undefined || seen.has(conversation)) continue;
      seen.add(conversation);
      hits.push({ ...hit, conversation });
      if (hits.length === limit) break;
    }
    return { hits };
  }

  close(): void {
    if (this.#closed) return;
    this.#closed = true;
    this.#observation?.detach();
    this.#flushLive();
    this.#db.close();
  }
}
