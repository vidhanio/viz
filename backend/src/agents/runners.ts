import type { BackendConfig, Harness } from "../config.ts";

export class Runners {
  readonly #configured: ReadonlyMap<string, Harness>;
  readonly #default: string | undefined;

  constructor(config: BackendConfig) {
    this.#configured = new Map(Object.entries(config.harnesses ?? {}));
    const selected = config.backend?.default_harness;
    if (selected !== undefined && !this.#configured.has(selected)) {
      throw new Error(`[backend].default_harness names unconfigured ACP harness ${selected}`);
    }
    this.#default = selected ?? (this.#configured.size === 1 ? this.#configured.keys().next().value : undefined);
  }

  get names(): readonly string[] {
    return [...this.#configured.keys()];
  }

  require(name = this.#default): { name: string; config: Harness } {
    if (name === undefined) {
      throw new Error(
        this.#configured.size === 0
          ? "no ACP harness configured; add [harnesses.<name>]"
          : "no default ACP harness; set [backend].default_harness",
      );
    }
    const config = this.#configured.get(name);
    if (config === undefined) throw new Error(`ACP harness ${name} is not configured`);
    return { name, config };
  }
}
