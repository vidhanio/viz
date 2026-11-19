/**
 * `viz-backend mcp-bridge`: a stdio MCP server that relays to viz's HTTP MCP endpoint, for ACP agents that only take
 * `command`-style MCP servers (every ACP agent must support those). Like t3code's `acp-mcp-bridge`: the SDK's stdio
 * transport reads JSON-RPC from stdin, and its Streamable HTTP client transport POSTs each message with the agent's
 * token and the protocol headers the message needs. stdout carries nothing else.
 */
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/client";
import {
  isInitializeRequest,
  isJSONRPCRequest,
  isJSONRPCResultResponse,
  type JSONRPCMessage,
} from "@modelcontextprotocol/server";
import { StdioServerTransport } from "@modelcontextprotocol/server/stdio";

const URL_ENV = "VIZ_MCP_URL";
const TOKEN_ENV = "VIZ_MCP_TOKEN";

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
  const local = new StdioServerTransport();
  const upstream = new StreamableHTTPClientTransport(new URL(url), {
    authProvider: { token: async () => token },
    redirectPolicy: "same-origin",
  });
  // A 2025-era client negotiates its version once; later requests must carry it as a header.
  const initializing = new Set<string | number>();
  upstream.onmessage = (message) => {
    if (isJSONRPCResultResponse(message) && initializing.delete(message.id)) {
      const version = message.result.protocolVersion;
      if (typeof version === "string") upstream.setProtocolVersion(version);
    }
    void local.send(message).catch((error: unknown) => fatal(error));
  };
  upstream.onerror = (error) => console.error(`mcp-bridge: ${error.message}`);
  local.onerror = (error) => console.error(`mcp-bridge: ${error.message}`);
  const { promise: closed, resolve: close } = Promise.withResolvers<void>();
  local.onclose = close;

  // One message at a time, in order: an `initialize` must finish before the calls that follow it.
  let queue = Promise.resolve();
  local.onmessage = (message) => {
    queue = queue.then(() => relay(message));
  };
  const relay = async (message: JSONRPCMessage) => {
    if (isJSONRPCRequest(message) && isInitializeRequest(message)) initializing.add(message.id);
    try {
      await upstream.send(message);
    } catch (error) {
      // Answer a request the backend didn't, so the agent isn't left waiting; notifications need no answer.
      if (isJSONRPCRequest(message)) {
        initializing.delete(message.id);
        const text = error instanceof Error ? error.message : String(error);
        await local.send({ jsonrpc: "2.0", id: message.id, error: { code: -32603, message: text } });
      }
    }
  };

  await upstream.start();
  await local.start();
  await closed;
  await queue;
  await upstream.close();
  process.exit(0);
}

function fatal(error: unknown): never {
  console.error(`mcp-bridge: ${error instanceof Error ? error.message : String(error)}`);
  process.exit(1);
}
