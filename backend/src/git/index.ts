import { createHash } from "node:crypto";
import { lstat, mkdir } from "node:fs/promises";
import { resolve } from "node:path";

import type { Context } from "@earendil-works/chord/context";
import { type ConversationId, defineDoc, type EntryId, type Harness } from "@earendil-works/pi-durable";

import type { GitState, Params, Project, Result } from "../protocol.ts";
import { command, git, optionalGit, repositoryRoot, stagedDiff, withIndex } from "./commands.ts";
import { diffTrees } from "./diff.ts";

type Snapshot = { root: string; commit: string };
const Checkpoints = defineDoc<{ prompts: Record<string, Snapshot> }>({
  kind: "viz.checkpoints",
  version: 1,
  scope: "conversation",
  history: "latest",
  fork: "initial",
  initial: () => ({ prompts: {} }),
  checkpointWhen: (_value, _ops, { deltasSinceBase }) => deltasSinceBase >= 32,
});

const COMMIT_DIFF_BYTES = 32_000;
const CHECKPOINT_AUTHOR = {
  GIT_AUTHOR_NAME: "viz",
  GIT_COMMITTER_NAME: "viz",
  GIT_AUTHOR_EMAIL: "viz@localhost",
  GIT_COMMITTER_EMAIL: "viz@localhost",
};
const STATUS: Record<string, GitState["files"][number]["status"]> = {
  A: "added",
  C: "added",
  D: "deleted",
  R: "renamed",
};

function parseState(output: string, worktree: string | null): GitState {
  const state: GitState = { branch: null, upstream: null, ahead: 0, behind: 0, worktree, files: [] };
  const records = output.split("\0");
  for (let i = 0; i < records.length; i++) {
    const line = records[i]!;
    if (line.startsWith("# branch.head ")) {
      const head = line.slice(14);
      state.branch = head === "(detached)" ? null : head;
    } else if (line.startsWith("# branch.upstream ")) {
      state.upstream = line.slice(18);
    } else if (line.startsWith("# branch.ab ")) {
      const [ahead, behind] = line.slice(12).split(" ");
      state.ahead = Number(ahead) || 0;
      state.behind = Math.abs(Number(behind)) || 0;
    } else if (line.startsWith("? ")) {
      state.files.push({ path: line.slice(2), staged: false, status: "untracked" });
    } else if (line.startsWith("u ")) {
      const fields = /^(?:[^ ]+ ){10}/.exec(line);
      if (fields) state.files.push({ path: line.slice(fields[0].length), staged: false, status: "conflicted" });
    } else if (line.startsWith("1 ") || line.startsWith("2 ")) {
      const rename = line[0] === "2";
      const fields = (rename ? /^(?:[^ ]+ ){9}/ : /^(?:[^ ]+ ){8}/).exec(line);
      if (fields) {
        const staged = line[2] !== ".";
        state.files.push({
          path: line.slice(fields[0].length),
          staged,
          status: STATUS[line[staged ? 2 : 3]!] ?? "modified",
        });
      }
      if (rename) i++; // The second NUL record is the source path, not another change.
    }
  }
  return state;
}

async function branchName(cwd: string, branch: string) {
  if (!branch || branch.startsWith("-") || branch.includes("\0")) throw new Error("Invalid branch name");
  const name = await git(cwd, ["check-ref-format", "--branch", branch]);
  if (name.slice(0, -1) !== branch) throw new Error("Use a branch name, not a checkout expression");
}

async function config(cwd: string, key: string) {
  return (await optionalGit(cwd, ["config", "--get", key]))?.slice(0, -1);
}

export class Git {
  readonly #refNamespace: string;

  constructor(
    private readonly stateDir: string,
    private readonly harness: Harness,
    private readonly generateMessage: (diff: string, context: Context) => Promise<string>,
  ) {
    const namespace = createHash("sha256").update(resolve(stateDir)).digest("hex").slice(0, 24);
    this.#refNamespace = `refs/viz/checkpoints/${namespace}`;
  }

  async root(cwd: string): Promise<string | null> {
    return repositoryRoot(cwd).catch(() => null);
  }

  async state(cwd: string, worktree: string | null): Promise<GitState | null> {
    const root = await this.root(cwd);
    if (root === null) return null;
    const output = await git(root, [
      "status",
      "--porcelain=v2",
      "-z",
      "--branch",
      "--ahead-behind",
      "--find-renames",
      "--untracked-files=all",
      "--ignore-submodules=none",
    ]);
    return parseState(output, worktree);
  }

  async createWorktree(
    project: Project,
    spec: NonNullable<Params<"conversation.create">["worktree"]>,
  ): Promise<{ path: string; base: string }> {
    if (!/^[a-zA-Z0-9][a-zA-Z0-9_-]*$/.test(project.id)) throw new Error("Invalid project id");
    const root = await repositoryRoot(project.path);
    await branchName(root, spec.branch);
    let requested = spec.base;
    if (requested === undefined) {
      requested = (await optionalGit(root, ["symbolic-ref", "--quiet", "HEAD"]))?.slice(0, -1) ?? "HEAD";
    } else {
      if (!requested || requested.startsWith("-") || requested.includes("\0")) throw new Error("Invalid base ref");
      if ((await optionalGit(root, ["check-ref-format", "--allow-onelevel", requested])) === null) {
        throw new Error("Invalid base ref");
      }
    }
    const commit = (await git(root, ["rev-parse", "--verify", "--end-of-options", `${requested}^{commit}`])).trimEnd();
    const ref = (
      await git(root, ["rev-parse", "--symbolic-full-name", "--verify", "--end-of-options", requested])
    ).slice(0, -1);
    const base = ref.startsWith("refs/") ? ref : commit;
    let slug = spec.branch.replace(/[^a-zA-Z0-9_.-]+/g, "-").slice(0, 80);
    if (slug !== spec.branch) slug += `~${createHash("sha256").update(spec.branch).digest("hex").slice(0, 12)}`;
    const parent = resolve(this.stateDir, "worktrees", project.id);
    const path = resolve(parent, slug);
    await mkdir(parent, { recursive: true, mode: 0o700 });
    const exists = await lstat(path).catch((error: NodeJS.ErrnoException) => {
      if (error.code !== "ENOENT") throw error;
      return null;
    });
    if (exists) throw new Error("Worktree path already exists");
    await git(root, ["worktree", "add", "--no-track", "-b", spec.branch, "--", path, commit]);
    return { path, base };
  }

  async checkpoint(id: ConversationId, prompt: EntryId, cwd: string, context: Context): Promise<void> {
    const root = await this.root(cwd);
    if (root === null) return;
    const key = String(prompt);
    if ((await this.harness.snapshot(Checkpoints, id, context))?.prompts[key]) return;
    const tree = await withIndex(root, async (index) => (await git(root, ["write-tree"], index)).trimEnd());
    const commit = (
      await git(root, ["commit-tree", tree, "--no-gpg-sign", "-m", `viz checkpoint ${id}/${prompt}`], {
        env: CHECKPOINT_AUTHOR,
      })
    ).trimEnd();
    // A database row alone does not protect the snapshot from Git garbage collection.
    await git(root, ["update-ref", `${this.#refNamespace}/${id}/${prompt}`, commit]);
    await this.harness.commit(async (tx) => {
      const { prompts } = await tx.doc(Checkpoints, id);
      prompts[key] = { root, commit };
    }, context);
  }

  async diff(
    id: ConversationId,
    cwd: string,
    base: string | undefined,
    spec: Pick<Params<"git.diff">, "turn" | "path" | "scope">,
    context: Context,
  ): Promise<Result<"git.diff">> {
    const root = await repositoryRoot(cwd);
    let from: string;
    let to: string | undefined;
    if (spec.scope === "branch") {
      const head = (await git(root, ["rev-parse", "--verify", "HEAD^{commit}"])).trimEnd();
      const refs = base === undefined ? ["refs/remotes/origin/HEAD"] : [base];
      if (base === undefined) {
        const branch = (await optionalGit(root, ["symbolic-ref", "--quiet", "HEAD"]))?.slice(0, -1);
        const upstream = branch && (await git(root, ["for-each-ref", "--format=%(upstream)", branch])).slice(0, -1);
        if (upstream) refs.push(upstream);
        refs.push("refs/heads/main", "refs/heads/master");
      }
      let target: string | null = null;
      for (const ref of refs) {
        target = await optionalGit(root, ["rev-parse", "--verify", "--quiet", "--end-of-options", `${ref}^{commit}`]);
        if (target !== null) break;
      }
      if (target === null) throw new Error(base === undefined ? "No base branch is configured" : "Base ref is missing");
      const mergeBase = await optionalGit(root, ["merge-base", target.trimEnd(), head]);
      if (mergeBase === null) throw new Error("Branch and base have no common history");
      from = mergeBase.trimEnd();
    } else if (spec.scope === "turn") {
      const prompts = (await this.harness.snapshot(Checkpoints, id, context))?.prompts;
      let turn = spec.turn;
      let next: number | undefined;
      for (const key in prompts ?? {}) {
        const candidate = Number(key);
        if (spec.turn === undefined) {
          if (turn === undefined || candidate > turn) turn = candidate;
        } else if (candidate > spec.turn && (next === undefined || candidate < next)) {
          next = candidate;
        }
      }
      if (turn === undefined) return { files: [] };
      from = checkpointAt(prompts, root, turn).commit;
      if (next !== undefined) to = checkpointAt(prompts, root, next).commit;
    } else {
      const head = await optionalGit(root, ["rev-parse", "--verify", "--quiet", "HEAD^{commit}"]);
      from = (head ?? (await git(root, ["hash-object", "-w", "-t", "tree", "--stdin"], { input: "" }))).trimEnd();
    }
    to ??= await withIndex(root, async (index) => (await git(root, ["write-tree"], index)).trimEnd());
    return diffTrees(root, from, to, spec.path);
  }

  async branches(cwd: string): Promise<Result<"git.branches">> {
    const root = await repositoryRoot(cwd);
    const [current, names] = await Promise.all([
      optionalGit(root, ["symbolic-ref", "--quiet", "HEAD"]),
      git(root, ["for-each-ref", "--format=%(refname:lstrip=2)", "refs/heads/"]),
    ]);
    return {
      current: current?.slice("refs/heads/".length, -1) ?? null,
      branches: names.split("\n").filter(Boolean),
    };
  }

  async checkout(cwd: string, branch: string, create?: boolean): Promise<void> {
    const root = await repositoryRoot(cwd);
    await branchName(root, branch);
    await git(root, create ? ["switch", "--no-track", "--create", branch] : ["switch", "--no-guess", "--", branch]);
  }

  async commit(
    cwd: string,
    message: string | undefined,
    all: boolean | undefined,
    context: Context,
  ): Promise<Result<"git.commit">> {
    const root = await repositoryRoot(cwd);
    if (all !== false) await git(root, ["add", "--all"]);
    let text = message?.trim();
    if (!text) {
      const diff = await stagedDiff(root, COMMIT_DIFF_BYTES);
      if (!diff) throw new Error("No staged changes");
      text = (await this.generateMessage(diff, context)).trim();
      if (!text) throw new Error("No commit message was generated");
    }
    await git(root, ["commit", "--file=-"], { input: text });
    const result = await git(root, ["log", "-1", "--format=%H%x00%B"]);
    const boundary = result.indexOf("\0");
    return { commit: result.slice(0, boundary), message: result.slice(boundary + 1).trimEnd() };
  }

  async push(cwd: string): Promise<void> {
    const root = await repositoryRoot(cwd);
    const ref = (await optionalGit(root, ["symbolic-ref", "--quiet", "HEAD"]))?.slice(0, -1);
    if (!ref?.startsWith("refs/heads/")) throw new Error("Cannot push a detached HEAD");
    const branch = ref.slice("refs/heads/".length);
    const [pushRemote, defaultRemote, upstreamRemote, upstreamRef] = await Promise.all([
      config(root, `branch.${branch}.pushRemote`),
      config(root, "remote.pushDefault"),
      config(root, `branch.${branch}.remote`),
      config(root, `branch.${branch}.merge`),
    ]);
    let remote = pushRemote || defaultRemote || upstreamRemote;
    if (!remote) {
      const remotes = (await git(root, ["remote"])).split("\n").filter(Boolean);
      remote = remotes.includes("origin") ? "origin" : remotes.length === 1 ? remotes[0] : undefined;
      if (!remote) throw new Error(remotes.length ? "No push remote is configured" : "No Git remotes are configured");
    }
    const upstream = upstreamRemote && upstreamRef;
    const destination = remote === upstreamRemote && upstreamRef ? upstreamRef : ref;
    await git(root, ["push", ...(upstream ? [] : ["--set-upstream"]), "--", remote, `${ref}:${destination}`]);
  }

  async pr(cwd: string, spec: Omit<Params<"git.pr">, "conversation">): Promise<Result<"git.pr">> {
    const root = await repositoryRoot(cwd);
    if (!Bun.which("gh")) throw new Error("GitHub CLI (gh) is not installed");
    const args = ["gh", "pr", "create", "--fill"];
    if (spec.title !== undefined) args.push(`--title=${spec.title}`);
    if (spec.body !== undefined) args.push("--body-file=-");
    if (spec.draft) args.push("--draft");
    const { stdout, done } = command(root, args, spec.body === undefined ? {} : { input: spec.body });
    const [output] = await Promise.all([stdout.text(), done]);
    const url = output
      .trim()
      .split("\n")
      .findLast((line) => /^https?:\/\/\S+$/.test(line));
    if (!url) throw new Error("GitHub CLI did not return a pull request URL");
    return { url };
  }

  async revert(id: ConversationId, cwd: string, turn: number, context: Context): Promise<void> {
    const root = await repositoryRoot(cwd);
    const prompts = (await this.harness.snapshot(Checkpoints, id, context))?.prompts;
    const snapshot = checkpointAt(prompts, root, turn);
    await withIndex(root, async (index) => {
      const current = (await git(root, ["write-tree"], index)).trimEnd();
      const target = (await git(root, ["rev-parse", "--verify", `${snapshot.commit}^{tree}`])).trimEnd();
      if (current === target) return;
      // Files added after the prompt are tracked by this temporary index, so restore removes them too.
      await git(root, ["restore", `--source=${target}`, "--worktree", "--no-recurse-submodules", "--", ":/"], index);
    });
  }
}

function checkpointAt(prompts: Record<string, Snapshot> | undefined, cwd: string, turn: number) {
  const snapshot = prompts?.[String(turn)];
  if (!snapshot) throw new Error("No checkpoint for this turn");
  if (snapshot.root !== cwd) throw new Error("Checkpoint belongs to a different worktree");
  return snapshot;
}
