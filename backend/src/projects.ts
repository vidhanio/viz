import { randomUUID } from "node:crypto";
import { realpath, stat } from "node:fs/promises";
import { homedir } from "node:os";
import { basename, join, resolve } from "node:path";

import type { Context } from "@earendil-works/chord/context";
import { type ConversationId, type Cursor, defineDoc, type Harness, type Tx } from "@earendil-works/pi-durable";

import type { Params, Project } from "./protocol.ts";

export const MetaDoc = defineDoc<{
  project: string | null;
  parent: ConversationId | null;
  archived: boolean;
  pinned?: boolean;
  unread?: boolean;
  /**
   * Deleted by the user: hidden from every list, search, tool, and RPC. pi-durable cannot remove a conversation, so
   * this tombstone is the deletion; its transcript stays in `viz.sqlite`.
   */
  deleted?: boolean;
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

export const ProjectsDoc = defineDoc<{ projects: Project[] }>({
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

/** Whether `id` is deleted; deleted conversations behave as if they did not exist. */
export async function isDeleted(harness: Harness, id: ConversationId, context: Context) {
  return (await harness.snapshot(MetaDoc, id, context))?.deleted === true;
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

export type ProjectStore = {
  list(context: Context): Promise<Project[]>;
  create(path: string, name: string | undefined, context: Context): Promise<Project>;
  update(id: string, changes: Omit<Params<"project.update">, "project">, context: Context): Promise<Project>;
  remove(id: string, move: (tx: Tx, id: ConversationId) => Promise<void>, context: Context): Promise<void>;
};

export function projects(harness: Harness, root: ConversationId): ProjectStore {
  return {
    async list(context: Context): Promise<Project[]> {
      const state = await harness.snapshot(ProjectsDoc, root, context);
      return state?.projects ?? [];
    },
    async create(path: string, name: string | undefined, context: Context) {
      const absolute = resolve(
        path === "~" ? homedir() : path.startsWith("~/") ? join(homedir(), path.slice(2)) : path,
      );
      const canonical = await realpath(absolute).catch(() => null);
      if (canonical === null || !(await stat(canonical)).isDirectory())
        throw new Error("Project path is not a directory");
      const project: Project = {
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
      return project;
    },
    async update(id: string, { name }, context: Context) {
      return harness.commit(async (tx) => {
        const project = await projectIn(tx, root, id);
        if (name !== undefined) project.name = name;
        return { ...project };
      }, context);
    },
    async remove(id: string, move: (tx: Tx, id: ConversationId) => Promise<void>, context: Context) {
      await harness.commit(async (tx) => {
        const doc = await tx.doc(ProjectsDoc, root);
        const index = doc.projects.findIndex((project) => project.id === id);
        if (index < 0) throw new Error("Project not found");
        for await (const record of conversationRecords(tx)) {
          const meta = await tx.doc(MetaDoc, record.id);
          if (!meta.deleted && meta.project === id) await move(tx, record.id);
        }
        doc.projects.splice(index, 1);
      }, context);
    },
  };
}
