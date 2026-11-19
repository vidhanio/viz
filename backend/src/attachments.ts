import { createHash, randomUUID } from "node:crypto";
import { mkdir, rename, rm, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";

import type { Attachment } from "./protocol.ts";

const CAP = 20 * 1024 * 1024;
export type StoredAttachment = { name: string; mime: string; path: string };

export async function saveAttachments(dir: string, attachments: Attachment[]): Promise<StoredAttachment[]> {
  const saved: StoredAttachment[] = [];
  for (const { name, mime, data } of attachments) {
    if (name === "." || name === ".." || /[\\/\x00-\x1f\x7f]/.test(name) || Buffer.byteLength(name) > 255) {
      throw new Error("Invalid attachment name");
    }
    if (data.length > Math.ceil(CAP / 3) * 4) throw new Error("Attachment exceeds 20 MiB");
    const padding = data.endsWith("==") ? 2 : data.endsWith("=") ? 1 : 0;
    const firstPadding = data.indexOf("=");
    if (
      data.length % 4 !== 0 ||
      /[^a-zA-Z\d+/=]/.test(data) ||
      (firstPadding !== -1 && firstPadding !== data.length - padding)
    )
      throw new Error("Invalid attachment base64");
    const bytes = Buffer.from(data, "base64");
    if (bytes.length !== (data.length / 4) * 3 - padding) throw new Error("Invalid attachment base64");
    if (bytes.length > CAP) throw new Error("Attachment exceeds 20 MiB");
    const hash = createHash("sha256").update(bytes).digest("hex");
    const directory = resolve(dir, "attachments", hash);
    const path = join(directory, name);
    await mkdir(directory, { mode: 0o700, recursive: true });
    if (!(await Bun.file(path).exists())) {
      const temporary = join(directory, `.upload-${randomUUID()}`);
      try {
        await writeFile(temporary, bytes, { mode: 0o600, flag: "wx" });
        await rename(temporary, path);
      } finally {
        await rm(temporary, { force: true });
      }
    }
    saved.push({ name, mime, path });
  }
  return saved;
}

export function withAttachmentPaths(text: string, attachments: StoredAttachment[]): string {
  if (attachments.length === 0) return text;
  return `${text}\n\nAttachments (local files):\n${attachments.map(({ name, path }) => `- ${name}: ${path}`).join("\n")}`;
}
