import { randomUUID } from "node:crypto";

import type { RequestPermissionRequest, RequestPermissionResponse } from "@agentclientprotocol/sdk";
import type { ConversationId } from "@earendil-works/pi-durable";

import type { Approval } from "../protocol.ts";
import type { Access } from "./options.ts";
import { approvalDetail } from "./tool-record.ts";

export const CANCELLED: RequestPermissionResponse = { outcome: { outcome: "cancelled" } };
const selected = (optionId: string): RequestPermissionResponse => ({ outcome: { outcome: "selected", optionId } });

type Pending = { approval: Approval; resolve: (response: RequestPermissionResponse) => void };

export class Approvals {
  readonly #pending = new Map<ConversationId, Map<string, Pending>>();
  readonly #observers = new Set<(id: ConversationId) => void>();

  observe(change: (id: ConversationId) => void): () => void {
    this.#observers.add(change);
    return () => this.#observers.delete(change);
  }

  #changed(id: ConversationId) {
    for (const change of this.#observers) change(id);
  }

  list(id: ConversationId): Approval[] {
    return Array.from(this.#pending.get(id)?.values() ?? [], ({ approval }) => approval);
  }

  request(
    id: ConversationId,
    access: Access,
    { options, toolCall }: RequestPermissionRequest,
  ): Promise<RequestPermissionResponse> {
    if (access !== "ask") {
      const kinds = access === "full" ? ["allow_once", "allow_always"] : ["reject_once", "reject_always"];
      for (const kind of kinds) {
        const option = options.find((item) => item.kind === kind);
        if (option) return Promise.resolve(selected(option.optionId));
      }
      return Promise.resolve(CANCELLED);
    }
    const approval: Approval = {
      id: randomUUID(),
      title: toolCall.title ?? toolCall.name ?? "Permission request",
      kind: toolCall.kind ?? null,
      detail: approvalDetail(toolCall),
      options: options.map(({ optionId, name, kind }) => ({ id: optionId, name, kind })),
    };
    const { promise, resolve } = Promise.withResolvers<RequestPermissionResponse>();
    let pending = this.#pending.get(id);
    if (!pending) this.#pending.set(id, (pending = new Map()));
    pending.set(approval.id, { approval, resolve });
    this.#changed(id);
    return promise;
  }

  respond(id: ConversationId, approvalId: string, option: string | null) {
    const pending = this.#pending.get(id);
    const item = pending?.get(approvalId);
    if (!item || !pending) throw new Error("Approval not found");
    if (option !== null && !item.approval.options.some(({ id }) => id === option))
      throw new Error("Invalid approval option");
    pending.delete(approvalId);
    if (!pending.size) this.#pending.delete(id);
    item.resolve(option === null ? CANCELLED : selected(option));
    this.#changed(id);
  }

  cancel(id: ConversationId) {
    const pending = this.#pending.get(id);
    if (!pending) return;
    this.#pending.delete(id);
    for (const item of pending.values()) item.resolve(CANCELLED);
    this.#changed(id);
  }
}
