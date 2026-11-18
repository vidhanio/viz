/**
 * The MCP server that gives ACP agents viz's tools (`agents/tools.ts`), as t3code gives its provider sessions its
 * orchestrator toolkit (`docs/findings/t3code-subagents.md`).
 *
 * Streamable HTTP, tools only, JSON responses (no SSE stream), on a loopback port of its own. Each agent session gets
 * its own bearer token, which is its identity: a tool call acts as the agent that holds the token, never as an id the
 * caller claims. Agents without HTTP MCP reach this endpoint through the stdio bridge (`bridge.ts`).
 */
import { randomBytes } from "node:crypto";

import { compile, lit, nul, num, obj, opt, str, union, unknown, type Validator } from "@earendil-works/pi-schema";

/** A tool, its JSON Schema input, and what it does for the agent `scope`. */
export type McpTool<Scope, Args = unknown> = {
  readonly name: string;
  readonly description: string;
  readonly input: object;
  readonly call: (args: Args, scope: Scope) => Promise<string>;
};

type JsonRpcId = string | number | null;
type JsonRpcRequest = { jsonrpc: "2.0"; id?: JsonRpcId; method: string; params?: unknown };
type JsonRpcResponse =
  | { jsonrpc: "2.0"; id: JsonRpcId; result: object }
  | { jsonrpc: "2.0"; id: JsonRpcId; error: { code: number; message: string; data?: unknown } };

const CURRENT_VERSION = "2026-07-28";
const LEGACY_VERSIONS = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"] as const;
const PROTOCOL_VERSIONS = [CURRENT_VERSION, ...LEGACY_VERSIONS];
const VERSION_META = "io.modelcontextprotocol/protocolVersion";
const CAPABILITIES_META = "io.modelcontextprotocol/clientCapabilities";
const SERVER_INFO_META = "io.modelcontextprotocol/serverInfo";
const PATH = "/mcp";

const METHOD_NOT_FOUND = -32601;
const INVALID_PARAMS = -32602;
const INVALID_REQUEST = -32600;
const HEADER_MISMATCH = -32020;
const UNSUPPORTED_VERSION = -32022;

const Message = compile(
  obj(
    { jsonrpc: lit("2.0"), id: opt(union([str(), num(), nul()])), method: str(), params: opt(unknown()) },
    { open: true },
  ),
);
const CallParams = compile(obj({ name: str(), arguments: opt(unknown()) }, { open: true }));
const InitializeParams = compile(obj({ protocolVersion: str() }, { open: true }));
const RequestMetadata = compile(
  obj({ _meta: obj({ [VERSION_META]: str(), [CAPABILITIES_META]: opt(unknown()) }, { open: true }) }, { open: true }),
);
const ClientCapabilities = compile(obj({}, { open: true }));
const ResourceParams = compile(obj({ uri: str() }, { open: true }));

export type McpServerOptions = { readonly name: string; readonly version: string; readonly instructions: string };

export class McpServer<Scope> {
  readonly #options: McpServerOptions;
  readonly #tools: Map<string, { tool: McpTool<Scope>; check: Validator<unknown> }>;
  readonly #scopes = new Map<string, Scope>();
  #server: Bun.Server<undefined> | undefined;

  constructor(options: McpServerOptions, tools: readonly McpTool<Scope>[]) {
    this.#options = options;
    this.#tools = new Map(tools.map((tool) => [tool.name, { tool, check: compile(tool.input) }]));
  }

  /** Listens on a free loopback port. */
  start(): void {
    if (this.#server !== undefined) return;
    this.#server = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: (request) => this.#fetch(request) });
  }

  stop(): void {
    void this.#server?.stop(true);
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

  async #fetch(request: Request): Promise<Response> {
    if (new URL(request.url).pathname !== PATH) return new Response(null, { status: 404 });
    // MCP requires refusing browser origins before credentials.
    if (request.headers.has("origin")) return new Response(null, { status: 403 });
    const header = request.headers.get("authorization");
    const scope = header?.slice(0, 7).toLowerCase() === "bearer " ? this.#scopes.get(header.slice(7)) : undefined;
    if (scope === undefined) return new Response(null, { status: 401, headers: { "www-authenticate": "Bearer" } });
    if (request.method !== "POST") return new Response(null, { status: 405, headers: { allow: "POST" } });

    let body: unknown;
    try {
      body = await request.json();
    } catch {
      return Response.json(failure(null, -32700, "invalid JSON"), { status: 400 });
    }

    // The modern protocol carries its version on every request, whereas legacy clients negotiate via initialize.
    const version = request.headers.get("mcp-protocol-version");
    if (
      request.headers.has("mcp-method") ||
      (version !== null && !LEGACY_VERSIONS.some((legacy) => legacy === version)) ||
      (isRequest(body) && requestMetadata(body.params)?.[VERSION_META] !== undefined)
    ) {
      return this.#modernRequest(request.headers, body, scope);
    }
    if (!Array.isArray(body)) {
      const response = await this.#handle(body, scope);
      return response === null ? new Response(null, { status: 202 }) : Response.json(response);
    }
    if (body.length === 0) return Response.json(failure(null, INVALID_REQUEST, "empty JSON-RPC batch"));
    const responses: JsonRpcResponse[] = [];
    for (const message of body) {
      const response = await this.#handle(message, scope);
      if (response !== null) responses.push(response);
    }
    if (responses.length === 0) return new Response(null, { status: 202 });
    return Response.json(responses);
  }

  async #modernRequest(headers: Headers, body: unknown, scope: Scope): Promise<Response> {
    if (!isRequest(body) || body.id == null) {
      return Response.json(failure(null, INVALID_REQUEST, "expected one JSON-RPC request"), { status: 400 });
    }
    const { id, method, params } = body;
    const metadata = requestMetadata(params);
    const version = metadata?.[VERSION_META];
    if (typeof version !== "string" || headers.get("mcp-protocol-version") !== version) {
      return Response.json(failure(id, HEADER_MISMATCH, "MCP-Protocol-Version does not match request metadata"), {
        status: 400,
      });
    }
    if (version !== CURRENT_VERSION) {
      return Response.json(
        failure(id, UNSUPPORTED_VERSION, "unsupported protocol version", {
          supported: PROTOCOL_VERSIONS,
          requested: version,
        }),
        { status: 400 },
      );
    }
    const name = requestName(body);
    if (
      headers.get("mcp-method") !== method ||
      (name !== undefined && !matchesNameHeader(headers.get("mcp-name"), name))
    ) {
      return Response.json(failure(id, HEADER_MISMATCH, "Mcp-Method or Mcp-Name does not match request"), {
        status: 400,
      });
    }
    if (!ClientCapabilities.check(metadata?.[CAPABILITIES_META])) {
      return Response.json(failure(id, INVALID_PARAMS, `missing ${CAPABILITIES_META} metadata`), { status: 400 });
    }
    const response = await this.#handle(body, scope, true);
    if (response === null) return new Response(null, { status: 202 });
    if ("error" in response) {
      return Response.json(response, { status: response.error.code === METHOD_NOT_FOUND ? 404 : 200 });
    }
    return Response.json(
      success(id, {
        ...response.result,
        resultType: "complete",
        _meta: { [SERVER_INFO_META]: { name: this.#options.name, version: this.#options.version } },
      }),
    );
  }

  /** One JSON-RPC message; null for notifications. */
  async #handle(message: unknown, scope: Scope, modern = false): Promise<JsonRpcResponse | null> {
    if (!isRequest(message)) return failure(null, INVALID_REQUEST, "expected a JSON-RPC 2.0 message");
    if (message.id === undefined) return null;
    const id = message.id;
    switch (message.method) {
      case "initialize":
        if (modern) break;
        return success(id, {
          protocolVersion: negotiate(message.params),
          capabilities: { tools: { listChanged: false } },
          serverInfo: { name: this.#options.name, version: this.#options.version },
          instructions: this.#options.instructions,
        });
      case "server/discover":
        if (!modern) break;
        return success(id, {
          supportedVersions: PROTOCOL_VERSIONS,
          capabilities: { tools: {} },
          instructions: this.#options.instructions,
          ttlMs: 60_000,
          cacheScope: "private",
        });
      case "ping":
        if (modern) break;
        return success(id, {});
      case "tools/list":
        return success(id, {
          tools: [...this.#tools.values()].map(({ tool }) => ({
            name: tool.name,
            description: tool.description,
            inputSchema: tool.input,
          })),
          ...(modern ? { ttlMs: 60_000, cacheScope: "private" } : {}),
        });
      case "tools/call":
        return this.#call(id, message.params, scope);
    }
    return failure(id, METHOD_NOT_FOUND, `no method ${message.method}`);
  }

  async #call(id: JsonRpcId, params: unknown, scope: Scope): Promise<JsonRpcResponse> {
    if (!CallParams.check(params)) return failure(id, INVALID_PARAMS, "expected { name, arguments? }");
    const { name, arguments: args = {} } = params;
    const entry = this.#tools.get(name);
    if (entry === undefined) return failure(id, INVALID_PARAMS, `no tool ${name}`);
    if (!entry.check.check(args)) {
      const detail = entry.check
        .errors(args)
        .map((e) => `${e.path || "/"}: ${e.message}`)
        .join("; ");
      return success(id, { content: [{ type: "text", text: `invalid arguments: ${detail}` }], isError: true });
    }
    try {
      const text = await entry.tool.call(args, scope);
      return success(id, { content: [{ type: "text", text }] });
    } catch (error) {
      const text = error instanceof Error ? error.message : String(error);
      return success(id, { content: [{ type: "text", text }], isError: true });
    }
  }
}

/** Erases a tool's argument type for the registry; its schema validates arguments before `call`. */
export function tool<Scope, Args>(definition: McpTool<Scope, Args>): McpTool<Scope> {
  return definition as McpTool<Scope>;
}

function isRequest(message: unknown): message is JsonRpcRequest {
  return Message.check(message) && !("result" in message) && !("error" in message);
}

function requestMetadata(params: unknown) {
  return RequestMetadata.check(params) ? params._meta : undefined;
}

function requestName({ method, params }: JsonRpcRequest): string | undefined {
  if (method === "tools/call" || method === "prompts/get") {
    return CallParams.check(params) ? params.name : undefined;
  }
  if (method === "resources/read") return ResourceParams.check(params) ? params.uri : undefined;
  return undefined;
}

function sentinel(header: string): string | undefined {
  return header.startsWith("=?base64?") && header.endsWith("?=") ? header.slice(9, -2) : undefined;
}

function matchesNameHeader(header: string | null, name: string): boolean {
  if (header === null) return false;
  const encoded = sentinel(header);
  return encoded === undefined ? header === name : encoded === Buffer.from(name, "utf8").toString("base64");
}

/** Adds the required HTTP mirrors when forwarding a modern stdio request. */
export function mcpRequestHeaders(message: unknown): Record<string, string> | undefined {
  if (!isRequest(message)) return undefined;
  const version = requestMetadata(message.params)?.[VERSION_META];
  if (typeof version !== "string") return undefined;
  const headers = { "mcp-protocol-version": version, "mcp-method": message.method };
  const name = requestName(message);
  if (name === undefined) return headers;
  const mirror =
    /^[\t -~]*$/.test(name) && name.trim() === name && sentinel(name) === undefined
      ? name
      : `=?base64?${Buffer.from(name, "utf8").toString("base64")}?=`;
  return { ...headers, "mcp-name": mirror };
}

function negotiate(params: unknown): string {
  const requested = InitializeParams.check(params) ? params.protocolVersion : undefined;
  return LEGACY_VERSIONS.find((version) => version === requested) ?? LEGACY_VERSIONS[0];
}

function success(id: JsonRpcId, result: object): JsonRpcResponse {
  return { jsonrpc: "2.0", id, result };
}

function failure(id: JsonRpcId, code: number, message: string, data?: unknown): JsonRpcResponse {
  return { jsonrpc: "2.0", id, error: { code, message, ...(data === undefined ? {} : { data }) } };
}
