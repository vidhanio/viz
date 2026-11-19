import { Buffer } from "node:buffer";

import type { Result } from "../protocol.ts";
import { boundedText, command, DIFF_OPTIONS, git } from "./commands.ts";

type File = Result<"git.diff">["files"][number];
type Change = { file: File; sections: number };
const PATCH_BYTES = 200_000;
const PATCH_HEADER = Buffer.from("\ndiff --git ");

// Raw output supplies exact paths and change kinds; numstat supplies counts even for truncated patches.
function changes(output: string) {
  const records = output.split("\0");
  const changes: Change[] = [];
  let pos = 0;
  while (records[pos]?.startsWith(":")) {
    const header = records[pos++]!;
    const code = header[header.lastIndexOf(" ") + 1];
    const old = records[pos++]!;
    const renamed = code === "R";
    const path = renamed ? records[pos++]! : old;
    const status = code === "A" ? "added" : code === "D" ? "deleted" : renamed ? "renamed" : "modified";
    // Git represents a type change as two adjacent patch sections (delete and add).
    changes.push({
      sections: code === "T" ? 2 : 1,
      file: { path, oldPath: renamed ? old : null, status, additions: 0, deletions: 0, patch: "" },
    });
  }
  for (const { file } of changes) {
    const record = records[pos++]!;
    const firstTab = record.indexOf("\t");
    const secondTab = record.indexOf("\t", firstTab + 1);
    file.additions = Number(record.slice(0, firstTab)) || 0; // Numstat uses '-' for binary files.
    file.deletions = Number(record.slice(firstTab + 1, secondTab)) || 0;
    if (secondTab === record.length - 1) pos += 2; // A rename has two NUL-delimited paths instead.
  }
  return changes;
}

async function readPatches(stdout: ReadableStream<Uint8Array>, changes: Change[], path: string | undefined) {
  let current = -1;
  let sections = 0;
  let file: File | undefined;
  let length = 0;
  const chunks: Buffer[] = [];
  const append = (chunk: Buffer) => {
    if (!file || length > PATCH_BYTES) return;
    const size = Math.min(chunk.length, PATCH_BYTES + 1 - length);
    if (size === 0) return;
    chunks.push(chunk.subarray(0, size));
    length += size;
  };
  const finish = () => {
    if (file) file.patch = boundedText(Buffer.concat(chunks, length), PATCH_BYTES);
    chunks.length = 0;
    length = 0;
  };
  let tail: Buffer = Buffer.from("\n"); // Also recognize the first patch header, with no preceding newline.
  for await (const bytes of stdout) {
    const chunk = Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const buffer = tail.length ? Buffer.concat([tail, chunk]) : chunk;
    let start = 0;
    for (let marker = buffer.indexOf(PATCH_HEADER); marker !== -1; marker = buffer.indexOf(PATCH_HEADER, start)) {
      append(buffer.subarray(start, marker + 1));
      start = marker + 1;
      if (sections > 0) {
        sections--;
        continue;
      }
      finish();
      const change = changes[++current];
      if (!change) throw new Error("Unexpected Git patch section");
      sections = change.sections - 1;
      file = path === undefined || path === change.file.path || path === change.file.oldPath ? change.file : undefined;
    }
    // Keep only a possible partial header, not the rest of a potentially huge line.
    const end = Math.max(start, buffer.length - PATCH_HEADER.length + 1);
    append(buffer.subarray(start, end));
    tail = buffer.subarray(end);
  }
  append(tail);
  finish();
  if (current !== changes.length - 1 || sections !== 0) throw new Error("Incomplete Git patch");
}

export async function diffTrees(
  cwd: string,
  from: string,
  to: string,
  path: string | undefined,
): Promise<Result<"git.diff">> {
  // Both commands compare the same immutable trees with the same rename detection and ordering.
  const args = [...DIFF_OPTIONS, from, to, "--"];
  const entries = changes(await git(cwd, ["diff", "--raw", "--numstat", "-z", ...args]));
  const files = entries.flatMap(({ file }) =>
    path === undefined || file.path === path || file.oldPath === path ? [file] : [],
  );
  if (files.length) {
    const { stdout, done } = command(cwd, ["git", "--no-optional-locks", "diff", "--patch", ...args]);
    await Promise.all([readPatches(stdout, entries, path), done]);
  }
  return { files };
}
