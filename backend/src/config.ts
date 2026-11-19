/**
 * viz configuration: XDG locations, layered TOML loading, and the backend's sections.
 *
 * Layers (later wins): `$VIZ_CONFIG_DIR/config.toml` (default `$XDG_CONFIG_HOME/viz/config.toml`), then
 * `config.d/*.toml` in lexical order. Tables merge recursively; other values replace. `$VIZ_CONFIG` names one file
 * that replaces all layers (e.g. the repo's gitignored `viz.local.toml` for development). Mirrors `crates/viz-config`;
 * keep the two in sync.
 */
import { readdir } from "node:fs/promises";
import { join } from "node:path";

import { compile, map, obj, oneOf, opt, type Static, str, union, arr } from "@earendil-works/pi-schema";

export type Env = Readonly<Record<string, string | undefined>>;

export type Paths = {
  /** `$VIZ_CONFIG`: the one config file to read instead of the layers. */
  readonly configFile: string | undefined;
  /** `$VIZ_CONFIG_DIR`, default `$XDG_CONFIG_HOME/viz`. */
  readonly configDir: string;
  /** `$STATE_DIRECTORY` (systemd `StateDirectory=`), default `$XDG_STATE_HOME/viz`; `[backend].state_dir` wins. */
  readonly stateDir: string;
  /** `$XDG_RUNTIME_DIR/viz`, falling back to the state dir. */
  readonly runtimeDir: string;
};

export function resolvePaths(env: Env = process.env): Paths {
  const get = (key: string) => (env[key] ? env[key] : undefined);
  const xdg = (key: string, fallback: string) => {
    const value = get(key) ?? (get("HOME") && join(get("HOME")!, fallback));
    if (!value) throw new Error(`cannot locate ${key}: set $HOME or $${key}`);
    return value;
  };
  const configDir = get("VIZ_CONFIG_DIR") ?? join(xdg("XDG_CONFIG_HOME", ".config"), "viz");
  const stateDir = get("STATE_DIRECTORY") ?? join(xdg("XDG_STATE_HOME", ".local/state"), "viz");
  const runtime = get("XDG_RUNTIME_DIR");
  return { configFile: get("VIZ_CONFIG"), configDir, stateDir, runtimeDir: runtime ? join(runtime, "viz") : stateDir };
}

/** A secret inline, from a file (sops-nix, agenix, systemd credentials), or from an environment variable. */
export const Secret = union([str(), obj({ file: str() }), obj({ env: str() })]);
export type Secret = Static<typeof Secret>;

export async function resolveSecret(secret: Secret, env: Env = process.env): Promise<string> {
  let value: string | undefined;
  if (typeof secret === "string") value = secret;
  else if ("file" in secret) value = (await Bun.file(secret.file).text()).replace(/\r?\n$/, "");
  else value = env[secret.env];
  if (!value) throw new Error(`secret ${typeof secret === "string" ? "<inline>" : JSON.stringify(secret)} is empty`);
  return value;
}

const ModelRef = obj({
  provider: str({ description: "pi-ai provider id, e.g. anthropic, openai, openrouter" }),
  model: str({ description: "Model id within the provider" }),
});

const Thinking = oneOf(["off", "minimal", "low", "medium", "high", "xhigh"]);

/** An ACP agent program (`docs/findings/acp.md`), e.g. `claude-agent-acp`, `codex-acp`, `opencode acp`. */
const Harness = obj({
  command: str({ description: "Executable, looked up on PATH when not absolute" }),
  args: opt(arr(str())),
  env: opt(map(Secret, { description: "Extra environment; values are secrets, e.g. API keys" })),
  permissions: opt(
    oneOf(["allow", "ask", "reject"], {
      description: "How viz answers the agent's permission requests; default reject",
    }),
  ),
  model: opt(str({ description: "Default ACP model option value" })),
  effort: opt(str({ description: "Default ACP thinking option value" })),
  mode: opt(str({ description: "Default ACP mode option value" })),
});
export type Harness = Static<typeof Harness>;

/** Sections the backend owns. Other sections (`[client]`, `[connectors.*]`) belong to the Rust components. */
export const BackendConfig = obj(
  {
    backend: opt(
      obj({
        socket: opt(str({ description: "Unix socket path; default $XDG_RUNTIME_DIR/viz/backend.sock" })),
        listen: opt(str({ description: "TCP host:port for remote clients (tailnet/LAN); requires token" })),
        token: opt(Secret),
        state_dir: opt(str({ description: "Where the databases live; default $XDG_STATE_HOME/viz" })),
        default_harness: opt(
          str({ description: "Default ACP runner for chats and delegations; implicit if only one is configured" }),
        ),
      }),
    ),
    orchestrator: obj({
      model: ModelRef,
      thinking: opt(Thinking),
      instructions: opt(str({ description: "Extra instructions appended to the orchestrator's system prompt" })),
      cwd: opt(str({ description: "Working directory for the orchestrator's tools; default $HOME" })),
    }),
    memory: obj({
      compactor: ModelRef,
      thinking: opt(Thinking),
    }),
    providers: opt(
      map(obj({ api_key: Secret }), {
        description: "Credentials by pi-ai provider id, e.g. [providers.anthropic]; else the provider's env var",
      }),
    ),
    harnesses: opt(
      map(Harness, {
        description: "Named ACP programs for chats and subagents",
      }),
    ),
  },
  { open: true },
);
export type BackendConfig = Static<typeof BackendConfig>;

const checkBackendConfig = compile(BackendConfig);

export type LoadedConfig = {
  readonly paths: Paths;
  readonly layers: readonly string[];
  readonly config: BackendConfig;
};

export async function loadConfig(env: Env = process.env): Promise<LoadedConfig> {
  const paths = resolvePaths(env);
  const layers = paths.configFile === undefined ? await configLayers(paths.configDir) : [paths.configFile];
  if (paths.configFile !== undefined && !(await Bun.file(paths.configFile).exists())) {
    throw new Error(`$VIZ_CONFIG: no file ${paths.configFile}`);
  }

  const doc: Record<string, unknown> = {};
  const loaded: string[] = [];
  for (const layer of layers) {
    const file = Bun.file(layer);
    if (!(await file.exists())) continue;
    let parsed: unknown;
    try {
      parsed = Bun.TOML.parse(await file.text());
    } catch (error) {
      throw new Error(`parsing ${layer}: ${(error as Error).message}`);
    }
    merge(doc, parsed as Record<string, unknown>);
    loaded.push(layer);
  }

  const errors = checkBackendConfig.errors(doc);
  if (errors.length > 0) {
    const lines = errors.map((e) => `  ${e.path || "/"}: ${e.message}`).join("\n");
    throw new Error(`invalid config (${loaded.join(", ") || "no files"}):\n${lines}`);
  }
  return { paths, layers: loaded, config: doc as BackendConfig };
}

async function configLayers(dir: string): Promise<string[]> {
  const layers = [join(dir, "config.toml")];
  const dropins = join(dir, "config.d");
  try {
    const names = (await readdir(dropins)).filter((name) => name.endsWith(".toml")).sort();
    layers.push(...names.map((name) => join(dropins, name)));
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
  }
  return layers;
}

export function stateDir({ paths, config }: LoadedConfig): string {
  return config.backend?.state_dir ?? paths.stateDir;
}

function isTable(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value) && !(value instanceof Date);
}

/** Merges `top` into `base`: tables recursively, everything else replaced. */
export function merge(base: Record<string, unknown>, top: Record<string, unknown>): void {
  for (const [key, value] of Object.entries(top)) {
    const existing = base[key];
    if (isTable(existing) && isTable(value)) merge(existing, value);
    else base[key] = value;
  }
}

export function backendSocket({ paths, config }: LoadedConfig): string {
  return config.backend?.socket ?? join(paths.runtimeDir, "backend.sock");
}
