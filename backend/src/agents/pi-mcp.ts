/** pi-acp 0.0.34 drops ACP's mcpServers. Load a session-scoped Pi extension using its Pi command hook. */
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

const launcher = `#!/bin/sh
exec "$VIZ_PI_COMMAND" "$@" --extension "$VIZ_PI_EXTENSION"
`;
const extensionSource = `export default (pi) => {
  const { VIZ_PI_MCP_URL, VIZ_PI_MCP_TOKEN } = process.env;
  pi.registerMcpServer("viz", {
    url: VIZ_PI_MCP_URL,
    headers: { Authorization: "Bearer " + VIZ_PI_MCP_TOKEN },
    exposure: "direct",
  });
};
`;

export async function piMcp(url: string, token: string, env: Record<string, string>) {
  const directory = await mkdtemp(join(tmpdir(), "viz-pi-"));
  const command = join(directory, "pi");
  const extension = join(directory, "viz.ts");
  const dispose = () => {
    void rm(directory, { force: true, recursive: true }).catch((error: unknown) =>
      console.error("Removing Pi MCP extension:", error instanceof Error ? error.message : error),
    );
  };
  try {
    await writeFile(command, launcher, { mode: 0o700 });
    await writeFile(extension, extensionSource, { mode: 0o600 });
  } catch (error) {
    dispose();
    throw error;
  }
  return {
    env: {
      ...env,
      PI_ACP_PI_COMMAND: command,
      VIZ_PI_COMMAND: env.PI_ACP_PI_COMMAND ?? "pi",
      VIZ_PI_EXTENSION: extension,
      VIZ_PI_MCP_URL: url,
      VIZ_PI_MCP_TOKEN: token,
    },
    dispose,
  };
}
