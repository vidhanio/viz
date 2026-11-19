/** Plain-text views of transcripts, shared by search, the orchestrator's tools, and the ACP agents' MCP tools. */
import type { Context } from "@earendil-works/chord/context";
import {
  AssistantEntry,
  type ConversationId,
  type Cursor,
  type EntryRecord,
  type Harness,
  type SettledSubmissionRecord,
  ToolResultEntry,
  UserEntry,
} from "@earendil-works/pi-durable";

import { PromptEntry, TextEntry, ToolEntry, TurnEntry } from "./agents/model.ts";

type Line = { role: "User" | "Assistant" | "Tool"; text: string };

/** The visible text of a native or ACP transcript entry, and who it is from. */
export function entryLine(entry: EntryRecord): Line | undefined {
  if (PromptEntry.is(entry)) return { role: "User", text: entry.data.text };
  if (TextEntry.is(entry)) return { role: "Assistant", text: entry.data.text };
  if (ToolEntry.is(entry)) return { role: "Tool", text: entry.data.title };

  const input = entry.kind === "viz.input";
  const data = entry.data;
  // A connector's original text excludes the routing prefix that the model receives.
  if (input && data && typeof data === "object" && !Array.isArray(data) && typeof data.text === "string") {
    return { role: "User", text: data.text };
  }
  const user = input || UserEntry.is(entry);
  const assistant = AssistantEntry.is(entry);
  const tool = ToolResultEntry.is(entry);
  if ((!user && !assistant && !tool) || !entry.model) return;

  const parts: string[] = [];
  for (const message of entry.model) {
    if (user && message.role === "user") {
      if (typeof message.content === "string") parts.push(message.content);
      else for (const block of message.content) if (block.type === "text") parts.push(block.text);
    } else if (assistant && message.role === "assistant") {
      for (const block of message.content) {
        if (block.type === "text") parts.push(block.text);
        else if (block.type === "toolCall") parts.push(block.name);
      }
    } else if (tool && message.role === "toolResult") {
      parts.push(message.toolName);
    }
  }
  return { role: user ? "User" : assistant ? "Assistant" : "Tool", text: parts.join("\n") };
}

/** The last `limit` lines of a conversation's transcript, oldest first, each cut to `width` characters. */
export async function transcript(
  harness: Harness,
  id: ConversationId,
  limit: number,
  context: Context,
  width = 4000,
): Promise<string> {
  const target = await harness.conversation(id, context);
  if (!target) throw new Error("Conversation not found");
  const lines: string[] = [];
  let cursor: Cursor | undefined;
  do {
    const page = await target.entries({}, 32, cursor, context);
    for (const entry of page.items) {
      const line = entryLine(entry);
      if (line?.text) lines.push(`${line.role}: ${line.text.slice(0, width)}`);
      else if (TurnEntry.is(entry) && entry.data.error) lines.push(`Error: ${entry.data.error}`);
      if (lines.length >= limit) break;
    }
    cursor = page.next;
  } while (cursor !== undefined && lines.length < limit);
  return lines.reverse().join("\n");
}

/** What a settled ACP chat input came to: its answer, or why it has none. */
export async function settledAnswer(
  harness: Harness,
  result: SettledSubmissionRecord,
  context: Context,
): Promise<{ answer: string } | { unanswered: string; error?: string }> {
  if (result.status === "done" && result.type === "input") {
    const turn = await harness.commit((tx) => tx.entry(TurnEntry, result.answer), context);
    return { answer: turn?.data.answer ?? "" };
  }
  const reason = result.status === "unanswered" ? result.reason : result.status;
  if (reason !== "acp_error") return { unanswered: reason };
  // The worker records the failure as the conversation's latest turn.
  let cursor: Cursor | undefined;
  do {
    const page = await harness.commit(
      (tx) => tx.scanEntries({ conversationId: result.conversationId }, 32, cursor),
      context,
    );
    const turn = page.items.find(TurnEntry.is);
    if (turn) return { unanswered: reason, ...(turn.data.error && { error: turn.data.error }) };
    cursor = page.next;
  } while (cursor !== undefined);
  return { unanswered: reason };
}
