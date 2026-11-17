/**
 * viz configuration: XDG locations, layered TOML loading, and the backend's sections.
 *
 * Layers (later wins): `$VIZ_CONFIG_DIR/config.toml` (default `$XDG_CONFIG_HOME/viz/config.toml`), then
 * `config.d/*.toml` in lexical order. Tables merge recursively; other values replace. Mirrors
 * `crates/viz-config`; keep the two in sync.
 */
import { readdir } from "node:fs/promises";
import { join } from "node:path";

import { compile, map, obj, oneOf, opt, type Static, str, union, arr } from "@earendil-works/pi-schema";

export type Env = Readonly<Record<string, string | undefined>>;

export type Paths = {
  /** `$VIZ_CONFIG_DIR`, default `$XDG_CONFIG_HOME/viz`. */
  readonly configDir: string;
  /** `$STATE_DIRECTORY` (systemd `StateDirectory=`), default `$XDG_STATE_HOME/viz`. */
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
  return { configDir, stateDir, runtimeDir: runtime ? join(runtime, "viz") : stateDir };
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

/** Sections the backend owns. Other sections (`[client]`, `[connectors.*]`) belong to the Rust components. */
export const BackendConfig = obj(
  {
    backend: opt(
      obj({
        socket: opt(str({ description: "Unix socket path; default $XDG_RUNTIME_DIR/viz/backend.sock" })),
        listen: opt(str({ description: "TCP host:port for remote clients (tailnet/LAN); requires token" })),
        token: opt(Secret),
      }),
    ),
    orchestrator: opt(
      obj({
        model: opt(ModelRef),
        thinking: opt(oneOf(["off", "minimal", "low", "medium", "high", "xhigh"])),
        instructions: opt(str({ description: "Extra instructions appended to the orchestrator's system prompt" })),
        cwd: opt(str({ description: "Working directory for the orchestrator's tools; default $HOME" })),
      }),
    ),
    subagents: opt(obj({ model: opt(ModelRef) })),
    harnesses: opt(
      map(
        obj({
          command: str({ description: "ACP agent executable, e.g. claude-agent-acp, codex-acp, opencode" }),
          args: opt(arr(str())),
          env: opt(map(str())),
        }),
      ),
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
  const layers = [join(paths.configDir, "config.toml")];
  const dropins = join(paths.configDir, "config.d");
  try {
    const names = (await readdir(dropins)).filter((name) => name.endsWith(".toml")).sort();
    layers.push(...names.map((name) => join(dropins, name)));
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
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
