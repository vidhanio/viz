/**
 * `viz-backend mcp-bridge`: a stdio MCP server that relays to viz's HTTP MCP endpoint, for ACP agents that only take
 * `command`-style MCP servers (every ACP agent must support those). Like t3code's `acp-mcp-bridge`: newline-delimited
 * JSON-RPC from stdin is POSTed with the agent's token; JSON answers go to stdout. stdout carries nothing else.
 */
import { compile, lit, nul, num, obj, str, union } from "@earendil-works/pi-schema";

import { mcpRequestHeaders } from "./server.ts";

const URL_ENV = "VIZ_MCP_URL";
const TOKEN_ENV = "VIZ_MCP_TOKEN";

const Reply = compile(obj({ jsonrpc: lit("2.0"), id: union([str(), num(), nul()]) }, { open: true }));
const InitializeReply = compile(obj({ result: obj({ protocolVersion: str() }, { open: true }) }, { open: true }));
const Initialize = compile(obj({ method: lit("initialize") }, { open: true }));

/** The environment the bridge reads, for the process that launches it. */
export function bridgeEnv(url: string, token: string): Record<string, string> {
  return { [URL_ENV]: url, [TOKEN_ENV]: token };
}

export async function runBridge(): Promise<never> {
  const url = process.env[URL_ENV];
  const token = process.env[TOKEN_ENV];
  if (!url || !token) {
    console.error(`mcp-bridge: ${URL_ENV} and ${TOKEN_ENV} must be set`);
    process.exit(2);
  }
  const headers = {
    authorization: `Bearer ${token}`,
    "content-type": "application/json",
    accept: "application/json, text/event-stream",
  };
  let legacyVersion: string | undefined;
  // One message at a time, in order: an `initialize` must finish before the calls that follow it.
  for await (const line of console) {
    if (line.trim() === "") continue;
    let message: unknown;
    try {
      message = JSON.parse(line);
    } catch {
      await write({ jsonrpc: "2.0", id: null, error: { code: -32700, message: "invalid JSON" } });
      continue;
    }
    const mirrors = mcpRequestHeaders(message);
    const version =
      mirrors === undefined && legacyVersion !== undefined && !Initialize.check(message)
        ? { "mcp-protocol-version": legacyVersion }
        : undefined;
    let response: Response;
    try {
      response = await fetch(url, {
        method: "POST",
        headers: { ...headers, ...version, ...mirrors },
        body: line,
        redirect: "error",
      });
    } catch {
      await fail(message, "cannot reach viz backend");
      continue;
    }
    if (response.status === 202) continue;
    // The MCP endpoint also returns JSON-RPC errors with non-2xx HTTP status codes.
    let reply: unknown;
    try {
      reply = await response.json();
    } catch {
      await fail(message, `invalid viz backend response (HTTP ${response.status})`);
      continue;
    }
    if (!Reply.check(reply) && !(Array.isArray(reply) && reply.length > 0 && reply.every(Reply.check))) {
      await fail(message, `invalid viz backend response (HTTP ${response.status})`);
      continue;
    }
    if (Initialize.check(message) && InitializeReply.check(reply)) legacyVersion = reply.result.protocolVersion;
    await write(reply);
  }
  process.exit(0);
}

function write(message: unknown): Promise<void> {
  const { promise, resolve, reject } = Promise.withResolvers<void>();
  process.stdout.write(`${JSON.stringify(message)}\n`, (error) => (error ? reject(error) : resolve()));
  return promise;
}

/** Answers a request the backend didn't, so the agent isn't left waiting; notifications need no answer. */
async function fail(request: unknown, message: string): Promise<void> {
  console.error(`mcp-bridge: ${message}`);
  if (Array.isArray(request)) {
    const responses = request.flatMap((item) => errorReply(item, message) ?? []);
    if (responses.length > 0) await write(responses);
    return;
  }
  const response = errorReply(request, message);
  if (response !== undefined) await write(response);
}

function errorReply(request: unknown, message: string) {
  if (request === null || typeof request !== "object" || !("id" in request)) return undefined;
  const { id } = request;
  if (id !== null && typeof id !== "string" && typeof id !== "number") return undefined;
  return { jsonrpc: "2.0", id, error: { code: -32603, message } };
}
