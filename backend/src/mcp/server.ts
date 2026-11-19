/**
 * The MCP server that gives ACP agents viz's tools (`agents/tools.ts`), as t3code gives its provider sessions its
 * orchestrator toolkit (`docs/findings/t3code-subagents.md`).
 *
 * The official SDK (`@modelcontextprotocol/server`) serves Streamable HTTP statelessly, both the 2026-07-28 protocol
 * and 2025-era clients, on a loopback port of its own. Each agent session gets its own bearer
 * token, which is its identity: a tool call acts as the agent that holds the token, never as an id the caller claims.
 * The token is checked here, before the SDK sees the request. Agents without HTTP MCP reach this endpoint through the
 * stdio bridge (`bridge.ts`).
 */
import { randomBytes } from "node:crypto";

import {
  type AuthInfo,
  createMcpHandler,
  fromJsonSchema,
  type JsonSchemaType,
  McpServer as SdkServer,
  type McpHttpHandler,
} from "@modelcontextprotocol/server";

/** A tool, its JSON Schema input (pi-schema), and what it does for the agent `scope`. */
export type McpTool<Scope, Args = unknown> = {
  readonly name: string;
  readonly description: string;
  readonly input: object;
  readonly call: (args: Args, scope: Scope) => Promise<{ text: string; child?: number }>;
};

const PATH = "/mcp";

export type McpServerOptions = { readonly name: string; readonly version: string; readonly instructions: string };

export class McpServer<Scope> {
  readonly #scopes = new Map<string, Scope>();
  /** The scope each verified request was admitted with; the SDK passes `authInfo` through unchanged. */
  readonly #admitted = new WeakMap<AuthInfo, Scope>();
  readonly #handler: McpHttpHandler;
  #server: Bun.Server<undefined> | undefined;

  constructor(
    options: McpServerOptions,
    tools: readonly McpTool<Scope>[],
    receiptFor: (scope: Scope) => ((name: string, child?: number) => string) | undefined,
  ) {
    const { name, version, instructions } = options;
    const schemas = tools.map((tool) => ({ tool, input: fromJsonSchema(tool.input as JsonSchemaType) }));
    // The SDK serves each request with a fresh server instance, bound here to the scope its token was admitted with.
    this.#handler = createMcpHandler(
      ({ authInfo }) => {
        const scope = authInfo && this.#admitted.get(authInfo);
        if (scope === undefined) throw new Error("Unauthenticated MCP request");
        const server = new SdkServer({ name, version }, { instructions, capabilities: { tools: {} } });
        for (const { tool, input } of schemas) {
          server.registerTool(tool.name, { description: tool.description, inputSchema: input }, async (args) => {
            const receipt = receiptFor(scope);
            const result = await tool.call(args, scope).then(
              ({ text, child }) => ({ text, child, isError: false }),
              (error: unknown) => ({
                text: error instanceof Error ? error.message : String(error),
                child: undefined,
                isError: true,
              }),
            );
            return {
              content: [
                { type: "text", text: result.text },
                ...(receipt ? [{ type: "text" as const, text: receipt(tool.name, result.child) }] : []),
              ],
              ...(result.isError && { isError: true }),
            };
          });
        }
        return server;
      },
      { onerror: (error) => console.error("MCP:", error.message) },
    );
  }

  /** Listens on a free loopback port. */
  start(): void {
    if (this.#server !== undefined) return;
    // A waiting subagent call can outlast Bun's request timeout; the enclosing ACP turn owns cancellation.
    this.#server = Bun.serve({
      hostname: "127.0.0.1",
      port: 0,
      idleTimeout: 0,
      fetch: (request) => this.#fetch(request),
    });
  }

  stop(): void {
    void this.#server?.stop(true);
    void this.#handler.close();
    this.#server = undefined;
    this.#scopes.clear();
  }

  get url(): string {
    if (this.#server === undefined) throw new Error("MCP server not started");
    return new URL(PATH, this.#server.url).href;
  }

  /** A new token acting as `scope`. */
  grant(scope: Scope): string {
    const token = randomBytes(24).toString("base64url");
    this.#scopes.set(token, scope);
    return token;
  }

  revoke(token: string): void {
    this.#scopes.delete(token);
  }

  #fetch(request: Request): Promise<Response> | Response {
    if (new URL(request.url).pathname !== PATH) return new Response(null, { status: 404 });
    // MCP requires refusing browser origins before credentials.
    if (request.headers.has("origin")) return new Response(null, { status: 403 });
    const header = request.headers.get("authorization");
    const token = header?.slice(0, 7).toLowerCase() === "bearer " ? header.slice(7) : undefined;
    const scope = token === undefined ? undefined : this.#scopes.get(token);
    if (token === undefined || scope === undefined)
      return new Response(null, { status: 401, headers: { "www-authenticate": "Bearer" } });
    const authInfo: AuthInfo = { token, clientId: "viz", scopes: [] };
    this.#admitted.set(authInfo, scope);
    return this.#handler.fetch(request, { authInfo });
  }
}

/** Erases a tool's argument type for the registry; its schema validates arguments before `call`. */
export function tool<Scope, Args>(definition: McpTool<Scope, Args>): McpTool<Scope> {
  return definition as McpTool<Scope>;
}
