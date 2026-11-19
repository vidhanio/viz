import { randomUUID } from "node:crypto";
import { realpath, stat } from "node:fs/promises";
import { homedir } from "node:os";
import { basename, join, resolve } from "node:path";

import type { Context } from "@earendil-works/chord/context";
import { type ConversationId, type Cursor, defineDoc, type Harness, type Tx } from "@earendil-works/pi-durable";

import type { Project } from "./protocol.ts";

export const MetaDoc = defineDoc<{
  project: string | null;
  parent: number | null;
  archived: boolean;
  createdAt: number;
  updatedAt: number;
}>({
  kind: "viz.meta",
  version: 1,
  scope: "conversation",
  history: "latest",
  fork: "initial",
  initial: () => ({ project: null, parent: null, archived: false, createdAt: 0, updatedAt: 0 }),
  checkpointWhen: () => true,
});

type StoredProject = Omit<Project, "branch">;
export const ProjectsDoc = defineDoc<{ projects: StoredProject[] }>({
  kind: "viz.projects",
  version: 1,
  scope: "conversation",
  history: "latest",
  fork: "initial",
  initial: () => ({ projects: [] }),
  checkpointWhen: () => true,
});

export async function* conversationRecords(tx: Tx) {
  let cursor: Cursor | undefined;
  do {
    const page = await tx.scanConversations({}, 256, cursor);
    yield* page.items;
    cursor = page.next;
  } while (cursor !== undefined);
}

export async function touch(tx: Tx, id: ConversationId) {
  const meta = await tx.doc(MetaDoc, id);
  meta.updatedAt = Date.now();
  if (!meta.createdAt) meta.createdAt = meta.updatedAt;
}

export async function projectIn(tx: Tx, root: ConversationId, id: string) {
  const found = (await tx.doc(ProjectsDoc, root)).projects.find((p) => p.id === id);
  if (!found) throw new Error("Project not found");
  return found;
}

async function withBranch(project: StoredProject): Promise<Project> {
  let branch: string | null = null;
  try {
    // Git resolves the .git/HEAD of both ordinary checkouts and linked worktrees.
    await stat(join(project.path, ".git"));
    const git = Bun.spawn(["git", "symbolic-ref", "--quiet", "HEAD"], {
      cwd: project.path,
      stdout: "pipe",
      stderr: "ignore",
    });
    const [ref, status] = await Promise.all([git.stdout.text(), git.exited]);
    if (status === 0 && ref.startsWith("refs/heads/")) branch = ref.trimEnd().slice("refs/heads/".length);
  } catch {
    // No readable branch.
  }
  return { ...project, branch };
}

export type ProjectStore = {
  list(context: Context): Promise<Project[]>;
  create(path: string, name: string | undefined, context: Context): Promise<Project>;
  update(id: string, name: string, context: Context): Promise<Project>;
  remove(id: string, move: (tx: Tx, id: ConversationId) => Promise<void>, context: Context): Promise<void>;
};

export function projects(harness: Harness, root: ConversationId): ProjectStore {
  return {
    async list(context: Context): Promise<Project[]> {
      const state = await harness.snapshot(ProjectsDoc, root, context);
      return Promise.all((state?.projects ?? []).map(withBranch));
    },
    async create(path: string, name: string | undefined, context: Context) {
      const absolute = resolve(
        path === "~" ? homedir() : path.startsWith("~/") ? join(homedir(), path.slice(2)) : path,
      );
      const canonical = await realpath(absolute).catch(() => null);
      if (canonical === null || !(await stat(canonical)).isDirectory())
        throw new Error("Project path is not a directory");
      const project = {
        id: randomUUID(),
        path: canonical,
        name: name ?? (basename(canonical) || canonical),
        createdAt: Date.now(),
      };
      await harness.commit(async (tx) => {
        const { projects } = await tx.doc(ProjectsDoc, root);
        if (projects.some((p) => p.path === canonical)) throw new Error("Project already exists");
        projects.push(project);
      }, context);
      return withBranch(project);
    },
    async update(id: string, name: string, context: Context) {
      const project = await harness.commit(async (tx) => {
        const project = await projectIn(tx, root, id);
        project.name = name;
        return { ...project };
      }, context);
      return withBranch(project);
    },
    async remove(id: string, move: (tx: Tx, id: ConversationId) => Promise<void>, context: Context) {
      await harness.commit(async (tx) => {
        const doc = await tx.doc(ProjectsDoc, root);
        const index = doc.projects.findIndex((project) => project.id === id);
        if (index < 0) throw new Error("Project not found");
        for await (const record of conversationRecords(tx)) {
          if ((await tx.doc(MetaDoc, record.id)).project === id) await move(tx, record.id);
        }
        doc.projects.splice(index, 1);
      }, context);
    },
  };
}
