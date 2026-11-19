import { opendir } from "node:fs/promises";
import { basename, join } from "node:path";

import fuzzysort, { type Prepared } from "fuzzysort";

const WALK_LIMIT = 20_000;
const SKIP_DIRS: Record<string, true> = { node_modules: true, target: true };

async function listFiles(cwd: string): Promise<string[]> {
  if (Bun.which("git")) {
    const git = Bun.spawn(["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"], {
      cwd,
      stdout: "pipe",
      stderr: "ignore",
    });
    const [output, status] = await Promise.all([git.stdout.text(), git.exited]);
    if (status === 0) return [...new Set(output.split("\0").filter(Boolean))];
  }

  const files: string[] = [];
  const dirs = [""];
  let visited = 0;
  while (dirs.length) {
    const dir = dirs.pop()!;
    const entries = await opendir(join(cwd, dir)).catch(() => undefined);
    if (!entries) continue;
    for await (const entry of entries) {
      if (++visited > WALK_LIMIT) return files;
      const path = join(dir, entry.name);
      if (entry.isDirectory()) {
        if (!entry.name.startsWith(".") && !Object.hasOwn(SKIP_DIRS, entry.name)) dirs.push(path);
      } else if (entry.isFile() || entry.isSymbolicLink()) files.push(path);
    }
  }
  return files;
}

type Candidate = { file: string; path: Prepared; name: Prepared };

export class FileSearch {
  readonly #cache = new Map<string, { expires: number; files: Promise<Candidate[]> }>();

  async search(cwd: string, text: string, limit = 50): Promise<string[]> {
    const now = Date.now();
    for (const [path, cached] of this.#cache) if (cached.expires <= now) this.#cache.delete(path);
    let cached = this.#cache.get(cwd);
    if (!cached) {
      const files = listFiles(cwd)
        .then((files) =>
          files
            .sort((a, b) => a.length - b.length || a.localeCompare(b))
            .map((file) => ({ file, path: fuzzysort.prepare(file), name: fuzzysort.prepare(basename(file)) })),
        )
        .catch((error: unknown) => {
          if (this.#cache.get(cwd)?.files === files) this.#cache.delete(cwd);
          throw error;
        });
      cached = { expires: now + 10_000, files };
      this.#cache.set(cwd, cached);
    }
    const files = await cached.files;
    const query = text.trim();
    if (!query) return files.slice(0, limit).map(({ file }) => file);
    const names: string[] = [];
    const paths: string[] = [];
    for (const { file, path, name } of files) {
      if (fuzzysort.single(query, name)) {
        names.push(file);
        if (names.length === limit) return names;
      } else if (paths.length < limit && fuzzysort.single(query, path)) paths.push(file);
    }
    names.push(...paths.slice(0, limit - names.length));
    return names;
  }
}
