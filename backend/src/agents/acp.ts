/**
 * One ACP agent process and its client connection (`docs/findings/acp.md`).
 *
 * viz is the ACP client: it spawns the harness, speaks protocol version 1 over stdio, and advertises no `fs`/`terminal`
 * capabilities, so agents use their own tools on the local machine. Every permission request is allowed.
 */
import * as acp from "@agentclientprotocol/sdk";

export type AcpEvents = {
  /** A `session/update` for a session of this process, in arrival order. */
  readonly onUpdate: (sessionId: string, update: acp.SessionUpdate) => void;
  /** The connection closed; `error` unless viz closed it. */
  readonly onClose: (error: Error | null) => void;
  readonly log: (line: string) => void;
};

export type Launch = {
  readonly command: readonly string[];
  readonly env: Readonly<Record<string, string>>;
  readonly cwd: string;
  /** Cancels startup only; after initialization the caller owns the process. */
  readonly signal?: AbortSignal;
};

const CLIENT_INFO = { name: "viz", title: "viz", version: "0.1.0" } as const;

// Keep the harness usable without inheriting provider tokens, viz config overrides, or other backend secrets.
const RUNTIME_ENV = [
  "PATH",
  "HOME",
  "USER",
  "LOGNAME",
  "SHELL",
  "TMPDIR",
  "TMP",
  "TEMP",
  "LANG",
  "LANGUAGE",
  "LC_ALL",
  "LC_CTYPE",
  "LC_MESSAGES",
  "TZ",
  "TERM",
  "COLORTERM",
  "XDG_CONFIG_HOME",
  "XDG_DATA_HOME",
  "XDG_STATE_HOME",
  "XDG_CACHE_HOME",
  "XDG_RUNTIME_DIR",
  "XDG_CONFIG_DIRS",
  "XDG_DATA_DIRS",
  "SSL_CERT_FILE",
  "SSL_CERT_DIR",
  "NODE_EXTRA_CA_CERTS",
] as const;

function launchEnv(overrides: Launch["env"]): Record<string, string> {
  const env = { ...overrides };
  for (const name of RUNTIME_ENV) {
    const value = process.env[name];
    if (value !== undefined && !Object.hasOwn(env, name)) env[name] = value;
  }
  return env;
}

export class AcpProcess {
  readonly #capabilities: acp.AgentCapabilities;
  readonly #connection: acp.ClientConnection;
  #closing = false;
  readonly agentName: string | undefined;

  private constructor(connection: acp.ClientConnection, init: acp.InitializeResponse) {
    this.#connection = connection;
    this.#capabilities = init.agentCapabilities ?? {};
    this.agentName = init.agentInfo?.name;
  }

  /** Spawns the harness and initializes the connection. */
  static async start(launch: Launch, events: AcpEvents): Promise<AcpProcess> {
    const { signal } = launch;
    signal?.throwIfAborted();
    const [command, ...args] = launch.command;
    if (command === undefined) throw new Error("harness command is empty");
    const child = Bun.spawn([command, ...args], {
      stdin: "pipe",
      stdout: "pipe",
      stderr: "pipe",
      env: launchEnv(launch.env),
      cwd: launch.cwd,
    });
    void pipeLines(child.stderr, events.log).catch((error: unknown) => events.log(`harness stderr: ${String(error)}`));

    let owner: AcpProcess | undefined;
    const stdin = new WritableStream<Uint8Array>({
      async write(chunk) {
        const written = child.stdin.write(chunk);
        if (typeof written !== "number") await written;
        const flushed = child.stdin.flush();
        if (typeof flushed !== "number") await flushed;
      },
    });
    const connection = acp
      .client({ name: CLIENT_INFO.name })
      .onRequest(acp.methods.client.session.requestPermission, ({ params }) => allow(params))
      .onNotification(acp.methods.client.session.update, ({ params }) =>
        events.onUpdate(params.sessionId, params.update),
      )
      .connect(acp.ndJsonStream(stdin, child.stdout));

    // The SDK rejects pending requests and cancels stdout on close, but does not close the subprocess stdin.
    let initializationAborted = false;
    void connection.closed.then(() => {
      stopProcess(child);
      const reason: unknown = connection.signal.reason;
      const intentional = initializationAborted || (owner !== undefined && owner.#closing);
      events.onClose(intentional ? null : reason instanceof Error ? reason : new Error(String(reason)));
    });
    void child.exited.then(
      (code) =>
        connection.close(
          new Error(`${command} exited ${child.signalCode ? `on ${child.signalCode}` : `with code ${code}`}`),
        ),
      (cause) => connection.close(new Error(`${command} failed`, { cause })),
    );

    const abort = () => {
      initializationAborted = true;
      connection.close(signal?.reason);
      child.kill("SIGTERM");
    };
    signal?.addEventListener("abort", abort, { once: true });
    try {
      const init = await connection.agent.request<acp.InitializeResponse, acp.InitializeRequest>(
        acp.methods.agent.initialize,
        { protocolVersion: acp.PROTOCOL_VERSION, clientCapabilities: {}, clientInfo: CLIENT_INFO },
      );
      signal?.throwIfAborted();
      if (init.protocolVersion !== acp.PROTOCOL_VERSION) {
        throw new Error(`${command} uses unsupported ACP protocol version ${init.protocolVersion}`);
      }
      owner = new AcpProcess(connection, init);
      return owner;
    } catch (error) {
      connection.close(error);
      throw error;
    } finally {
      signal?.removeEventListener("abort", abort);
    }
  }

  get closed(): boolean {
    return this.#connection.signal.aborted;
  }

  /** Whether the agent can take viz's tools over HTTP MCP; otherwise they go through the stdio bridge. */
  get supportsHttpMcp(): boolean {
    return this.#capabilities.mcpCapabilities?.http === true;
  }

  get supportsImages(): boolean {
    return this.#capabilities.promptCapabilities?.image === true;
  }

  newSession(cwd: string, mcpServers: acp.McpServer[]): Promise<acp.NewSessionResponse> {
    return this.#connection.agent.request(acp.methods.agent.session.new, { cwd, mcpServers });
  }

  async discardSession(sessionId: string): Promise<void> {
    const capabilities = this.#capabilities.sessionCapabilities;
    const methods = acp.methods.agent.session;
    if (capabilities?.delete) await this.#connection.agent.request(methods.delete, { sessionId });
    else if (capabilities?.close) await this.#connection.agent.request(methods.close, { sessionId });
  }

  /** Restores the same session; a successful load has finished sending its history before replying. */
  restoreSession(sessionId: string, cwd: string, mcpServers: acp.McpServer[]): Promise<acp.LoadSessionResponse> {
    const params = { sessionId, cwd, mcpServers };
    const methods = acp.methods.agent.session;
    if (this.#capabilities.sessionCapabilities?.resume) return this.#connection.agent.request(methods.resume, params);
    if (this.#capabilities.loadSession === true) return this.#connection.agent.request(methods.load, params);
    throw new Error("ACP harness cannot restore sessions");
  }

  setOption(sessionId: string, configId: string, value: string | boolean): Promise<acp.SetSessionConfigOptionResponse> {
    return this.#connection.agent.request(acp.methods.agent.session.setConfigOption, {
      sessionId,
      configId,
      ...(typeof value === "boolean" ? { type: "boolean", value } : { value }),
    });
  }

  /** Runs one prompt turn; resolves with its stop reason when the agent ends it. */
  async prompt(sessionId: string, prompt: acp.ContentBlock[]): Promise<acp.StopReason> {
    const response = await this.#connection.agent.request(acp.methods.agent.session.prompt, {
      sessionId,
      prompt,
    });
    return response.stopReason;
  }

  /** Asks the agent to end its current turn; the prompt then stops with `cancelled`. */
  async cancel(sessionId: string): Promise<void> {
    await this.#connection.agent.notify(acp.methods.agent.session.cancel, { sessionId });
  }

  close(): void {
    this.#closing = true;
    this.#connection.close();
  }
}

function stopProcess(child: Bun.PipedSubprocess): void {
  if (child.exitCode !== null || child.signalCode !== null) return;
  // Let the harness clean up its own sessions and tools on EOF, with bounded time for stuck children.
  const terminate = setTimeout(() => child.kill("SIGTERM"), 1_000);
  const force = setTimeout(() => child.kill("SIGKILL"), 2_000);
  const clear = () => {
    clearTimeout(terminate);
    clearTimeout(force);
  };
  void child.exited.then(clear, clear);
  try {
    const end = child.stdin.end();
    if (typeof end !== "number") void end.catch(() => {});
  } catch {
    // The harness may already have closed its stdin; the exit timers still apply.
  }
}

async function pipeLines(stream: ReadableStream<Uint8Array>, log: (line: string) => void): Promise<void> {
  const decoder = new TextDecoder();
  let rest = "";
  for await (const chunk of stream) {
    const lines = (rest + decoder.decode(chunk, { stream: true })).split("\n");
    rest = lines.pop() ?? "";
    for (const line of lines) if (line.trim() !== "") log(line);
  }
  rest += decoder.decode();
  if (rest.trim() !== "") log(rest);
}

/** Allows once if offered, else always; a request without an allow option is cancelled. */
function allow({ options }: acp.RequestPermissionRequest): acp.RequestPermissionResponse {
  const option =
    options.find(({ kind }) => kind === "allow_once") ?? options.find(({ kind }) => kind === "allow_always");
  return { outcome: option ? { outcome: "selected", optionId: option.optionId } : { outcome: "cancelled" } };
}
