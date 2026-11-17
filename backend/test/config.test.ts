import { expect, test } from "bun:test";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { backendSocket, loadConfig, resolvePaths, resolveSecret } from "../src/config.ts";

test("XDG resolution matches crates/viz-config", () => {
  expect(resolvePaths({ HOME: "/h", XDG_RUNTIME_DIR: "/run/user/1" })).toEqual({
    configDir: "/h/.config/viz",
    stateDir: "/h/.local/state/viz",
    runtimeDir: "/run/user/1/viz",
  });
  expect(resolvePaths({ HOME: "/h", VIZ_CONFIG_DIR: "/etc/viz", STATE_DIRECTORY: "/s" })).toEqual({
    configDir: "/etc/viz",
    stateDir: "/s",
    runtimeDir: "/s",
  });
  expect(() => resolvePaths({})).toThrow();
});

test("drop-ins merge over config.toml", async () => {
  const dir = await mkdtemp(join(tmpdir(), "viz-config-"));
  await mkdir(join(dir, "config.d"));
  await writeFile(
    join(dir, "config.toml"),
    `[backend]\nlisten = "127.0.0.1:1"\n[orchestrator]\nthinking = "high"\nmodel = { provider = "anthropic", model = "a" }\n[connectors.discord]\nbot_token = { file = "/x" }\n`,
  );
  await writeFile(join(dir, "config.d/10.toml"), `[orchestrator.model]\nmodel = "b"\n[backend]\nsocket = "/tmp/s"\n`);
  await writeFile(join(dir, "config.d/notes.txt"), "not toml [");

  const loaded = await loadConfig({ HOME: dir, VIZ_CONFIG_DIR: dir });
  expect(loaded.layers).toEqual([join(dir, "config.toml"), join(dir, "config.d/10.toml")]);
  expect(loaded.config.orchestrator).toEqual({ thinking: "high", model: { provider: "anthropic", model: "b" } });
  expect(backendSocket(loaded)).toBe("/tmp/s");
  await rm(dir, { recursive: true });
});

test("invalid sections are reported with their path", async () => {
  const dir = await mkdtemp(join(tmpdir(), "viz-config-"));
  await writeFile(join(dir, "config.toml"), `[orchestrator]\nthinking = "max"\n`);
  expect(loadConfig({ HOME: dir, VIZ_CONFIG_DIR: dir })).rejects.toThrow("/orchestrator/thinking");
  await rm(dir, { recursive: true });
});

test("secrets resolve from inline, file, and env", async () => {
  const dir = await mkdtemp(join(tmpdir(), "viz-secret-"));
  await writeFile(join(dir, "token"), "from-file\n");
  expect(await resolveSecret("inline")).toBe("inline");
  expect(await resolveSecret({ file: join(dir, "token") })).toBe("from-file");
  expect(await resolveSecret({ env: "T" }, { T: "from-env" })).toBe("from-env");
  expect(resolveSecret({ env: "MISSING" }, {})).rejects.toThrow();
  await rm(dir, { recursive: true });
});
