import { Buffer } from "node:buffer";
import { copyFile, mkdtemp, realpath, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

class CommandError extends Error {
  constructor(
    readonly code: number,
    message: string,
  ) {
    super(message);
  }
}

type CommandOptions = { env?: Record<string, string>; input?: string };

export function command(cwd: string, argv: string[], { env, input }: CommandOptions = {}) {
  const child = Bun.spawn(argv, {
    cwd,
    env: { ...process.env, ...env, GIT_TERMINAL_PROMPT: "0", GH_PROMPT_DISABLED: "1" },
    stdin: input === undefined ? "ignore" : Buffer.from(input),
    stdout: "pipe",
    stderr: "pipe",
  });
  const done = Promise.all([child.exited, child.stderr.text()]).then(([code, stderr]) => {
    if (code !== 0)
      throw new CommandError(code, stderr.split(/\r?\n/, 1)[0]?.trim() || `${argv[0]} exited with ${code}`);
  });
  return { stdout: child.stdout, done };
}

export async function git(cwd: string, args: string[], options?: CommandOptions) {
  const { stdout, done } = command(cwd, ["git", "--no-optional-locks", ...args], options);
  const [output] = await Promise.all([stdout.text(), done]);
  return output;
}

export async function optionalGit(cwd: string, args: string[], absentCode = 1) {
  try {
    return await git(cwd, args);
  } catch (error) {
    if (error instanceof CommandError && error.code === absentCode) return null;
    throw error;
  }
}

export async function repositoryRoot(cwd: string) {
  const path = await git(cwd, ["rev-parse", "--show-toplevel"]);
  return realpath(path.slice(0, -1)); // Git appends exactly one LF, even if the directory name contains one.
}

export async function withIndex<T>(cwd: string, run: (index: CommandOptions) => Promise<T>) {
  const dir = await mkdtemp(join(tmpdir(), "viz-git-"));
  const file = join(dir, "index");
  const index = { env: { GIT_INDEX_FILE: file } };
  try {
    // Start from the set of files Git actually tracks, including staged additions that now match an ignore rule.
    const path = await git(cwd, ["rev-parse", "--path-format=absolute", "--git-path", "index"]);
    try {
      await copyFile(path.slice(0, -1), file);
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
      const head = await optionalGit(cwd, ["rev-parse", "--verify", "--quiet", "HEAD"]);
      await git(cwd, ["read-tree", head === null ? "--empty" : head.trimEnd()], index);
    }
    await git(cwd, ["-c", "core.splitIndex=false", "-c", "core.fsmonitor=false", "add", "--all"], index);
    return await run(index);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
}

export const DIFF_OPTIONS = [
  "--no-color",
  "--no-ext-diff",
  "--no-textconv",
  "--no-relative",
  "--default-prefix",
  "--submodule=short",
  "--ignore-submodules=dirty",
  "--find-renames",
  "--output-indicator-new=+",
  "--output-indicator-old=-",
  "--output-indicator-context= ",
];

const decoder = new TextDecoder("utf-8", { ignoreBOM: true });

export function boundedText(bytes: Buffer, limit: number) {
  // A cut through a UTF-8 code point must not emit a replacement character past the byte limit.
  let end = Math.min(bytes.length, limit);
  if (end < bytes.length) while (end > 0 && (bytes[end]! & 0xc0) === 0x80) end--;
  const text = decoder.decode(bytes.subarray(0, end));
  if (Buffer.byteLength(text) <= limit) return text;
  // Non-UTF-8 source bytes can expand to three bytes when decoded as replacement characters.
  const buffer = Buffer.allocUnsafe(limit);
  const { written } = new TextEncoder().encodeInto(text, buffer);
  return decoder.decode(buffer.subarray(0, written));
}

export async function stagedDiff(cwd: string, limit: number) {
  const { stdout, done } = command(cwd, ["git", "--no-optional-locks", "diff", ...DIFF_OPTIONS, "--cached", "--patch"]);
  const [patch] = await Promise.all([
    (async () => {
      const chunks: Buffer[] = [];
      let length = 0;
      for await (const chunk of stdout) {
        const size = Math.min(chunk.length, limit + 1 - length);
        if (size > 0) chunks.push(Buffer.from(chunk.buffer, chunk.byteOffset, size));
        length += size;
      }
      return boundedText(Buffer.concat(chunks, length), limit);
    })(),
    done,
  ]);
  return patch;
}
