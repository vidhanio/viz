import { afterAll, beforeAll, expect, test } from "bun:test";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { createModels } from "@earendil-works/pi-ai/models";
import { fauxAssistantMessage, fauxProvider, fauxText } from "@earendil-works/pi-ai/providers/faux";

import { backendSocket, loadConfig } from "../src/config.ts";
import { openHarness, type VizHarness } from "../src/harness.ts";
import { serve, type VizServer } from "../src/server.ts";

const context = BACKGROUND_CONTEXT;
const TOKEN = "test-token-0123456789";
let dir: string;
let viz: VizHarness;
let server: VizServer;
const faux = fauxProvider();

beforeAll(async () => {
  dir = await mkdtemp(join(tmpdir(), "viz-backend-"));
  await writeFile(
    join(dir, "config.toml"),
    `[orchestrator]\nmodel = { provider = "faux", model = "${faux.getModel().id}" }\ncwd = "${dir}"\n`,
  );
  const loaded = await loadConfig({ HOME: dir, VIZ_CONFIG_DIR: dir, XDG_RUNTIME_DIR: join(dir, "run") });
  const models = createModels();
  models.setProvider(faux.provider);
  // A real SQLite file: exercises pi-durable's node:sqlite storage under Bun.
  viz = await openHarness(loaded, { models }, context);
  server = await serve(viz, { socket: backendSocket(loaded), listen: "127.0.0.1:0", token: TOKEN, version: "test" });
});

afterAll(async () => {
  await server.stop();
  await viz.harness.close(context);
  await rm(dir, { recursive: true });
});

test("health over the unix socket needs no token", async () => {
  const response = await fetch("http://localhost/v1/health", { unix: server.socket });
  expect(await response.json()).toEqual({ ok: true, version: "test", protocol: 1 });
});

test("TCP requires the bearer token", async () => {
  expect((await fetch(`${server.url}/v1/health`)).status).toBe(401);
  const ok = await fetch(`${server.url}/v1/health`, { headers: { authorization: `Bearer ${TOKEN}` } });
  expect(ok.status).toBe(200);
});

test("a connector's input is answered back to that connector only", async () => {
  faux.setResponses([fauxAssistantMessage([fauxText("pong")])]);
  const open = (name: string) => {
    const ws = new WebSocket(`${server.url!.replace("http", "ws")}/v1/rpc?token=${TOKEN}`);
    const inbox: any[] = [];
    const waiters: ((m: any) => void)[] = [];
    ws.onmessage = (event) => {
      const message = JSON.parse(String(event.data));
      const waiter = waiters.shift();
      if (waiter) waiter(message);
      else inbox.push(message);
    };
    let id = 0;
    const next = () => new Promise<any>((resolve) => (inbox.length ? resolve(inbox.shift()) : waiters.push(resolve)));
    const call = async (method: string, params: unknown) => {
      ws.send(JSON.stringify({ jsonrpc: "2.0", id: ++id, method, params }));
      return next();
    };
    return {
      ready: new Promise((resolve) => (ws.onopen = resolve)),
      hello: () => call("hello", { protocol: 1, client: { kind: "connector", name } }),
      call,
      next,
      inbox,
      close: () => ws.close(),
    };
  };

  const discord = open("discord");
  const other = open("imessage");
  await Promise.all([discord.ready, other.ready]);

  const notHello = await discord.call("conversations.list", {});
  expect(notHello.error.code).toBe(-32000);

  const hello = await discord.hello();
  expect(hello.result).toEqual({ protocol: 1, version: "test", root: viz.root.id });
  await other.hello();

  const invalid = await discord.call("conversation.submit", { text: 1 });
  expect(invalid.error.code).toBe(-32602);

  const origin = { connector: "discord", channel: "123", author: "alice" };
  const submitted = await discord.call("conversation.submit", { text: "ping", origin, requestId: "msg-1" });
  expect(submitted.result.submission).toBeNumber();

  const reply = await discord.next();
  expect(reply).toMatchObject({ method: "reply", params: { origin, status: "done", text: "pong" } });
  expect(other.inbox).toEqual([]);

  // The retry with the same requestId is the same submission, answered once.
  const retried = await discord.call("conversation.submit", { text: "ping", origin, requestId: "msg-1" });
  expect(retried.result.submission).toBe(submitted.result.submission);

  const listed = await discord.call("conversations.list", {});
  expect(listed.result.conversations.map((c: { id: number }) => c.id)).toContain(viz.root.id);

  discord.close();
  other.close();
});
