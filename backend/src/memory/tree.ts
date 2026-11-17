/**
 * The memory tree's arithmetic and texts, ported from uniichat's `Memory.ts`
 * (github.com/VictorTaelin/uniichat @ 0500bf6). Sizes are bytes of UTF-8.
 *
 * Node (0,i) is message i (or its summary line); node (l,i) merges (l-1,2i) and (l-1,2i+1). A node covers messages
 * `span(node)`; its zoom target is `ref(node)` = "id+n".
 */

/** The most bytes a node's line may hold. */
export const NODE = 512;
/** Summaries building at once. */
export const JOBS = 64;
/** Lines asked of the compactor per node before the shortest is kept. */
export const TRIES = 5;
/** A view past its `high` merges to half: the chat's 64-128 KB, the compactions' 16-32 KB. */
export const HIGH = 128_000;
export const SEEN = 32_000;
/** Lines per cache block of a view. */
export const BLOCK = 4;
/** The most characters of one message; longer texts are logged as several in a row (uniichat Clip.ts). */
export const CAP = 30_000;
/** How long a compaction waits to ask again after its model failed (an outage only delays a summary). */
export const AGAIN = 60_000;

/** A ruler, not a sample line: the compactor copied a sample's content. */
const SCALE = "-".repeat(NODE);

export const WAIT = "(not summarized yet: zoom it)";

/** A compaction's task before its `<input>`: one message to compress (level 0), or two lines to merge. */
const LEAF = `Compaction: compress message {id} into one line of at most ${NODE} bytes
(about 70 words), the length of this ruler:
${SCALE}
<input>
`;

const MERGE = `Compaction: merge lines {a} and {b}, adjacent, into one line of at most
${NODE} bytes (about 70 words), the length of this ruler:
${SCALE}
<chat> may hold their messages, {id} to {end}, in more detail: take details
of them from there too.
<input>
`;

/** Node (l,i), as a line of a view. */
export type Part = { readonly l: number; readonly i: number };

/** A text block of a call's message; `mark`: a cache entry ends after it. */
export type Block = { readonly text: string; readonly mark: boolean };

const encoder = new TextEncoder();
const decoder = new TextDecoder();

export function bytes(text: string): number {
  return encoder.encode(text).length;
}

/** Node (l,i)'s key: an in-order position, unique across levels. */
export function slot(l: number, i: number): number {
  return (2 * i + 1) * 2 ** l - 1;
}

/** The first and last message a node covers. */
export function span(part: Part): [number, number] {
  const a = part.i * 2 ** part.l;
  return [a, a + 2 ** part.l - 1];
}

/** The message a node's compaction view reaches: its own for a leaf, else the end of what it covers. */
export function ctx(l: number, i: number): number {
  return l === 0 ? i : (i + 1) * 2 ** l;
}

export function halves(part: Part): Part[] {
  return part.l === 0
    ? []
    : [
        { l: part.l - 1, i: 2 * part.i },
        { l: part.l - 1, i: 2 * part.i + 1 },
      ];
}

/** The tree's roots: a node per power of two in the count, largest first. */
export function roots(count: number): Part[] {
  const out: Part[] = [];
  for (let l = 30, id = 0; l >= 0; l--) {
    if (id + 2 ** l <= count) {
      out.push({ l, i: id / 2 ** l });
      id += 2 ** l;
    }
  }
  return out;
}

/** Newlines as spaces; most lines have none. */
export function flat(text: string): string {
  return text.includes("\n") ? text.replace(/\s*\n\s*/g, " ") : text;
}

/** A node's zoom target, "id+n". */
export function ref(part: Part): string {
  return `${part.i * 2 ** part.l}+${2 ** part.l}`;
}

export function line(part: Part, text: string): string {
  return `${ref(part)}|${flat(text)}`;
}

/** A text's first NODE bytes, without a character cut in half. */
export function cut(text: string): string {
  return decoder.decode(encoder.encode(text).subarray(0, NODE)).replace(/\uFFFD+$/, "");
}

/** The note sent after a line over NODE bytes. */
export function over(text: string): string {
  return `Too long: your last line for this <input> was ${bytes(text)} bytes,
over the ${NODE}-byte limit. Write the whole line again, cutting just
enough of the least valuable items to fit before this cut:
${cut(text)}| ← LIMIT`;
}

/** Node (l,i)'s compaction task, but its input. */
export function task(l: number, i: number): string {
  const [a, z] = span({ l, i });
  const n = 2 ** l / 2;
  const fill: [string, string][] = [
    ["{a}", `${a}+${n}`],
    ["{b}", `${a + n}+${n}`],
    ["{id}", String(a)],
    ["{end}", String(z)],
  ];
  return fill.reduce((t, [k, v]) => t.replace(k, v), l === 0 ? LEAF : MERGE);
}

/** Where the page of a text from `at` ends: CAP characters on, after its last newline if it has one. */
function pageEnd(text: string, at: number): number {
  if (text.length - at <= CAP) return text.length;
  const nl = text.lastIndexOf("\n", at + CAP - 1);
  const low = text.charCodeAt(at + CAP) >> 10 === 0x37;
  return nl >= at ? nl + 1 : at + CAP - (low ? 1 : 0);
}

/** A text in pages of at most CAP characters. */
export function pages(text: string): string[] {
  const out: string[] = [];
  let at = 0;
  do {
    const end = pageEnd(text, at);
    out.push(text.slice(at, end));
    at = end;
  } while (at < text.length);
  return out;
}

/** A text's page from `at`, saying where to go on. */
export function page(text: string, at: number): string {
  const end = pageEnd(text, at);
  const on = `\n[characters ${at + 1}-${end} of ${text.length}; go on with at ${end}]`;
  return text.slice(at, end) + (end < text.length ? on : "");
}
