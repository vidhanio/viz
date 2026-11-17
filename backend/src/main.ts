/** viz-backend: opens the harness and serves clients until SIGINT/SIGTERM. */
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";

import pkg from "../package.json" with { type: "json" };
import { backendSocket, loadConfig, resolveSecret } from "./config.ts";
import { openHarness } from "./harness.ts";
import { serve } from "./server.ts";

const context = BACKGROUND_CONTEXT;
const loaded = await loadConfig();
const { backend } = loaded.config;
console.log(`viz-backend ${pkg.version}: config ${loaded.layers.join(", ") || "(none)"}`);

const viz = await openHarness(loaded, {}, context);
const server = await serve(viz, {
  socket: backendSocket(loaded),
  ...(backend?.listen && { listen: backend.listen }),
  ...(backend?.token && { token: await resolveSecret(backend.token) }),
  version: pkg.version,
});
console.log(`listening on ${server.socket}${server.url ? ` and ${server.url}` : ""}`);

let stopping = false;
const shutdown = async (signal: string) => {
  if (stopping) return;
  stopping = true;
  console.log(`${signal}: shutting down`);
  await server.stop();
  await viz.harness.close(context);
  process.exit(0);
};
process.on("SIGINT", () => void shutdown("SIGINT"));
process.on("SIGTERM", () => void shutdown("SIGTERM"));
