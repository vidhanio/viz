/**
 * The orchestrator's memory, ported from uniichat's `Memory.ts` (github.com/VictorTaelin/uniichat @ 0500bf6).
 *
 * The chat, kept forever, and the view of it every turn sees. Messages (the orchestrator's transcript, flattened by
 * `sync.ts`) are logged in order; over them grows a binary tree (`tree.ts`): node (0,i) is message i in at most NODE
 * bytes, node (l,i) merges (l-1,2i) and (l-1,2i+1). A source that fits is its node; else a compaction writes it: a call
 * on the compactor model with the memory prompt, no tools, and its own view (`seen`) up to the node's `ctx`; a line too
 * long is asked anew. The call asks till it is answered (an outage only delays a node); answered with no line, its node
 * is its source clipped. JOBS build at once, in the order the messages came.
 *
 * A view lists nodes covering the chat to its end, oldest first; each message appends its node. Past `high` bytes, a
 * batch merges the adjacent siblings whose parent is built and most due (oldest since its last message, in halves:
 * detail fades with age) down to half, so a view changes only at its end between batches. Two views: the chat's (for
 * turns, to the last message) and the compactions' (`seen`, to the first message not built). A call gets one in
 * BLOCK-line blocks, the last whole one marked for the prompt cache.
 *
 * Differences from uniichat: storage is one SQLite file instead of JSONL logs and indexes; there are no imports
 * (one side), billing holds, images, or agent logs; the turn loop is pi-durable's (`extension.ts`).
 */
import { Database, type Statement } from "bun:sqlite";

import {
  AGAIN,
  BLOCK,
  type Block,
  bytes,
  ctx,
  cut,
  flat,
  halves,
  HIGH,
  JOBS,
  line,
  NODE,
  over,
  page,
  pages,
  type Part,
  ref,
  roots,
  SEEN,
  slot,
  span,
  task,
  TRIES,
  WAIT,
} from "./tree.ts";

/** Who a message is from: the user, viz's replies, its tool calls, their results, a subagent's report, a note. */
export type Kind = "user" | "viz" | "tool" | "echo" | "work" | "note";

/** A message to log; `entry`: the pi-durable entry it came from. */
export type Msg = { readonly kind: Kind; readonly text: string; readonly date: string; readonly entry: number };

type Message = Msg & { readonly i: number };

/** A view: its lines, the bytes of those built, a batch under way, the bytes past which it merges to half. */
type View = { parts: Part[]; size: number; batch: boolean; readonly high: number };

/** A view as kept in `meta.views`. */
type Kept = { parts: [number, number][]; batch: boolean };

/** One compaction call's answer, or why there is none (the memory asks again after AGAIN). */
export type CompactResult =
  { readonly ok: true; readonly text: string } | { readonly ok: false; readonly error: string };

/** Asks the compactor model, with the memory prompt as its system prompt, for one line. */
export type Compactor = (blocks: readonly Block[], signal: AbortSignal) => Promise<CompactResult>;

const SCHEMA = `
CREATE TABLE IF NOT EXISTS messages (
  i     INTEGER PRIMARY KEY,
  kind  TEXT NOT NULL,
  text  TEXT NOT NULL,
  date  TEXT NOT NULL,
  entry INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS nodes (
  slot INTEGER PRIMARY KEY,
  l    INTEGER NOT NULL,
  i    INTEGER NOT NULL,
  text TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);`;

export class Memory {
  /** Messages logged. */
  count: number;
  readonly #db: Database;
  readonly #compactor: Compactor;
  readonly #stmt: {
    message: Statement<{ kind: Kind; text: string; date: string; entry: number }, [number]>;
    node: Statement<{ text: string }, [number]>;
    addMessage: Statement<unknown, [number, string, string, string, number]>;
    addNode: Statement<unknown, [number, number, number, string]>;
    meta: Statement<{ value: string }, [string]>;
    setMeta: Statement<unknown, [string, string]>;
  };
  /** The chat's view. */
  readonly view: View = { parts: [], size: 0, batch: false, high: HIGH };
  /** The compactions' view. */
  readonly seen: View = { parts: [], size: 0, batch: false, high: SEEN };
  /** Nodes not built whose halves are, by level, sorted by index. */
  readonly #ready: number[][] = [];
  /** Nodes building, by slot. */
  readonly #busy = new Set<number>();
  /** Checks run as the views change (`settle`). */
  readonly #wait = new Set<() => void>();
  readonly #abort = new AbortController();

  constructor(path: string, compactor: Compactor) {
    this.#compactor = compactor;
    this.#db = new Database(path, { create: true });
    this.#db.exec("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;");
    this.#db.exec(SCHEMA);
    this.#stmt = {
      message: this.#db.query("SELECT kind, text, date, entry FROM messages WHERE i = ?"),
      node: this.#db.query("SELECT text FROM nodes WHERE slot = ?"),
      addMessage: this.#db.query("INSERT INTO messages (i, kind, text, date, entry) VALUES (?, ?, ?, ?, ?)"),
      addNode: this.#db.query("INSERT OR REPLACE INTO nodes (slot, l, i, text) VALUES (?, ?, ?, ?)"),
      meta: this.#db.query("SELECT value FROM meta WHERE key = ?"),
      setMeta: this.#db.query("INSERT OR REPLACE INTO meta (key, value) VALUES (?, ?)"),
    };
    this.count = this.#db.query<{ n: number }, []>("SELECT COALESCE(MAX(i) + 1, 0) AS n FROM messages").get()!.n;

    const saved = this.meta("views");
    const kept = saved === null ? null : (JSON.parse(saved) as { view: Kept; seen: Kept });
    this.#open(this.view, kept?.view ?? null);
    this.#open(this.seen, kept?.seen ?? kept?.view ?? null);
    for (const part of roots(this.count).flatMap((root) => this.#todo(root))) this.#ready_(part, true);
    this.fit();
    this.#save();
    this.#pump();
  }

  /** Lets go: no node starts again, running compactions are cut, the database closes once they end. */
  stop(): void {
    this.#abort.abort();
    for (const check of this.#wait) check();
  }

  close(): void {
    this.stop();
    this.#db.close();
  }

  meta(key: string): string | null {
    return this.#stmt.meta.get(key)?.value ?? null;
  }

  /**
   * Logs messages (a text over CAP characters as several in a row), with `meta` set in the same transaction, then
   * starts building their nodes.
   */
  push(msgs: readonly Msg[], meta?: readonly [string, string]): void {
    const logged = msgs.flatMap((msg) => pages(msg.text).map((text) => ({ ...msg, text })));
    this.#db.transaction(() => {
      logged.forEach((msg, k) => this.#stmt.addMessage.run(this.count + k, msg.kind, msg.text, msg.date, msg.entry));
      if (meta !== undefined) this.#stmt.setMeta.run(meta[0], meta[1]);
    })();
    // Only `meta` moved: a batch goes on at a message, not at nothing.
    if (logged.length === 0) return;
    const from = this.count;
    this.count += logged.length;
    for (let i = from; i < this.count; i++) this.#ready_({ l: 0, i }, true);
    this.fit(true);
    this.#pump();
  }

  message(i: number): Message | null {
    if (!Number.isInteger(i) || i < 0 || i >= this.count) return null;
    const row = this.#stmt.message.get(i);
    return row ? { i, ...row } : null;
  }

  // Tree
  // ----

  /** Node (l,i)'s text; null: not built. */
  node(part: Part): string | null {
    return this.#stmt.node.get(slot(part.l, part.i))?.text ?? null;
  }

  built(l: number, i: number): boolean {
    return this.node({ l, i }) !== null;
  }

  /** A message as models read it, tagged by its author. */
  static show(msg: { readonly kind: Kind; readonly text: string }): string {
    return `${msg.kind}: ${msg.text}`;
  }

  /** What node (l,i) summarizes: its message, or its halves' lines; null: not there yet. */
  #source(l: number, i: number): string | null {
    if (l === 0) {
      const msg = this.message(i);
      return msg === null ? null : Memory.show(msg);
    }
    const [a, b] = halves({ l, i }).map((half) => this.node(half));
    return a === null || b === null ? null : `${flat(a!)}\n${flat(b!)}`;
  }

  /** The (k+1)th message whose leaf isn't built, else the count. */
  #first(k = 0): number {
    return this.#ready[0]?.[k] ?? this.count;
  }

  /** A node ready to build (on), or no more (built). */
  #ready_({ l, i }: Part, on: boolean): void {
    const queue = (this.#ready[l] ??= []);
    let [a, z] = [0, queue.length];
    while (a < z) {
      const m = (a + z) >> 1;
      [a, z] = queue[m]! < i ? [m + 1, z] : [a, m];
    }
    if (on !== (queue[a] === i)) queue.splice(a, on ? 0 : 1, ...(on ? [i] : []));
  }

  /** The nodes under `part` not built whose sources are: only nodes not built and their halves are visited. */
  #todo(part: Part): Part[] {
    if (this.built(part.l, part.i)) return [];
    const parts = halves(part);
    const here = parts.every((half) => this.built(half.l, half.i)) ? [part] : [];
    return [...here, ...parts.flatMap((half) => this.#todo(half))];
  }

  /** Builds what's ready: those whose source fits at once, together; the rest as compaction jobs. */
  #pump(): void {
    let built = false;
    for (let free = this.#starts(); free.length > 0 && !this.#abort.signal.aborted; free = this.#starts()) {
      this.#store(free);
      built = true;
    }
    if (built) this.fit();
  }

  /**
   * The nodes ready up to the JOBS-th message not built, in the order the messages came (`ctx`; at a tie, a merge
   * first), while JOBS run. Starts jobs; returns the nodes whose source fits.
   */
  #starts(): (Part & { text: string })[] {
    const free: (Part & { text: string })[] = [];
    const end = this.#first(JOBS - 1);
    const next: Part[] = [];
    this.#ready.forEach((queue, l) => {
      for (let k = 0; k < queue.length && ctx(l, queue[k]!) <= end; k++) next.push({ l, i: queue[k]! });
    });
    next.sort((a, b) => ctx(a.l, a.i) - ctx(b.l, b.i) || b.l - a.l);
    for (const { l, i } of next) {
      if (this.#abort.signal.aborted || this.#busy.size >= JOBS) break;
      const text = this.#busy.has(slot(l, i)) ? null : this.#source(l, i);
      if (text !== null && bytes(text) <= NODE) free.push({ l, i, text });
      else if (text !== null) this.#job(l, i, text);
    }
    return free;
  }

  #job(l: number, i: number, text: string): void {
    const key = slot(l, i);
    this.#busy.add(key);
    this.#build(l, i, text)
      .finally(() => {
        this.#busy.delete(key);
        this.#pump();
        this.fit();
      })
      .catch((error: unknown) => {
        if (!this.#abort.signal.aborted) console.error(`memory: node ${l}/${i}:`, error);
      });
  }

  /**
   * Node (l,i)'s summary: its view to the node and its task, asked till a line fits, TRIES lines at most, the shortest
   * kept. A try after a line too long adds only where the limit cuts it. No line: its source clipped.
   */
  async #build(l: number, i: number, text: string): Promise<void> {
    const job = { text: `${task(l, i)}${text}\n</input>`, mark: false };
    const upto = Math.min(ctx(l, i), this.#first(0));
    const blocks = [...this.blocks(upto, this.seen.parts), job];
    const lines: string[] = [];
    let note: Block | undefined;
    while (lines.length < TRIES) {
      const answer = await this.#ask(note === undefined ? blocks : [...blocks, note]);
      if (answer === null) throw new Error("stopped");
      const got = answer.trim();
      if (got === "") break;
      lines.push(got);
      if (bytes(got) <= NODE) break;
      note = { text: over(got), mark: false };
    }
    if (lines.length === 0) console.error(`memory: node ${l}/${i}: an empty line; clipped`);
    const best = lines.length === 0 ? cut(flat(text)) : lines.reduce((a, b) => (bytes(b) < bytes(a) ? b : a));
    this.#store([{ l, i, text: best }]);
  }

  /** Asks till answered: a failure waits AGAIN and asks again. Null: stopped. */
  async #ask(blocks: readonly Block[]): Promise<string | null> {
    const signal = this.#abort.signal;
    while (!signal.aborted) {
      const result = await this.#compactor(blocks, signal);
      if (result.ok) return result.text;
      if (signal.aborted) break;
      console.error(`memory: compactor failed, asking again in ${AGAIN / 1000} s: ${result.error}`);
      const { promise, resolve } = Promise.withResolvers<void>();
      const timer = setTimeout(resolve, AGAIN);
      signal.addEventListener("abort", () => (clearTimeout(timer), resolve()), { once: true });
      await promise;
    }
    return null;
  }

  /** Nodes built: logged in one write; their parents ready once both halves are. */
  #store(parts: readonly (Part & { text: string })[]): void {
    if (this.#abort.signal.aborted) return;
    this.#db.transaction(() => {
      for (const { l, i, text } of parts) this.#stmt.addNode.run(slot(l, i), l, i, text);
    })();
    for (const { l, i, text } of parts) {
      this.#ready_({ l, i }, false);
      for (const view of [this.view, this.seen]) view.size += l === 0 && i < Memory.#end(view) ? bytes(text) : 0;
    }
    for (const { l, i } of parts) {
      const up = { l: l + 1, i: i >> 1 };
      if (this.built(l, i ^ 1) && !this.built(up.l, up.i)) this.#ready_(up, true);
    }
  }

  // View
  // ----

  /** A view as kept (none: the highest nodes built). */
  #open(view: View, kept: Kept | null): void {
    const keys = kept === null ? null : new Set(kept.parts.map(([l, i]) => slot(l, i)));
    view.parts = roots(this.count).flatMap((part) => this.#down(part, keys));
    view.size = view.parts.reduce((n, part) => n + this.#size(part), 0);
    view.batch = kept?.batch ?? false;
  }

  /** The view under `part`, down from it, keeping the built nodes met: those of `keys` (none: any), or messages. */
  #down(part: Part, keys: Set<number> | null): Part[] {
    const built = this.built(part.l, part.i);
    if (part.l === 0 || (built && (keys === null || keys.has(slot(part.l, part.i))))) return [part];
    return halves(part).flatMap((half) => this.#down(half, keys));
  }

  #save(): void {
    const kept = (view: View): Kept => ({ parts: view.parts.map((part) => [part.l, part.i]), batch: view.batch });
    this.#stmt.setMeta.run("views", JSON.stringify({ view: kept(this.view), seen: kept(this.seen) }));
  }

  #text(part: Part): string {
    return this.node(part) ?? WAIT;
  }

  /** A line's bytes as calls get it: none until built. */
  #size(part: Part): number {
    const text = this.node(part);
    return text === null ? 0 : bytes(text);
  }

  /**
   * Each view to its end; a batch starts as a view passes `high`, or goes on at a message (`next`) if a missing parent
   * stopped it; between messages, only a whole batch, to half: each change makes every call after write the view anew.
   */
  fit(next = false): void {
    // Jobs ending after `close` land here; the database is gone.
    if (this.#abort.signal.aborted) return;
    let changed = false;
    this.#reach(this.view, this.count);
    this.#reach(this.seen, Math.min(this.count, this.#first(0)));
    for (const view of [this.view, this.seen]) {
      const [parts, batch] = [view.parts.length, view.batch];
      if (view.size > view.high || (next && view.batch)) {
        const merged = this.#merge(view);
        if (next || merged.size <= Math.max(view.high / 2, view.size - view.high)) {
          [view.parts, view.size] = [merged.parts, merged.size];
        }
        view.batch = view.size > view.high / 2;
      }
      changed ||= view.parts.length !== parts || view.batch !== batch;
    }
    if (changed) this.#save();
    for (const check of this.#wait) check();
  }

  /** A view to message `end`: its lines from it dropped, the messages to it appended. */
  #reach(view: View, end: number): void {
    for (const part of view.parts.splice(Memory.#cover(view, end))) view.size -= this.#size(part);
    for (let i = Memory.#end(view); i < end; i++) {
      view.parts.push({ l: 0, i });
      view.size += this.#size({ l: 0, i });
    }
  }

  /** The message after a view's last line. */
  static #end({ parts }: View): number {
    return parts.length === 0 ? 0 : span(parts[parts.length - 1]!)[1] + 1;
  }

  /** The index of a view's line holding message id (past its end: the length). */
  static #cover({ parts }: View, id: number): number {
    let [a, z] = [0, parts.length];
    while (a < z) {
      const m = (a + z) >> 1;
      [a, z] = span(parts[m]!)[1] < id ? [m + 1, z] : [a, m];
    }
    return a;
  }

  /**
   * A view's most due pairs merged down to half, as their parents allow: a pair whose parent is built, most due first
   * ((T + 1) / 2^l - i, T the view's end: its age from its last message, in halves), the oldest of equals; a heap of
   * pairs over the view as a linked list, so a view of thousands merges in milliseconds.
   */
  #merge(from: View): { parts: Part[]; size: number } {
    const [t, low] = [Memory.#end(from), from.high / 2];
    const view: (Part | null)[] = [...from.parts];
    const next = view.map((_, k) => k + 1);
    const prev = view.map((_, k) => k - 1);
    type Pair = { d: number; k: number; a: Part; b: Part };
    const heap: Pair[] = [];
    const above = (x: Pair, y: Pair) => x.d > y.d || (x.d === y.d && x.k < y.k);
    const swap = (x: number, y: number) => ([heap[x], heap[y]] = [heap[y]!, heap[x]!]);
    const offer = (k: number) => {
      const [a, b] = [view[k], view[next[k]!]];
      if (k < 0 || !a || !b || a.i % 2 !== 0 || b.l !== a.l || b.i !== a.i + 1 || !this.built(a.l + 1, a.i / 2)) return;
      heap.push({ d: (t + 1) / 2 ** a.l - a.i, k, a, b });
      for (let x = heap.length - 1; x > 0 && above(heap[x]!, heap[(x - 1) >> 1]!); x = (x - 1) >> 1)
        swap(x, (x - 1) >> 1);
    };
    const take = () => {
      const top = heap[0]!;
      const last = heap.pop()!;
      if (heap.length > 0) {
        heap[0] = last;
        for (let x = 0; ;) {
          const m = [x, 2 * x + 1, 2 * x + 2].reduce((m, c) => (c < heap.length && above(heap[c]!, heap[m]!) ? c : m));
          if (m === x) break;
          swap(x, m);
          x = m;
        }
      }
      return top;
    };
    let size = from.size;
    view.forEach((_, k) => offer(k));
    while (heap.length > 0 && size > low) {
      const { k, a, b } = take();
      const j = next[k]!;
      if (view[k] !== a || view[j] !== b) continue;
      const up = { l: a.l + 1, i: a.i / 2 };
      size += this.#size(up) - this.#size(a) - this.#size(b);
      [view[k], view[j]] = [up, null];
      next[k] = next[j]!;
      prev[next[j]!] = k;
      offer(prev[k]!);
      offer(k);
    }
    return { parts: view.filter((part) => part !== null), size };
  }

  /** Resolves true once the messages before `end` are built; false if `signal` (or `stop`) aborts first. */
  settle(end: number, signal: AbortSignal | undefined): Promise<boolean> {
    const { promise, resolve } = Promise.withResolvers<boolean>();
    const check = () => {
      const built = this.#first() >= end;
      if (built || signal?.aborted || this.#abort.signal.aborted) {
        this.#wait.delete(check);
        signal?.removeEventListener("abort", check);
        resolve(built);
      }
    };
    this.#wait.add(check);
    signal?.addEventListener("abort", check);
    check();
    return promise;
  }

  /**
   * The view's lines built before message `end`, in blocks (the last cut short), one marked for the prompt cache: the
   * last whole one, else the opening one.
   */
  blocks(end = this.count, parts = this.view.parts): Block[] {
    const lines = parts
      .filter((part) => span(part)[1] < end && this.built(part.l, part.i))
      .map((part) => `${line(part, this.#text(part))}\n`);
    const whole = Math.floor(lines.length / BLOCK);
    const blocks: Block[] = [{ text: "<chat>\n", mark: whole === 0 }];
    for (let k = 0; k < whole; k++) {
      blocks.push({ text: lines.slice(k * BLOCK, (k + 1) * BLOCK).join(""), mark: k === whole - 1 });
    }
    blocks.push({ text: `${lines.slice(whole * BLOCK).join("")}</chat>\n`, mark: false });
    return blocks;
  }

  // Tools
  // -----

  /** Line id+n, if it is one: n a power of 2, id a multiple of n, within the chat. */
  #part(id: number, n: number): Part | null {
    const l = Math.log2(n);
    const ok = Number.isInteger(id) && Number.isInteger(l) && id >= 0 && id % n === 0 && id + n <= this.count;
    return ok ? { l, i: id / n } : null;
  }

  /** A message (n = 1) whole, a long one a page from `at`; else the two lines under id+n. */
  zoom(id: number, n: number, at: number): string {
    const part = this.#part(id, n);
    if (part === null) {
      const tops = roots(this.count).map(ref).join(", ");
      return `No line ${id}+${n}: n is a power of 2, id a multiple of n. The tree's roots: ${tops}.`;
    }
    if (n === 1) {
      const msg = this.message(id)!;
      return `${id}+0|${Memory.show({ ...msg, text: page(msg.text, at) })}`;
    }
    return halves(part)
      .map((half) => line(half, this.node(half) ?? WAIT))
      .join("\n");
  }

  date(id: number): string {
    const msg = this.message(id);
    return msg === null ? `No message ${id}.` : new Date(msg.date).toLocaleString("sv-SE");
  }
}
