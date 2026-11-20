import type { ToolCallUpdate } from "@agentclientprotocol/sdk";

import type { ToolRecord } from "./model.ts";

/** Structured input stays JSON, so clients can read fields such as `command`, `path`, or `timeout`. */
export function inputText(value: unknown): string | undefined {
  return (typeof value === "string" ? value : JSON.stringify(value, null, 2))?.slice(0, 4000);
}

const OUTPUT_FIELDS = ["formatted_output", "aggregated_output", "output", "content", "text", "result"] as const;

/**
 * Readable text from ACP `rawOutput`, whose shape is runner-specific: a string, text content blocks, a known text field,
 * or `stdout`/`stderr`; anything else as JSON.
 */
function rawText(value: unknown, depth = 0): string | undefined {
  if (value == null) return "";
  if (typeof value === "string") return value;
  if (typeof value !== "object" || depth > 2) return undefined;
  if (Array.isArray(value)) {
    const parts = value.map((item) => rawText(item, depth + 1));
    return parts.every((part) => part !== undefined) ? parts.join("\n") : undefined;
  }
  const record = value as Record<string, unknown>;
  for (const field of OUTPUT_FIELDS) {
    const text = rawText(record[field], depth + 1);
    if (text) return text;
  }
  const streams = [record.stdout, record.stderr].filter((item): item is string => typeof item === "string" && !!item);
  return streams.length ? streams.join("\n") : undefined;
}

/** ACP and MCP do not share call IDs. Correlate the response itself, not names, titles, or request order. */
export class ToolReceipts {
  readonly #calls = new Map<string, { viz: string; child?: number; claimedBy?: string }>();

  issue = (viz: string, child?: number): string => {
    const id = crypto.randomUUID();
    this.#calls.set(id, { viz, ...(child !== undefined && { child }) });
    // Some runners only forward MCP text blocks, not structured response metadata.
    return `[viz-tool:${id}]`;
  };

  strip(value: string, record: ToolRecord): string {
    if (!this.#calls.size) return value;
    return value.replace(/\[viz-tool:([0-9a-f-]{36})\]/g, (marker: string, id: string) => {
      const call = this.#calls.get(id);
      if (!call) return marker;
      if (call.claimedBy === undefined || call.claimedBy === record.id) {
        call.claimedBy = record.id;
        record.viz = call.viz;
        if (call.child !== undefined) record.child = call.child;
      }
      return "";
    });
  }
}

export class ToolContent {
  record: ToolRecord;
  #rawOutput = "";
  #contentText = "";

  constructor(id: string) {
    this.record = { id, title: id, kind: null, status: "pending" };
  }

  update(update: ToolCallUpdate, receipts: ToolReceipts): ToolRecord {
    const record = this.record;
    record.title = update.title ?? record.title;
    if (update.name != null) record.name = update.name;
    record.kind = update.kind ?? record.kind;
    record.status = update.status ?? record.status;
    if (record.status === "in_progress") record.startedAt ??= Date.now();
    if (update.rawInput != null) {
      const input = inputText(update.rawInput);
      if (input) record.input = input;
      else delete record.input;
    }
    if (update.rawOutput != null) {
      const raw = rawText(update.rawOutput, 0) ?? JSON.stringify(update.rawOutput, null, 2) ?? "";
      this.#rawOutput = receipts.strip(raw, record).slice(0, 8000);
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
        } else if (item.type === "content" && item.content.type === "text") {
          const text = receipts.strip(item.content.text, record);
          if (text && this.#contentText.length < 8000) {
            const separator = this.#contentText ? "\n" : "";
            this.#contentText += separator + text.slice(0, 8000 - this.#contentText.length - separator.length);
          }
        }
      }
      if (diffs.length) record.diffs = diffs;
      else delete record.diffs;
    }
    if (update.locations) {
      if (update.locations.length) record.locations = [...new Set(update.locations.map(({ path }) => path))];
      else delete record.locations;
    }
    // ACP content is the display text; raw output is runner metadata, shown only when there is no content.
    let output = this.#contentText || this.#rawOutput.trim();
    // omp's execute content starts with an echo of its `$ …` title, even before stdout exists.
    if (record.kind === "execute" && record.title.startsWith("$ ")) {
      if (output === record.title) output = "";
      else if (output.startsWith(`${record.title}\n`)) output = output.slice(record.title.length + 1);
    }
    if (output) record.output = output;
    else delete record.output;
    return record;
  }
}
