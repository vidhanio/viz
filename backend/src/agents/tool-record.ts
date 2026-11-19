import type { ToolCallUpdate } from "@agentclientprotocol/sdk";

import type { ToolRecord } from "./model.ts";

export function inputText(value: unknown): string | undefined {
  return JSON.stringify(value, null, 2)?.slice(0, 4000);
}

export class ToolContent {
  record: ToolRecord;
  #rawOutput = "";
  #contentText = "";

  constructor(id: string) {
    this.record = { id, title: id, kind: null, status: "pending" };
  }

  update(update: ToolCallUpdate): ToolRecord {
    const record = this.record;
    record.title = update.title ?? record.title;
    record.kind = update.kind ?? record.kind;
    record.status = update.status ?? record.status;
    if (update.rawInput !== undefined) record.input = inputText(update.rawInput)!;
    if (update.rawOutput !== undefined) {
      this.#rawOutput = (
        typeof update.rawOutput === "string" ? update.rawOutput : JSON.stringify(update.rawOutput, null, 2)
      ).slice(0, 8000);
    }
    if (update.content) {
      this.#contentText = "";
      const diffs: NonNullable<ToolRecord["diffs"]> = [];
      for (const item of update.content) {
        if (item.type === "diff") {
          diffs.push({
            path: item.path,
            oldText: item.oldText?.slice(0, 20000) ?? null,
            newText: item.newText.slice(0, 20000),
          });
        } else if (item.type === "content" && item.content.type === "text" && this.#contentText.length < 8000) {
          const separator = this.#contentText ? "\n" : "";
          this.#contentText +=
            separator + item.content.text.slice(0, 8000 - this.#contentText.length - separator.length);
        }
      }
      if (diffs.length) record.diffs = diffs;
      else delete record.diffs;
    }
    if (update.locations) {
      if (update.locations.length) record.locations = [...new Set(update.locations.map(({ path }) => path))];
      else delete record.locations;
    }
    const text = this.#contentText;
    const raw = this.#rawOutput;
    if (text && raw && text !== raw) record.output = `${text}\n${raw}`.slice(0, 8000);
    else if (text || raw) record.output = text || raw;
    else delete record.output;
    return record;
  }
}
